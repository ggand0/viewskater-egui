//! The RAW formats that are TIFF files: a chain of image file directories
//! (IFDs), each a list of tags, and some tags point to child IFDs. The
//! JPEG is in a different place in each format:
//!
//! - ARW: IFD0, as the `JPEGInterchangeFormat` offset and length tags.
//!   Bodies from 2020 on have a second, full-size one in a later IFD.
//! - NEF: a child IFD of IFD0, as the same pair of tags.
//! - CR2: IFD0, as a single strip with JPEG compression.
//! - DNG: a child IFD, as a single strip with JPEG compression.
//! - RW2: IFD0, in Panasonic's tag 0x002E.
//! - ORF: inside the maker note, see `olympus_jpegs`.
//!
//! KDC, NRW, PEF, RWL, SR2, SRF and SRW files have theirs in one of these
//! places too.
//!
//! So every IFD is walked and every JPEG in it is collected. A CR2 and a
//! DNG may store the sensor data as a lossless JPEG too. The frame header
//! check in `pick_for_display` tells the two apart.

use std::io::{self, Read, Seek};

use image::metadata::Orientation;

use super::{Found, Source, Span};

/// Limits for the walk. The file is untrusted input: offsets may point
/// anywhere and IFDs may point at each other in a loop.
const MAX_IFDS: usize = 64;
const MAX_ENTRIES: usize = 1024;
const MAX_SUB_IFDS: usize = 16;
/// The most that is read from the start of a file for its EXIF.
const MAX_EXIF_PREFIX: u64 = 4 * 1024 * 1024;
/// A value larger than this is not worth extending the read for. An RW2
/// has its embedded JPEG and its sensor data as tag values in IFD0.
const MAX_EXIF_VALUE: u64 = 128 * 1024;

const TIFF_MAGIC: u16 = 42;
const RW2_MAGIC: u16 = 0x55;
/// An ORF has the letters "RO" or "RS" where a TIFF file has 42.
const ORF_MAGICS: [u16; 2] = [0x4F52, 0x5352];

const TAG_PANASONIC_JPEG: u16 = 0x002E;
const TAG_COMPRESSION: u16 = 0x0103;
const TAG_PHOTOMETRIC: u16 = 0x0106;
const TAG_STRIP_OFFSETS: u16 = 0x0111;
pub(super) const TAG_ORIENTATION: u16 = 0x0112;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_SUB_IFDS: u16 = 0x014A;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;
pub(super) const TAG_EXIF_IFD: u16 = 0x8769;
pub(super) const TAG_GPS_IFD: u16 = 0x8825;
pub(super) const TAG_INTEROP_IFD: u16 = 0xA005;
const TAG_MAKER_NOTE: u16 = 0x927C;
/// Tags of the Olympus maker note and of its camera settings IFD.
const TAG_OLYMPUS_THUMBNAIL: u16 = 0x0100;
const TAG_OLYMPUS_CAMERA_SETTINGS: u16 = 0x2020;
const TAG_OLYMPUS_PREVIEW_START: u16 = 0x0101;
const TAG_OLYMPUS_PREVIEW_LENGTH: u16 = 0x0102;

const TYPE_SHORT: u16 = 3;
pub(super) const TYPE_LONG: u16 = 4;
const TYPE_IFD: u16 = 13;

const COMPRESSION_OLD_JPEG: u32 = 6;
const COMPRESSION_JPEG: u32 = 7;
/// The photometric interpretation of sensor data, a color filter array.
const PHOTOMETRIC_CFA: u32 = 32803;

#[derive(Debug, Clone, Copy)]
struct TiffHeader {
    order: ByteOrder,
    magic: u16,
    ifd0: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    pub(super) fn u16(self, b: &[u8]) -> u16 {
        match self {
            ByteOrder::Little => u16::from_le_bytes([b[0], b[1]]),
            ByteOrder::Big => u16::from_be_bytes([b[0], b[1]]),
        }
    }

    pub(super) fn u32(self, b: &[u8]) -> u32 {
        match self {
            ByteOrder::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            ByteOrder::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        }
    }

    pub(super) fn u16_bytes(self, v: u16) -> [u8; 2] {
        match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        }
    }

    pub(super) fn u32_bytes(self, v: u32) -> [u8; 4] {
        match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        }
    }
}

/// One 12-byte IFD entry.
pub(super) struct Entry<'a> {
    pub(super) order: ByteOrder,
    pub(super) bytes: &'a [u8],
}

impl Entry<'_> {
    pub(super) fn tag(&self) -> u16 {
        self.order.u16(&self.bytes[0..2])
    }

    pub(super) fn kind(&self) -> u16 {
        self.order.u16(&self.bytes[2..4])
    }

    pub(super) fn count(&self) -> u32 {
        self.order.u32(&self.bytes[4..8])
    }

    /// The four value bytes as a number: the value itself when it fits,
    /// else the offset of the value.
    pub(super) fn value_or_offset(&self) -> u32 {
        self.order.u32(&self.bytes[8..12])
    }

    /// How many bytes the value takes. More than four means the entry
    /// holds the value's offset. Zero for a type TIFF does not define.
    pub(super) fn value_size(&self) -> u64 {
        let unit = match self.kind() {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => 0,
        };
        unit * self.count() as u64
    }

    /// A single SHORT or LONG value. A SHORT sits in the first two of the
    /// four value bytes.
    pub(super) fn single(&self) -> Option<u32> {
        if self.count() != 1 {
            return None;
        }
        match self.kind() {
            TYPE_SHORT => Some(self.order.u16(&self.bytes[8..10]) as u32),
            TYPE_LONG | TYPE_IFD => Some(self.value_or_offset()),
            _ => None,
        }
    }
}

/// The byte order and the offset of IFD0 from the header of a TIFF block
/// in memory. `None` when the block does not start with a TIFF header.
pub(super) fn header_of(block: &[u8]) -> Option<(ByteOrder, u32)> {
    let order = match block.get(..2)? {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return None,
    };
    (order.u16(block.get(2..4)?) == TIFF_MAGIC).then_some((order, order.u32(block.get(4..8)?)))
}

/// The entries of the IFD at `offset` in a TIFF block in memory, 12 bytes
/// each. `None` when they do not fit inside the block.
pub(super) fn entries_of(block: &[u8], order: ByteOrder, offset: u32) -> Option<&[u8]> {
    let start = offset as usize;
    let count = order.u16(block.get(start..start.checked_add(2)?)?) as usize;
    block.get(start + 2..(start + 2).checked_add(count * 12)?)
}

/// The JPEG candidates, the orientation and the EXIF block of a TIFF-based
/// RAW file. `None` when the file is not TIFF-based.
pub(super) fn find<R: Read + Seek>(source: &mut Source<R>) -> io::Result<Option<Found>> {
    let (mut found, tiff) = walk(source)?;
    let Some(tiff) = tiff else {
        return Ok(None);
    };
    if ORF_MAGICS.contains(&tiff.magic) {
        found.jpegs.extend(olympus_jpegs(source, tiff)?);
    }
    found.exif = Some(exif_block(source, tiff)?);
    Ok(Some(found))
}

/// The entries of the IFD at `offset`, 12 bytes each, followed by the
/// four bytes with the offset of the next IFD in the chain. `None` when
/// there is no readable IFD at `offset`.
fn read_ifd<R: Read + Seek>(source: &mut Source<R>, order: ByteOrder, offset: u64) -> io::Result<Option<Vec<u8>>> {
    let mut count = [0; 2];
    if !source.contains(offset, 2) {
        return Ok(None);
    }
    source.read_at(offset, &mut count)?;
    let count = order.u16(&count) as usize;
    if count == 0 || count > MAX_ENTRIES {
        return Ok(None);
    }
    let mut body = vec![0; count * 12 + 4];
    if !source.contains(offset + 2, body.len() as u64) {
        return Ok(None);
    }
    source.read_at(offset + 2, &mut body)?;
    Ok(Some(body))
}

/// Walk every IFD of a TIFF-based RAW file and collect the places that
/// may hold a JPEG, plus IFD0's orientation. A file that is not
/// TIFF-based gives no header and nothing found. An IFD that cannot be
/// read is skipped and the walk goes on.
fn walk<R: Read + Seek>(source: &mut Source<R>) -> io::Result<(Found, Option<TiffHeader>)> {
    let mut found = Found::nothing();
    let mut header = [0; 8];
    if source.read_at(0, &mut header).is_err() {
        return Ok((found, None));
    }
    let order = match &header[..2] {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return Ok((found, None)),
    };
    let magic = order.u16(&header[2..4]);
    if magic != TIFF_MAGIC && magic != RW2_MAGIC && !ORF_MAGICS.contains(&magic) {
        return Ok((found, None));
    }

    let ifd0 = order.u32(&header[4..8]) as u64;

    let mut pending = vec![ifd0];
    let mut seen = Vec::new();
    while let Some(offset) = pending.pop() {
        if offset == 0 || seen.contains(&offset) || seen.len() == MAX_IFDS {
            continue;
        }
        let is_ifd0 = seen.is_empty();
        seen.push(offset);

        let Some(body) = read_ifd(source, order, offset)? else {
            continue;
        };
        let count = body.len() / 12;

        let mut compression = None;
        let mut photometric = None;
        let mut strip = (None, None);
        let mut jpeg = (None, None);
        let mut children = Vec::new();
        for bytes in body[..count * 12].chunks_exact(12) {
            let entry = Entry { order, bytes };
            match entry.tag() {
                TAG_COMPRESSION => compression = entry.single(),
                TAG_PHOTOMETRIC => photometric = entry.single(),
                TAG_STRIP_OFFSETS => strip.0 = entry.single(),
                TAG_STRIP_BYTE_COUNTS => strip.1 = entry.single(),
                TAG_JPEG_OFFSET => jpeg.0 = entry.single(),
                TAG_JPEG_LENGTH => jpeg.1 = entry.single(),
                TAG_ORIENTATION if is_ifd0 => {
                    let turn = entry.single().and_then(|v| u8::try_from(v).ok());
                    found.orientation = turn.and_then(Orientation::from_exif).unwrap_or(found.orientation);
                }
                // The value is the JPEG itself, so the count is its length.
                TAG_PANASONIC_JPEG if magic == RW2_MAGIC => {
                    found.jpegs.push(Span { offset: entry.value_or_offset() as u64, len: entry.count() as u64 });
                }
                TAG_SUB_IFDS => children = sub_ifds(source, &entry)?,
                _ => {}
            }
        }
        if let (Some(offset), Some(len)) = jpeg {
            found.jpegs.push(Span { offset: offset as u64, len: len as u64 });
        }
        let jpeg_compressed = matches!(compression, Some(COMPRESSION_OLD_JPEG | COMPRESSION_JPEG));
        if let (Some(offset), Some(len), true) = (strip.0, strip.1, jpeg_compressed) {
            if photometric != Some(PHOTOMETRIC_CFA) {
                found.jpegs.push(Span { offset: offset as u64, len: len as u64 });
            }
        }

        // The next IFD in the chain, then the children.
        pending.push(order.u32(&body[count * 12..]) as u64);
        pending.extend(children);
    }

    found.jpegs.sort_by_key(|span| (span.offset, span.len));
    found.jpegs.dedup();
    Ok((found, Some(TiffHeader { order, magic, ifd0 })))
}

/// The start of the file, long enough to hold IFD0, the Exif, GPS and
/// interoperability IFDs and every value their entries point to. Offsets
/// in a TIFF file count from the start of the file, so this prefix is a
/// TIFF block that an EXIF parser reads as it is. An RW2 and an ORF get
/// the TIFF magic number in place of their own.
///
/// Values may lie behind image data, so the prefix can contain a
/// thumbnail. It stops at `MAX_EXIF_PREFIX`, and the parser skips a value
/// that is cut off or was left out for its size.
fn exif_block<R: Read + Seek>(source: &mut Source<R>, tiff: TiffHeader) -> io::Result<Vec<u8>> {
    let mut end = 8;
    let mut ifds = vec![tiff.ifd0];
    // IFD0 and the three it leads to, and no more than that whatever a
    // broken file points at.
    for _ in 0..4 {
        let Some(offset) = ifds.pop() else {
            break;
        };
        let Some(body) = read_ifd(source, tiff.order, offset)? else {
            continue;
        };
        end = end.max(offset + 2 + body.len() as u64);
        for bytes in body[..body.len() - 4].chunks_exact(12) {
            let entry = Entry { order: tiff.order, bytes };
            if matches!(entry.tag(), TAG_EXIF_IFD | TAG_GPS_IFD | TAG_INTEROP_IFD) {
                ifds.extend(entry.single().map(u64::from));
            }
            let size = entry.value_size();
            if size > 4 && size <= MAX_EXIF_VALUE && source.contains(entry.value_or_offset() as u64, size) {
                end = end.max(entry.value_or_offset() as u64 + size);
            }
        }
    }
    let mut block = vec![0; end.min(MAX_EXIF_PREFIX).min(source.len) as usize];
    source.read_at(0, &mut block)?;
    if tiff.magic != TIFF_MAGIC && block.len() >= 4 {
        let magic = match tiff.order {
            ByteOrder::Little => TIFF_MAGIC.to_le_bytes(),
            ByteOrder::Big => TIFF_MAGIC.to_be_bytes(),
        };
        block[2..4].copy_from_slice(&magic);
    }
    Ok(block)
}

/// The JPEGs of an Olympus or OM System ORF. They are inside the maker
/// note, which the Exif IFD points to. The maker note is a header and an
/// IFD:
///
/// - Tag 0x0100 is a 160x120 JPEG.
/// - Tag 0x2020 points to the camera settings IFD, where tags 0x0101 and
///   0x0102 are the offset and the length of a 1600x1200 or 3200x2400
///   JPEG.
///
/// The header is "OLYMPUS\0" and four more bytes, or "OM SYSTEM\0\0\0" and
/// four more bytes on bodies from 2022 on, and the offsets in the maker
/// note count from its start. Bodies up to about 2007 have "OLYMP\0"
/// and two more bytes, offsets that count from the start of the file, and
/// only the small JPEG.
fn olympus_jpegs<R: Read + Seek>(source: &mut Source<R>, tiff: TiffHeader) -> io::Result<Vec<Span>> {
    let mut jpegs = Vec::new();
    let order = tiff.order;
    let find = |ifd: &[u8], tag: u16| -> Option<(u32, u32, bool)> {
        ifd[..ifd.len() - 4]
            .chunks_exact(12)
            .map(|bytes| Entry { order, bytes })
            .find(|entry| entry.tag() == tag)
            .map(|entry| (entry.value_or_offset(), entry.count(), entry.single().is_some()))
    };

    let Some(ifd0) = read_ifd(source, order, tiff.ifd0)? else {
        return Ok(jpegs);
    };
    let Some((exif_at, _, true)) = find(&ifd0, TAG_EXIF_IFD) else {
        return Ok(jpegs);
    };
    let Some(exif) = read_ifd(source, order, exif_at as u64)? else {
        return Ok(jpegs);
    };
    let Some((note, _, false)) = find(&exif, TAG_MAKER_NOTE) else {
        return Ok(jpegs);
    };
    let note = note as u64;

    let mut header = [0; 12];
    if !source.contains(note, header.len() as u64) {
        return Ok(jpegs);
    }
    source.read_at(note, &mut header)?;
    // Where the maker note's IFD starts, and what its offsets count from.
    let (ifd_at, base) = if header.starts_with(b"OLYMPUS\0") {
        (note + 12, note)
    } else if header.starts_with(b"OM SYSTEM\0") {
        (note + 16, note)
    } else if header.starts_with(b"OLYMP\0") {
        (note + 8, 0)
    } else {
        return Ok(jpegs);
    };
    let Some(note_ifd) = read_ifd(source, order, ifd_at)? else {
        return Ok(jpegs);
    };

    if let Some((offset, len, false)) = find(&note_ifd, TAG_OLYMPUS_THUMBNAIL) {
        jpegs.push(Span { offset: base + offset as u64, len: len as u64 });
    }
    if let Some((settings_at, _, true)) = find(&note_ifd, TAG_OLYMPUS_CAMERA_SETTINGS) {
        if let Some(settings) = read_ifd(source, order, base + settings_at as u64)? {
            let start = find(&settings, TAG_OLYMPUS_PREVIEW_START);
            let len = find(&settings, TAG_OLYMPUS_PREVIEW_LENGTH);
            if let (Some((start, _, true)), Some((len, _, true))) = (start, len) {
                jpegs.push(Span { offset: base + start as u64, len: len as u64 });
            }
        }
    }
    Ok(jpegs)
}

/// The offsets in a SubIFDs tag. One offset sits in the entry, more are
/// an array somewhere else in the file.
fn sub_ifds<R: Read + Seek>(source: &mut Source<R>, entry: &Entry) -> io::Result<Vec<u64>> {
    if !matches!(entry.kind(), TYPE_LONG | TYPE_IFD) {
        return Ok(Vec::new());
    }
    let count = (entry.count() as usize).min(MAX_SUB_IFDS);
    if count <= 1 {
        return Ok(entry.single().map(u64::from).into_iter().collect());
    }
    let mut array = vec![0; count * 4];
    let offset = entry.value_or_offset() as u64;
    if !source.contains(offset, array.len() as u64) {
        return Ok(Vec::new());
    }
    source.read_at(offset, &mut array)?;
    Ok(array.chunks_exact(4).map(|b| entry.order.u32(b) as u64).collect())
}

/// Small TIFF-based files for the tests here, in `cr3` and in `file_io`.
#[cfg(test)]
pub(in crate::raw) mod test_files {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    use super::*;

    /// Writes IFDs and data at the offsets the test chooses.
    pub(in crate::raw) struct TiffBuilder {
        order: ByteOrder,
        bytes: Vec<u8>,
    }

    impl TiffBuilder {
        pub(in crate::raw) fn new(order: ByteOrder, magic: u16, first_ifd: u32) -> Self {
            let mut builder = Self { order, bytes: Vec::new() };
            let mark = if order == ByteOrder::Little { b"II" } else { b"MM" };
            builder.bytes.extend_from_slice(mark);
            let magic = builder.u16(magic);
            let first = builder.u32(first_ifd);
            builder.bytes.extend_from_slice(&magic);
            builder.bytes.extend_from_slice(&first);
            builder
        }

        fn u16(&self, v: u16) -> [u8; 2] {
            if self.order == ByteOrder::Little { v.to_le_bytes() } else { v.to_be_bytes() }
        }

        fn u32(&self, v: u32) -> [u8; 4] {
            if self.order == ByteOrder::Little { v.to_le_bytes() } else { v.to_be_bytes() }
        }

        pub(in crate::raw) fn place(&mut self, offset: usize, data: &[u8]) {
            if self.bytes.len() < offset + data.len() {
                self.bytes.resize(offset + data.len(), 0);
            }
            self.bytes[offset..offset + data.len()].copy_from_slice(data);
        }

        /// Entries are (tag, type, count, value). A SHORT value goes into
        /// the first two value bytes, as TIFF asks.
        pub(in crate::raw) fn ifd(&mut self, offset: usize, entries: &[(u16, u16, u32, u32)], next: u32) {
            let mut out = self.u16(entries.len() as u16).to_vec();
            for &(tag, kind, count, value) in entries {
                out.extend_from_slice(&self.u16(tag));
                out.extend_from_slice(&self.u16(kind));
                out.extend_from_slice(&self.u32(count));
                if kind == TYPE_SHORT {
                    out.extend_from_slice(&self.u16(value as u16));
                    out.extend_from_slice(&[0, 0]);
                } else {
                    out.extend_from_slice(&self.u32(value));
                }
            }
            out.extend_from_slice(&self.u32(next));
            self.place(offset, &out);
        }

        pub(in crate::raw) fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    pub(crate) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let pixels = vec![128u8; (width * height * 3) as usize];
        JpegEncoder::new(&mut out).write_image(&pixels, width, height, ExtendedColorType::Rgb8).unwrap();
        out
    }

    /// A file laid out like a Nikon NEF: `jpeg` in a child IFD of IFD0,
    /// `orientation` and the make "NIKON" in IFD0.
    pub(crate) fn nef_like(jpeg: &[u8], orientation: u32) -> Vec<u8> {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (0x010F, 2, 6, 60),
            (TAG_ORIENTATION, TYPE_SHORT, 1, orientation),
            (TAG_SUB_IFDS, TYPE_LONG, 1, 100),
        ], 0);
        tiff.place(60, b"NIKON\0");
        tiff.ifd(100, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 6),
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 1000),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, jpeg.len() as u32),
        ], 0);
        tiff.place(1000, jpeg);
        tiff.finish()
    }

    /// A RAW file with sensor data and no JPEG, like a DNG from a cinema
    /// camera.
    pub(crate) fn without_jpeg() -> Vec<u8> {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 7),
            (TAG_PHOTOMETRIC, TYPE_SHORT, 1, PHOTOMETRIC_CFA),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 1000),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, 100),
        ], 0);
        tiff.place(1000, &[0; 100]);
        tiff.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::test_files::{jpeg, TiffBuilder};
    use super::*;
    use crate::raw::{contents, lossy_jpeg_size, pick_for_display};
    use crate::metadata::{parse_exif, ExifData};

    /// The start of a lossless JPEG, as a CR2 stores its sensor data: a
    /// Huffman table segment, then a frame header with marker 0xC3.
    fn lossless_jpeg_start() -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00];
        out.extend_from_slice(&[0xFF, 0xC3, 0x00, 0x0B, 14, 0x0E, 0x7C, 0x0A, 0xE0, 2, 0, 0, 0]);
        out.resize(4096, 0);
        out
    }

    fn shown(file: Vec<u8>) -> Option<(u32, u32, Orientation)> {
        let found = contents(Cursor::new(file)).unwrap();
        let image = image::load_from_memory(&found.jpeg?).unwrap();
        Some((image.width(), image.height(), found.orientation))
    }

    /// Sony ARW: the JPEG offset and length tags in IFD0, a thumbnail in
    /// IFD1, a full-size JPEG in IFD2. The 1616x1080 one is shown.
    #[test]
    fn arw_shows_the_mid_size_jpeg() {
        let (thumb, preview, full) = (jpeg(160, 120), jpeg(1616, 1080), jpeg(3000, 2000));
        assert!(preview.len() < full.len());
        let full_at = 8000 + preview.len() as u32 + 100;
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 6),
            (TAG_ORIENTATION, TYPE_SHORT, 1, 6),
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 8000),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, preview.len() as u32),
        ], 100);
        tiff.ifd(100, &[
            (TAG_ORIENTATION, TYPE_SHORT, 1, 8),
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 300),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, thumb.len() as u32),
        ], 200);
        tiff.ifd(200, &[
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, full_at),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, full.len() as u32),
        ], 0);
        assert!(thumb.len() < 7000);
        tiff.place(300, &thumb);
        tiff.place(8000, &preview);
        tiff.place(full_at as usize, &full);

        assert_eq!(shown(tiff.finish()), Some((1616, 1080, Orientation::Rotate90)));

        // A 4:3 camera's 1440x1080 is shown too, and not the full-size JPEG.
        let (preview, full) = (jpeg(1440, 1080), jpeg(3000, 2250));
        let full_at = 1000 + preview.len() as u32 + 100;
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 1000),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, preview.len() as u32),
        ], 100);
        tiff.ifd(100, &[
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, full_at),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, full.len() as u32),
        ], 0);
        tiff.place(1000, &preview);
        tiff.place(full_at as usize, &full);
        assert_eq!(shown(tiff.finish()), Some((1440, 1080, Orientation::NoTransforms)));
    }

    /// Nikon NEF, big-endian here: IFD0 is an uncompressed thumbnail and
    /// the JPEG is in the first of two child IFDs. The second child is the
    /// sensor data. With nothing at 1600 pixels the largest JPEG is shown.
    #[test]
    fn nef_jpeg_is_in_a_child_ifd() {
        let preview = jpeg(640, 424);
        let mut tiff = TiffBuilder::new(ByteOrder::Big, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 1),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 400),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, 100),
            (TAG_SUB_IFDS, TYPE_LONG, 2, 80),
        ], 0);
        tiff.place(80, &[0, 0, 0, 100, 0, 0, 0, 160]);
        tiff.ifd(100, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 6),
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 1000),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, preview.len() as u32),
        ], 0);
        tiff.ifd(160, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 34713),
            (TAG_PHOTOMETRIC, TYPE_SHORT, 1, PHOTOMETRIC_CFA),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 500),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, 100),
        ], 0);
        tiff.place(1000, &preview);

        assert_eq!(shown(tiff.finish()), Some((640, 424, Orientation::NoTransforms)));
    }

    /// Canon CR2: the JPEG is a strip of IFD0. The sensor data in a later
    /// IFD is a strip with the same compression value, a lossless JPEG,
    /// and is larger. It is not shown.
    #[test]
    fn cr2_sensor_data_is_not_mistaken_for_the_jpeg() {
        let (preview, sensor) = (jpeg(1000, 8), lossless_jpeg_start());
        assert!(preview.len() < sensor.len());
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 16);
        tiff.ifd(16, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 6),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 1000),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, preview.len() as u32),
        ], 100);
        tiff.ifd(100, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 6),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 10_000),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, sensor.len() as u32),
        ], 0);
        tiff.place(1000, &preview);
        tiff.place(10_000, &sensor);
        assert_eq!(shown(tiff.finish()), Some((1000, 8, Orientation::NoTransforms)));

        // Only sensor data: nothing to show.
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 16);
        tiff.ifd(16, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 6),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 1000),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, sensor.len() as u32),
        ], 0);
        tiff.place(1000, &sensor);
        assert_eq!(shown(tiff.finish()), None);
    }

    /// DNG: the JPEG is a strip in a child IFD. Sensor data compressed as
    /// a JPEG is skipped by its photometric value, before any read of it.
    #[test]
    fn dng_jpeg_is_a_strip_in_a_child_ifd() {
        let preview = jpeg(1024, 8);
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[(TAG_SUB_IFDS, TYPE_LONG, 2, 60)], 0);
        tiff.place(60, &[100, 0, 0, 0, 160, 0, 0, 0]);
        tiff.ifd(100, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 7),
            (TAG_PHOTOMETRIC, TYPE_SHORT, 1, PHOTOMETRIC_CFA),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 5000),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, 100),
        ], 0);
        tiff.ifd(160, &[
            (TAG_COMPRESSION, TYPE_SHORT, 1, 7),
            (TAG_PHOTOMETRIC, TYPE_SHORT, 1, 6),
            (TAG_STRIP_OFFSETS, TYPE_LONG, 1, 1000),
            (TAG_STRIP_BYTE_COUNTS, TYPE_LONG, 1, preview.len() as u32),
        ], 0);
        tiff.place(1000, &preview);
        tiff.place(5000, &[0; 100]);

        let file = tiff.finish();
        let mut source = Source::new(Cursor::new(file.clone())).unwrap();
        assert_eq!(walk(&mut source).unwrap().0.jpegs, [Span { offset: 1000, len: preview.len() as u64 }]);
        assert_eq!(shown(file), Some((1024, 8, Orientation::NoTransforms)));
    }

    /// Panasonic RW2: its own magic number, and the JPEG in tag 0x002E.
    #[test]
    fn rw2_jpeg_is_in_the_panasonic_tag() {
        let preview = jpeg(1920, 8);
        let mut tiff = TiffBuilder::new(ByteOrder::Little, RW2_MAGIC, 24);
        tiff.ifd(24, &[
            (TAG_PANASONIC_JPEG, 7, preview.len() as u32, 1000),
            (TAG_ORIENTATION, TYPE_SHORT, 1, 3),
        ], 0);
        tiff.place(1000, &preview);
        assert_eq!(shown(tiff.finish()), Some((1920, 8, Orientation::Rotate180)));
    }

    /// An ORF with the maker note at 300. `header` is the maker note's
    /// header, and `base` is what the offsets inside it count from.
    fn orf(header: &[u8], base: u32, thumbnail: &[u8], preview: Option<&[u8]>) -> Vec<u8> {
        let note_ifd = 300 + header.len();
        let mut tiff = TiffBuilder::new(ByteOrder::Little, ORF_MAGICS[0], 8);
        tiff.ifd(8, &[
            (TAG_ORIENTATION, TYPE_SHORT, 1, 6),
            (TAG_EXIF_IFD, TYPE_LONG, 1, 100),
        ], 0);
        tiff.ifd(100, &[(TAG_MAKER_NOTE, 7, 200, 300)], 0);
        tiff.place(300, header);
        let mut entries = vec![(TAG_OLYMPUS_THUMBNAIL, 7, thumbnail.len() as u32, 1000 - base)];
        if let Some(preview) = preview {
            entries.push((TAG_OLYMPUS_CAMERA_SETTINGS, TYPE_IFD, 1, 400 - base));
            tiff.ifd(400, &[
                (TAG_OLYMPUS_PREVIEW_START, TYPE_LONG, 1, 20_000 - base),
                (TAG_OLYMPUS_PREVIEW_LENGTH, TYPE_LONG, 1, preview.len() as u32),
            ], 0);
            tiff.place(20_000, preview);
        }
        tiff.ifd(note_ifd, &entries, 0);
        tiff.place(1000, thumbnail);
        tiff.finish()
    }

    /// Olympus ORF: the JPEGs are inside the maker note, and its offsets
    /// count from the maker note's start. OM System bodies have a longer
    /// header. Old bodies count from the start of the file and only have
    /// the 160x120 JPEG.
    #[test]
    fn orf_jpegs_are_inside_the_maker_note() {
        let (thumbnail, preview) = (jpeg(160, 120), jpeg(1600, 1200));
        assert!(thumbnail.len() < 19_000);

        let olympus = orf(b"OLYMPUS\0II\x03\0", 300, &thumbnail, Some(&preview));
        assert_eq!(shown(olympus), Some((1600, 1200, Orientation::Rotate90)));

        let om_system = orf(b"OM SYSTEM\0\0\0II\x04\0", 300, &thumbnail, Some(&preview));
        assert_eq!(shown(om_system), Some((1600, 1200, Orientation::Rotate90)));

        let old = orf(b"OLYMP\0\x01\0", 0, &thumbnail, None);
        assert_eq!(shown(old), Some((160, 120, Orientation::Rotate90)));

        // A maker note from another maker: nothing, and no error.
        let other = orf(b"Nikon\0\x02\x10\0\0", 300, &thumbnail, Some(&preview));
        assert_eq!(shown(other), None);

        // The EXIF block of an ORF parses, with 42 written over "RO".
        let block = contents(Cursor::new(orf(b"OLYMPUS\0II\x03\0", 300, &thumbnail, None))).unwrap().exif.unwrap();
        assert_eq!(block[..4], *b"II*\0");
        let ExifData::Present(exif) = parse_exif(block) else {
            panic!("the block did not parse");
        };
        assert_eq!(exif.orientation.as_deref(), Some("Rotate 90° CW"));
    }

    const TAG_MAKE: u16 = 0x010F;
    const TAG_EXPOSURE_TIME: u16 = 0x829A;
    const TYPE_ASCII: u16 = 2;
    const TYPE_RATIONAL: u16 = 5;
    const TYPE_UNDEFINED: u16 = 7;

    /// The EXIF block reaches past a thumbnail that sits between IFD0 and
    /// the Exif IFD, as in a Pentax DNG, and ends with the last value.
    #[test]
    fn exif_block_ends_with_the_last_value() {
        let mut tiff = TiffBuilder::new(ByteOrder::Big, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_MAKE, TYPE_ASCII, 7, 40),
            (TAG_EXIF_IFD, TYPE_LONG, 1, 5000),
        ], 0);
        tiff.place(40, b"PENTAX\0");
        tiff.place(100, &[0x55; 4000]);
        tiff.ifd(5000, &[(TAG_EXPOSURE_TIME, TYPE_RATIONAL, 1, 5100)], 0);
        tiff.place(5100, &[0, 0, 0, 1, 0, 0, 0, 125]);
        tiff.place(9000, &[0x55; 100]);

        let block = contents(Cursor::new(tiff.finish())).unwrap().exif.unwrap();
        assert_eq!(block.len(), 5108);
        let ExifData::Present(exif) = parse_exif(block) else {
            panic!("the block did not parse");
        };
        assert_eq!(exif.camera.as_deref(), Some("PENTAX"));
        assert_eq!(exif.shutter.as_deref(), Some("1/125 s"));
    }

    /// An RW2 has image data as tag values in IFD0. The block does not
    /// grow to take them in, and it has the TIFF magic number.
    #[test]
    fn exif_block_of_an_rw2_leaves_the_image_data_out() {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, RW2_MAGIC, 24);
        tiff.ifd(24, &[
            (TAG_PANASONIC_JPEG, TYPE_UNDEFINED, 200_000, 3000),
            (TAG_MAKE, TYPE_ASCII, 10, 100),
        ], 0);
        tiff.place(100, b"Panasonic\0");
        tiff.place(3000, &[0x55; 200_000]);

        let block = contents(Cursor::new(tiff.finish())).unwrap().exif.unwrap();
        assert_eq!(block.len(), 110);
        assert_eq!(block[..4], *b"II*\0");
        let ExifData::Present(exif) = parse_exif(block) else {
            panic!("the block did not parse");
        };
        assert_eq!(exif.camera.as_deref(), Some("Panasonic"));
    }

    #[test]
    fn broken_files_give_nothing() {
        assert!(shown(Vec::new()).is_none());
        assert!(shown(b"II".to_vec()).is_none());
        assert!(shown(b"this is not a TIFF file at all".to_vec()).is_none());
        assert!(contents(Cursor::new(b"this is not a TIFF file at all".to_vec())).unwrap().exif.is_none());

        // The first IFD is outside the file.
        assert!(shown(TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 5000).finish()).is_none());

        // IFD0 names itself as the next IFD and as its own child.
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[(TAG_SUB_IFDS, TYPE_LONG, 1, 8)], 8);
        assert!(shown(tiff.finish()).is_none());

        // An entry count that runs past the end of the file.
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.place(8, &[0xFF, 0x03]);
        assert!(shown(tiff.finish()).is_none());

        // A JPEG whose offset and length point outside the file, and one
        // whose bytes are not a JPEG.
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 4_000_000),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, 1000),
        ], 100);
        tiff.ifd(100, &[
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 200),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, 64),
        ], 0);
        tiff.place(200, &[0x11; 64]);
        assert!(shown(tiff.finish()).is_none());
    }

    /// Prints what the walk finds in real RAW files:
    ///
    ///     VIEWSKATER_RAW_FILES=a.arw:b.nef cargo test real_raw_files -- --ignored --nocapture
    ///
    /// The list uses the platform's path list separator, `:` or `;` on
    /// Windows.
    #[test]
    #[ignore]
    fn real_raw_files_have_an_embedded_jpeg() {
        let Ok(list) = std::env::var("VIEWSKATER_RAW_FILES") else {
            eprintln!("VIEWSKATER_RAW_FILES is not set");
            return;
        };
        for path in std::env::split_paths(&list) {
            let mut source = Source::new(std::fs::File::open(&path).unwrap()).unwrap();
            let (found, tiff) = walk(&mut source).unwrap();
            eprintln!("{}", path.display());
            eprintln!("  orientation: {:?}", found.orientation);
            for span in &found.jpegs {
                let size = lossy_jpeg_size(&mut source, *span).unwrap();
                eprintln!("  candidate at {} with {} bytes: {:?}", span.offset, span.len, size);
            }
            let picked = pick_for_display(&mut source, &found.jpegs).unwrap();
            eprintln!("  shown: {picked:?}");
            if let Some(tiff) = tiff {
                eprintln!("  EXIF block: {} bytes", exif_block(&mut source, tiff).unwrap().len());
            }
        }
    }
}

