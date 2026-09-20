//! Camera RAW files, shown through the JPEG the camera stored inside them.
//!
//! A camera renders a JPEG when the shot is taken and writes it into the
//! RAW file next to the sensor data. This module finds that JPEG and
//! reads it. The sensor data is never read.
//!
//! ARW, CR2, DNG, NEF and RW2 are TIFF files: a chain of image file
//! directories (IFDs), each a list of tags, and some tags point to child
//! IFDs. The JPEG is in a different place in each format:
//!
//! - ARW: IFD0, as the `JPEGInterchangeFormat` offset and length tags.
//!   Bodies from 2020 on have a second, full-size one in a later IFD.
//! - NEF: a child IFD of IFD0, as the same pair of tags.
//! - CR2: IFD0, as a single strip with JPEG compression.
//! - DNG: a child IFD, as a single strip with JPEG compression.
//! - RW2: IFD0, in Panasonic's tag 0x002E.
//!
//! So every IFD is walked and every JPEG in it is collected. A CR2 and a
//! DNG may store the sensor data as a lossless JPEG too, and the frame
//! header of each candidate tells the two apart.

use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use image::metadata::Orientation;

pub const EXTENSIONS: &[&str] = &["arw", "cr2", "dng", "nef", "rw2"];

/// The JPEG shown for a RAW file is the smallest embedded one with at
/// least this many pixels on its long side, or the largest when none has.
/// Sony's 1616x1080 and Panasonic's 1920 wide JPEGs pass. The 160x120
/// thumbnails do not.
const MIN_LONG_SIDE: u32 = 1600;

/// Limits for the walk. The file is untrusted input: offsets may point
/// anywhere and IFDs may point at each other in a loop.
const MAX_IFDS: usize = 64;
const MAX_ENTRIES: usize = 1024;
const MAX_SUB_IFDS: usize = 16;
const MAX_JPEG_SEGMENTS: usize = 256;

const TIFF_MAGIC: u16 = 42;
const RW2_MAGIC: u16 = 0x55;

const TAG_PANASONIC_JPEG: u16 = 0x002E;
const TAG_COMPRESSION: u16 = 0x0103;
const TAG_PHOTOMETRIC: u16 = 0x0106;
const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_ORIENTATION: u16 = 0x0112;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_SUB_IFDS: u16 = 0x014A;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;

const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_IFD: u16 = 13;

const COMPRESSION_OLD_JPEG: u32 = 6;
const COMPRESSION_JPEG: u32 = 7;
/// The photometric interpretation of sensor data, a color filter array.
const PHOTOMETRIC_CFA: u32 = 32803;

/// The JPEG to show for a RAW file.
pub struct EmbeddedJpeg {
    pub bytes: Vec<u8>,
    /// The turn the RAW file's orientation tag asks for. The embedded
    /// JPEG of an ARW, CR2, DNG or NEF has no EXIF block of its own.
    pub orientation: Orientation,
}

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.iter().any(|raw| ext.eq_ignore_ascii_case(raw)))
}

/// The embedded JPEG of the RAW file at `path`, or `None` when the file
/// has none.
pub fn read_embedded_jpeg(path: &Path) -> io::Result<Option<EmbeddedJpeg>> {
    embedded_jpeg(std::fs::File::open(path)?)
}

fn embedded_jpeg(file: impl Read + Seek) -> io::Result<Option<EmbeddedJpeg>> {
    let mut source = Source::new(file)?;
    let found = walk(&mut source)?;
    let Some(jpeg) = pick_for_display(&mut source, &found.jpegs)? else {
        return Ok(None);
    };
    let mut bytes = vec![0; jpeg.len as usize];
    source.read_at(jpeg.offset, &mut bytes)?;
    Ok(Some(EmbeddedJpeg { bytes, orientation: found.orientation }))
}

/// Where a JPEG is inside the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    offset: u64,
    len: u64,
}

struct Found {
    jpegs: Vec<Span>,
    orientation: Orientation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn u16(self, b: &[u8]) -> u16 {
        match self {
            ByteOrder::Little => u16::from_le_bytes([b[0], b[1]]),
            ByteOrder::Big => u16::from_be_bytes([b[0], b[1]]),
        }
    }

    fn u32(self, b: &[u8]) -> u32 {
        match self {
            ByteOrder::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            ByteOrder::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        }
    }
}

/// Reads at absolute offsets. The position is tracked here so that a
/// read close to the last one moves inside `BufReader`'s buffer and does
/// not reach the file.
struct Source<R> {
    inner: BufReader<R>,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> Source<R> {
    fn new(mut file: R) -> io::Result<Self> {
        let len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Self { inner: BufReader::new(file), pos: 0, len })
    }

    /// Whether `len` bytes at `offset` are inside the file.
    fn contains(&self, offset: u64, len: u64) -> bool {
        offset.checked_add(len).is_some_and(|end| end <= self.len)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if !self.contains(offset, buf.len() as u64) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.inner.seek_relative(offset as i64 - self.pos as i64)?;
        self.pos = offset;
        self.inner.read_exact(buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }
}

/// One 12-byte IFD entry.
struct Entry<'a> {
    order: ByteOrder,
    bytes: &'a [u8],
}

impl Entry<'_> {
    fn tag(&self) -> u16 {
        self.order.u16(&self.bytes[0..2])
    }

    fn kind(&self) -> u16 {
        self.order.u16(&self.bytes[2..4])
    }

    fn count(&self) -> u32 {
        self.order.u32(&self.bytes[4..8])
    }

    /// The four value bytes as a number: the value itself when it fits,
    /// else the offset of the value.
    fn value_or_offset(&self) -> u32 {
        self.order.u32(&self.bytes[8..12])
    }

    /// A single SHORT or LONG value. A SHORT sits in the first two of the
    /// four value bytes.
    fn single(&self) -> Option<u32> {
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

/// Walk every IFD of a TIFF-based RAW file and collect the places that
/// may hold a JPEG, plus IFD0's orientation. A file that is not
/// TIFF-based gives an empty result. An IFD that cannot be read is
/// skipped and the walk goes on.
fn walk<R: Read + Seek>(source: &mut Source<R>) -> io::Result<Found> {
    let mut found = Found { jpegs: Vec::new(), orientation: Orientation::NoTransforms };
    let mut header = [0; 8];
    if source.read_at(0, &mut header).is_err() {
        return Ok(found);
    }
    let order = match &header[..2] {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return Ok(found),
    };
    let magic = order.u16(&header[2..4]);
    if magic != TIFF_MAGIC && magic != RW2_MAGIC {
        return Ok(found);
    }

    let mut pending = vec![order.u32(&header[4..8]) as u64];
    let mut seen = Vec::new();
    while let Some(offset) = pending.pop() {
        if offset == 0 || seen.contains(&offset) || seen.len() == MAX_IFDS {
            continue;
        }
        let is_ifd0 = seen.is_empty();
        seen.push(offset);

        let mut count = [0; 2];
        if !source.contains(offset, 2) {
            continue;
        }
        source.read_at(offset, &mut count)?;
        let count = order.u16(&count) as usize;
        if count == 0 || count > MAX_ENTRIES {
            continue;
        }
        // The entries and the offset of the next IFD in the chain.
        let mut body = vec![0; count * 12 + 4];
        if !source.contains(offset + 2, body.len() as u64) {
            continue;
        }
        source.read_at(offset + 2, &mut body)?;

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

    found.jpegs.retain(|span| span.len > 0 && source.contains(span.offset, span.len));
    found.jpegs.sort_by_key(|span| (span.offset, span.len));
    found.jpegs.dedup();
    Ok(found)
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

/// The smallest JPEG with `MIN_LONG_SIDE` pixels on its long side, or
/// the one with the most pixels when none is that large. Candidates are
/// tried from the fewest bytes up, so a file with a mid-size JPEG never
/// has the header of its full-size one read.
fn pick_for_display<R: Read + Seek>(source: &mut Source<R>, jpegs: &[Span]) -> io::Result<Option<Span>> {
    let mut by_len = jpegs.to_vec();
    by_len.sort_by_key(|span| span.len);
    let mut largest: Option<(Span, u64)> = None;
    for span in by_len {
        let Some((width, height)) = lossy_jpeg_size(source, span)? else {
            continue;
        };
        if width.max(height) >= MIN_LONG_SIDE {
            return Ok(Some(span));
        }
        let pixels = width as u64 * height as u64;
        if largest.is_none_or(|(_, most)| pixels > most) {
            largest = Some((span, pixels));
        }
    }
    Ok(largest.map(|(span, _)| span))
}

/// Width and height from the frame header of a baseline, extended or
/// progressive JPEG. `None` for anything else: bytes that are not a JPEG,
/// and the lossless JPEG a CR2 or a DNG stores its sensor data in.
fn lossy_jpeg_size<R: Read + Seek>(source: &mut Source<R>, span: Span) -> io::Result<Option<(u32, u32)>> {
    let end = span.offset + span.len;
    let mut soi = [0; 2];
    if span.len < 4 {
        return Ok(None);
    }
    source.read_at(span.offset, &mut soi)?;
    if soi != [0xFF, 0xD8] {
        return Ok(None);
    }
    let mut at = span.offset + 2;
    for _ in 0..MAX_JPEG_SEGMENTS {
        let mut head = [0; 4];
        if at + 4 > end {
            return Ok(None);
        }
        source.read_at(at, &mut head)?;
        if head[0] != 0xFF {
            return Ok(None);
        }
        let segment_len = u16::from_be_bytes([head[2], head[3]]) as u64;
        match head[1] {
            // A fill byte before the marker.
            0xFF => at += 1,
            // Markers without a length: TEM and the restart markers.
            0x01 | 0xD0..=0xD7 => at += 2,
            // Baseline, extended sequential and progressive frames.
            0xC0..=0xC2 => {
                let mut frame = [0; 5];
                if at + 9 > end {
                    return Ok(None);
                }
                source.read_at(at + 4, &mut frame)?;
                let height = u16::from_be_bytes([frame[1], frame[2]]) as u32;
                let width = u16::from_be_bytes([frame[3], frame[4]]) as u32;
                return Ok((width > 0 && height > 0).then_some((width, height)));
            }
            // Lossless, differential and arithmetic frames, which the
            // JPEG decoder does not read. 0xC4, 0xC8 and 0xCC are not
            // frame headers.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Ok(None),
            // The scan or the end of the image before any frame header.
            0xDA | 0xD9 => return Ok(None),
            _ if segment_len < 2 => return Ok(None),
            _ => at += 2 + segment_len,
        }
    }
    Ok(None)
}

/// Small TIFF-based files for the tests here and in `file_io`.
#[cfg(test)]
pub(crate) mod test_files {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    use super::*;

    /// Writes IFDs and data at the offsets the test chooses.
    pub(super) struct TiffBuilder {
        order: ByteOrder,
        bytes: Vec<u8>,
    }

    impl TiffBuilder {
        pub(super) fn new(order: ByteOrder, magic: u16, first_ifd: u32) -> Self {
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

        pub(super) fn place(&mut self, offset: usize, data: &[u8]) {
            if self.bytes.len() < offset + data.len() {
                self.bytes.resize(offset + data.len(), 0);
            }
            self.bytes[offset..offset + data.len()].copy_from_slice(data);
        }

        /// Entries are (tag, type, count, value). A SHORT value goes into
        /// the first two value bytes, as TIFF asks.
        pub(super) fn ifd(&mut self, offset: usize, entries: &[(u16, u16, u32, u32)], next: u32) {
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

        pub(super) fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    pub(crate) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let pixels = vec![128u8; (width * height * 3) as usize];
        JpegEncoder::new(&mut out).write_image(&pixels, width, height, ExtendedColorType::Rgb8).unwrap();
        out
    }

    /// A file laid out like a Nikon NEF: `jpeg` in a child IFD of IFD0
    /// and `orientation` in IFD0.
    pub(crate) fn nef_like(jpeg: &[u8], orientation: u32) -> Vec<u8> {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, TIFF_MAGIC, 8);
        tiff.ifd(8, &[
            (TAG_ORIENTATION, TYPE_SHORT, 1, orientation),
            (TAG_SUB_IFDS, TYPE_LONG, 1, 100),
        ], 0);
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

    /// The start of a lossless JPEG, as a CR2 stores its sensor data: a
    /// Huffman table segment, then a frame header with marker 0xC3.
    fn lossless_jpeg_start() -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0x00, 0x00];
        out.extend_from_slice(&[0xFF, 0xC3, 0x00, 0x0B, 14, 0x0E, 0x7C, 0x0A, 0xE0, 2, 0, 0, 0]);
        out.resize(4096, 0);
        out
    }

    fn shown(file: Vec<u8>) -> Option<(u32, u32, Orientation)> {
        let found = embedded_jpeg(Cursor::new(file)).unwrap()?;
        let image = image::load_from_memory(&found.bytes).unwrap();
        Some((image.width(), image.height(), found.orientation))
    }

    /// Sony ARW: the JPEG offset and length tags in IFD0, a thumbnail in
    /// IFD1, a full-size JPEG in IFD2. The 1616 wide one is shown.
    #[test]
    fn arw_shows_the_mid_size_jpeg() {
        let (thumb, preview, full) = (jpeg(160, 120), jpeg(1616, 8), jpeg(6000, 8));
        assert!(preview.len() < full.len());
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
            (TAG_JPEG_OFFSET, TYPE_LONG, 1, 20_000),
            (TAG_JPEG_LENGTH, TYPE_LONG, 1, full.len() as u32),
        ], 0);
        assert!(thumb.len() < 7000 && preview.len() < 12_000);
        tiff.place(300, &thumb);
        tiff.place(8000, &preview);
        tiff.place(20_000, &full);

        assert_eq!(shown(tiff.finish()), Some((1616, 8, Orientation::Rotate90)));
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
        assert_eq!(walk(&mut source).unwrap().jpegs, [Span { offset: 1000, len: preview.len() as u64 }]);
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

    #[test]
    fn broken_files_give_nothing() {
        assert!(shown(Vec::new()).is_none());
        assert!(shown(b"II".to_vec()).is_none());
        assert!(shown(b"this is not a TIFF file at all".to_vec()).is_none());

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
            let found = walk(&mut source).unwrap();
            eprintln!("{}", path.display());
            eprintln!("  orientation: {:?}", found.orientation);
            for span in &found.jpegs {
                let size = lossy_jpeg_size(&mut source, *span).unwrap();
                eprintln!("  candidate at {} with {} bytes: {:?}", span.offset, span.len, size);
            }
            let picked = pick_for_display(&mut source, &found.jpegs).unwrap();
            eprintln!("  shown: {picked:?}");
        }
    }
}
