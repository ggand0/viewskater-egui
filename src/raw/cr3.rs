//! Canon CR3. The file has the structure of an MP4: a sequence of boxes,
//! each with a header of its size and a four-letter type, and some boxes
//! hold more boxes. A CR3 has a JPEG in three places:
//!
//! - `THMB`, 160x120, inside Canon's `uuid` box in `moov`.
//! - `PRVW`, 1620x1080, inside a `uuid` box of its own after `moov`.
//! - The full-size one is the only sample of the first track. The track's
//!   `stsz` box has its length and the `co64` box its offset.
//!
//! A CR3 shot with HDR PQ turned on has HEVC in all three places and no
//! JPEG. The frame header check in `pick_for_display` rejects those.
//!
//! The EXIF is in Canon's `uuid` box as separate TIFF blocks: `CMT1` is
//! IFD0, `CMT2` the Exif IFD, `CMT3` the maker note and `CMT4` the GPS
//! IFD. Each block has its own TIFF header, and its offsets count from
//! its own start, so `join_exif` builds one block out of them.

use std::io::{self, Read, Seek};

use image::metadata::Orientation;

use super::tiff::{self, ByteOrder, Entry, TAG_EXIF_IFD, TAG_GPS_IFD, TAG_INTEROP_IFD, TAG_ORIENTATION, TYPE_LONG};
use super::{Found, Source, Span};

const CANON_UUID: [u8; 16] = [
    0x85, 0xC0, 0xB6, 0x87, 0x82, 0x0F, 0x11, 0xE0, 0x81, 0x11, 0xF4, 0xCE, 0x46, 0x2B, 0x6A, 0x48,
];
const PREVIEW_UUID: [u8; 16] = [
    0xEA, 0xF4, 0x2B, 0x5E, 0x1C, 0x98, 0x4B, 0x88, 0xB9, 0xFB, 0xB7, 0xDC, 0x40, 0x6E, 0x4D, 0x16,
];

/// The file is untrusted input. No more boxes than this are read at one
/// level, and an EXIF block larger than this is left out. The blocks in
/// real files are 0.5 to 2 KB.
const MAX_BOXES: usize = 64;
const MAX_EXIF_BLOCK: u64 = 256 * 1024;

/// In `THMB` and in `PRVW` the image starts 16 bytes into the box body.
/// The fields before it differ between the two boxes and between their
/// versions, so the image is taken to run to the end of the box. A JPEG
/// decoder stops at the end marker and ignores the padding after it.
const IMAGE_AT: u64 = 16;
/// The `uuid` box with `PRVW` has 8 bytes between its id and `PRVW`.
const PREVIEW_BOXES_AT: u64 = 16 + 8;

struct BoxAt {
    kind: [u8; 4],
    /// Where the box's content starts, after its header.
    body: u64,
    end: u64,
}

/// The JPEG candidates, the orientation and the EXIF block of a CR3.
/// `None` when the file is not a CR3.
pub(super) fn find<R: Read + Seek>(source: &mut Source<R>) -> io::Result<Option<Found>> {
    let mut start = [0; 12];
    if !source.contains(0, 12) {
        return Ok(None);
    }
    source.read_at(0, &mut start)?;
    if &start[4..8] != b"ftyp" || &start[8..12] != b"crx " {
        return Ok(None);
    }

    let mut found = Found::nothing();
    // CMT1, CMT2 and CMT4.
    let mut exif_blocks: [Option<Vec<u8>>; 3] = [None, None, None];
    for top in boxes(source, 0, source.len)? {
        if &top.kind == b"moov" {
            for inner in boxes(source, top.body, top.end)? {
                if &inner.kind == b"trak" {
                    found.jpegs.extend(first_sample(source, &inner)?);
                } else if has_uuid(source, &inner, &CANON_UUID)? {
                    for canon in boxes(source, inner.body + 16, inner.end)? {
                        let slot = match &canon.kind {
                            b"CMT1" => 0,
                            b"CMT2" => 1,
                            b"CMT4" => 2,
                            b"THMB" => {
                                found.jpegs.extend(image_in(&canon));
                                continue;
                            }
                            _ => continue,
                        };
                        let len = canon.end - canon.body;
                        if len <= MAX_EXIF_BLOCK {
                            let mut block = vec![0; len as usize];
                            source.read_at(canon.body, &mut block)?;
                            exif_blocks[slot] = Some(block);
                        }
                    }
                }
            }
        } else if has_uuid(source, &top, &PREVIEW_UUID)? {
            for preview in boxes(source, top.body + PREVIEW_BOXES_AT, top.end)? {
                if &preview.kind == b"PRVW" {
                    found.jpegs.extend(image_in(&preview));
                }
            }
        }
    }

    let [ifd0, exif, gps] = exif_blocks;
    if let Some(ifd0) = ifd0 {
        found.orientation = orientation_of(&ifd0).unwrap_or(found.orientation);
        found.exif = join_exif(&ifd0, exif.as_deref(), gps.as_deref());
    }
    Ok(Some(found))
}

/// The boxes between `start` and `end`. The list ends at the first box
/// whose size does not fit.
fn boxes<R: Read + Seek>(source: &mut Source<R>, start: u64, end: u64) -> io::Result<Vec<BoxAt>> {
    let mut found = Vec::new();
    let mut at = start;
    while found.len() < MAX_BOXES && at.checked_add(8).is_some_and(|header_end| header_end <= end) {
        let mut header = [0; 8];
        source.read_at(at, &mut header)?;
        let mut size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let mut header_len = 8;
        if size == 1 {
            // The size does not fit in 32 bits and follows the type.
            let mut large = [0; 8];
            if at + 16 > end {
                break;
            }
            source.read_at(at + 8, &mut large)?;
            size = u64::from_be_bytes(large);
            header_len = 16;
        } else if size == 0 {
            // The box runs to the end of what holds it.
            size = end - at;
        }
        if size < header_len || size > end - at {
            break;
        }
        found.push(BoxAt { kind: [header[4], header[5], header[6], header[7]], body: at + header_len, end: at + size });
        at += size;
    }
    Ok(found)
}

/// Whether `b` is a `uuid` box with this id in its first 16 bytes.
fn has_uuid<R: Read + Seek>(source: &mut Source<R>, b: &BoxAt, uuid: &[u8; 16]) -> io::Result<bool> {
    if &b.kind != b"uuid" || b.end - b.body < 16 {
        return Ok(false);
    }
    let mut id = [0; 16];
    source.read_at(b.body, &mut id)?;
    Ok(&id == uuid)
}

/// The image in a `THMB` or a `PRVW` box.
fn image_in(b: &BoxAt) -> Option<Span> {
    let offset = b.body + IMAGE_AT;
    (offset < b.end).then(|| Span { offset, len: b.end - offset })
}

/// The first sample of a track: the first entry of `stsz` for the length
/// and of `co64` or `stco` for the offset. The tables are in
/// `mdia` / `minf` / `stbl`.
fn first_sample<R: Read + Seek>(source: &mut Source<R>, trak: &BoxAt) -> io::Result<Option<Span>> {
    let mut table = BoxAt { kind: trak.kind, body: trak.body, end: trak.end };
    for kind in [b"mdia", b"minf", b"stbl"] {
        let Some(next) = boxes(source, table.body, table.end)?.into_iter().find(|b| &b.kind == kind) else {
            return Ok(None);
        };
        table = next;
    }

    let (mut offset, mut len) = (None, None);
    for b in boxes(source, table.body, table.end)? {
        // Version and flags, then for `stsz` the size of every sample (or
        // 0), the count and the sizes, for `co64` and `stco` the count and
        // the offsets.
        let mut head = [0; 16];
        let available = ((b.end - b.body) as usize).min(head.len());
        source.read_at(b.body, &mut head[..available])?;
        let be32 = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
        match &b.kind {
            b"stsz" if available == 16 && be32(4) != 0 => len = Some(be32(4) as u64),
            b"stsz" if available == 16 && be32(8) > 0 => len = Some(be32(12) as u64),
            b"co64" if available == 16 && be32(4) > 0 => offset = Some((be32(8) as u64) << 32 | be32(12) as u64),
            b"stco" if available >= 12 && be32(4) > 0 => offset = Some(be32(8) as u64),
            _ => {}
        }
    }
    Ok(offset.zip(len).map(|(offset, len)| Span { offset, len }))
}

/// The orientation tag in IFD0 of a TIFF block.
fn orientation_of(block: &[u8]) -> Option<Orientation> {
    let (order, ifd0) = tiff::header_of(block)?;
    tiff::entries_of(block, order, ifd0)?
        .chunks_exact(12)
        .map(|bytes| Entry { order, bytes })
        .find(|entry| entry.tag() == TAG_ORIENTATION)
        .and_then(|entry| u8::try_from(entry.single()?).ok())
        .and_then(Orientation::from_exif)
}

/// One TIFF block out of Canon's separate ones. `ifd0` stays as it is, so
/// its offsets stay right. The Exif and the GPS block are appended, and
/// the offsets in their IFDs get the block's new position added. A new
/// IFD0 at the end has the old entries and the pointers to the two
/// appended IFDs, and the header points to it.
fn join_exif(ifd0: &[u8], exif: Option<&[u8]>, gps: Option<&[u8]>) -> Option<Vec<u8>> {
    let (order, ifd0_at) = tiff::header_of(ifd0)?;
    let mut entries: Vec<[u8; 12]> = tiff::entries_of(ifd0, order, ifd0_at)?
        .chunks_exact(12)
        .filter(|bytes| !matches!(Entry { order, bytes }.tag(), TAG_EXIF_IFD | TAG_GPS_IFD))
        .map(|bytes| bytes.try_into().expect("chunks_exact(12)"))
        .collect();

    let mut out = ifd0.to_vec();
    for (tag, block) in [(TAG_EXIF_IFD, exif), (TAG_GPS_IFD, gps)] {
        let Some((block, (block_order, first))) = block.and_then(|b| Some((b, tiff::header_of(b)?))) else {
            continue;
        };
        if block_order != order {
            continue;
        }
        out.resize(out.len().next_multiple_of(2), 0);
        let base = out.len() as u32;
        out.extend_from_slice(block);
        if !rebase_ifd(&mut out, order, base, first, true) {
            out.truncate(base as usize);
            continue;
        }
        let mut pointer = [0; 12];
        pointer[0..2].copy_from_slice(&order.u16_bytes(tag));
        pointer[2..4].copy_from_slice(&order.u16_bytes(TYPE_LONG));
        pointer[4..8].copy_from_slice(&order.u32_bytes(1));
        pointer[8..12].copy_from_slice(&order.u32_bytes(base + first));
        entries.push(pointer);
    }

    entries.sort_by_key(|bytes| Entry { order, bytes }.tag());
    out.resize(out.len().next_multiple_of(2), 0);
    let new_ifd0 = out.len() as u32;
    out.extend_from_slice(&order.u16_bytes(entries.len() as u16));
    for entry in &entries {
        out.extend_from_slice(entry);
    }
    out.extend_from_slice(&[0; 4]); // no next IFD
    out[4..8].copy_from_slice(&order.u32_bytes(new_ifd0));
    Some(out)
}

/// A block was appended to `out` at `base`. Add `base` to every offset in
/// its IFD at `ifd`: the values that do not fit in an entry, and the
/// pointer to the interoperability IFD, whose own offsets move too.
/// False when the IFD does not fit inside `out`, and `out` is then
/// partly changed.
fn rebase_ifd(out: &mut [u8], order: ByteOrder, base: u32, ifd: u32, follow_interop: bool) -> bool {
    let Some(start) = (base as usize).checked_add(ifd as usize) else {
        return false;
    };
    let Some(count) = out.get(start..start + 2).map(|b| order.u16(b) as usize) else {
        return false;
    };
    for i in 0..count {
        let at = start + 2 + i * 12;
        let Some(bytes) = out.get(at..at + 12) else {
            return false;
        };
        let entry = Entry { order, bytes };
        let is_interop = entry.tag() == TAG_INTEROP_IFD && entry.single().is_some();
        if entry.value_size() <= 4 && !is_interop {
            continue;
        }
        let old = entry.value_or_offset();
        let Some(moved) = old.checked_add(base) else {
            return false;
        };
        out[at + 8..at + 12].copy_from_slice(&order.u32_bytes(moved));
        if is_interop && follow_interop {
            rebase_ifd(out, order, base, old, false);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::metadata::{parse_exif, ExifData};
    use crate::raw::contents;
    use crate::raw::test_files::jpeg;
    use crate::raw::tiff::test_files::TiffBuilder;

    fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    /// 16 bytes of box fields, then the image.
    fn image_box(kind: &[u8; 4], image: &[u8]) -> Vec<u8> {
        boxed(kind, &[&[0; 16][..], image].concat())
    }

    /// A track whose only sample is `len` bytes at `offset`.
    fn track(offset: u64, len: u32) -> Vec<u8> {
        let stsz = boxed(b"stsz", &[&[0; 8][..], &1u32.to_be_bytes(), &len.to_be_bytes()].concat());
        let co64 = boxed(b"co64", &[&[0; 4][..], &1u32.to_be_bytes(), &offset.to_be_bytes()].concat());
        let stbl = boxed(b"stbl", &[boxed(b"stsd", &[0; 8]), stsz, co64].concat());
        boxed(b"trak", &boxed(b"mdia", &boxed(b"minf", &stbl)))
    }

    const TAG_MAKE: u16 = 0x010F;
    const TAG_EXPOSURE_TIME: u16 = 0x829A;
    const TAG_GPS_LATITUDE_REF: u16 = 1;
    const TAG_GPS_LATITUDE: u16 = 2;
    const TAG_GPS_LONGITUDE_REF: u16 = 3;
    const TAG_GPS_LONGITUDE: u16 = 4;

    /// CMT1: the make as a value outside the entry, and orientation 6.
    fn cmt1() -> Vec<u8> {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, 42, 8);
        tiff.ifd(8, &[(TAG_MAKE, 2, 6, 40), (TAG_ORIENTATION, 3, 1, 6)], 0);
        tiff.place(40, b"Canon\0");
        tiff.finish()
    }

    /// CMT2: the exposure time, a rational outside the entry.
    fn cmt2() -> Vec<u8> {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, 42, 8);
        tiff.ifd(8, &[(TAG_EXPOSURE_TIME, 5, 1, 30)], 0);
        tiff.place(30, &[1, 0, 0, 0, 250, 0, 0, 0]);
        tiff.finish()
    }

    /// CMT4: 35 degrees north, 139 degrees east.
    fn cmt4() -> Vec<u8> {
        let mut tiff = TiffBuilder::new(ByteOrder::Little, 42, 8);
        tiff.ifd(8, &[
            (TAG_GPS_LATITUDE_REF, 2, 2, u32::from_le_bytes(*b"N\0\0\0")),
            (TAG_GPS_LATITUDE, 5, 3, 70),
            (TAG_GPS_LONGITUDE_REF, 2, 2, u32::from_le_bytes(*b"E\0\0\0")),
            (TAG_GPS_LONGITUDE, 5, 3, 100),
        ], 0);
        let degrees = |d: u8| [d, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
        tiff.place(70, &degrees(35));
        tiff.place(100, &degrees(139));
        tiff.finish()
    }

    /// A CR3 with the three images in their places. The full-size one
    /// sits right after the boxes, where `mdat` would be.
    fn cr3(thumbnail: &[u8], preview: &[u8], full: &[u8]) -> Vec<u8> {
        let canon = [
            &CANON_UUID[..],
            &boxed(b"CNCV", b"CanonCR3"),
            &boxed(b"CMT1", &cmt1()),
            &boxed(b"CMT2", &cmt2()),
            &boxed(b"CMT3", &[0; 40]),
            &boxed(b"CMT4", &cmt4()),
            &image_box(b"THMB", thumbnail),
        ]
        .concat();
        let preview_uuid = boxed(b"uuid", &[&PREVIEW_UUID[..], &[0; 8], &image_box(b"PRVW", preview)].concat());
        let ftyp = boxed(b"ftyp", b"crx \0\0\0\x01crx isom");

        // The track holds the full-size image's offset, which depends on
        // the length of everything before it. A track box has the same
        // length whatever the offset is.
        let before = |track: &[u8]| {
            let moov = boxed(b"moov", &[&boxed(b"uuid", &canon)[..], &boxed(b"mvhd", &[0; 100]), track].concat());
            [&ftyp[..], &moov, &preview_uuid].concat()
        };
        let mdat_at = before(&track(0, 0)).len() as u64;
        let mut file = before(&track(mdat_at + 8, full.len() as u32));
        file.extend(boxed(b"mdat", full));
        file
    }

    #[test]
    fn cr3_shows_the_preview_and_joins_the_exif_blocks() {
        let file = cr3(&jpeg(160, 120), &jpeg(1620, 1080), &jpeg(3000, 2000));

        let mut source = Source::new(Cursor::new(file.clone())).unwrap();
        let found = find(&mut source).unwrap().unwrap();
        assert_eq!(found.jpegs.len(), 3, "THMB, PRVW and the first track");

        let found = contents(Cursor::new(file)).unwrap();
        let shown = image::load_from_memory(&found.jpeg.unwrap()).unwrap();
        assert_eq!((shown.width(), shown.height()), (1620, 1080));
        assert_eq!(found.orientation, Orientation::Rotate90);

        let ExifData::Present(exif) = parse_exif(found.exif.unwrap()) else {
            panic!("the joined block did not parse");
        };
        assert_eq!(exif.camera.as_deref(), Some("Canon"));
        assert_eq!(exif.shutter.as_deref(), Some("1/250 s"));
        assert_eq!(exif.orientation.as_deref(), Some("Rotate 90° CW"));
        let location = exif.location.expect("the GPS block");
        assert_eq!((location.latitude, location.longitude), (35.0, 139.0));
    }

    /// With HDR PQ turned on the three places hold HEVC. There is nothing
    /// to show, and the EXIF is there all the same.
    #[test]
    fn cr3_without_a_jpeg_still_has_its_exif() {
        let hevc = [&[0, 0, 0, 0x14][..], b"CISZ", &[0x55; 300]].concat();
        let found = contents(Cursor::new(cr3(&hevc, &hevc, &hevc))).unwrap();
        assert!(found.jpeg.is_none());
        assert!(matches!(parse_exif(found.exif.unwrap()), ExifData::Present(_)));
    }

    #[test]
    fn broken_cr3_files_give_nothing() {
        let ftyp = boxed(b"ftyp", b"crx \0\0\0\x01crx isom");

        // Only the file type box.
        assert!(contents(Cursor::new(ftyp.clone())).unwrap().jpeg.is_none());

        // A box that claims to be larger than the file, one smaller than
        // its own header, and a `uuid` box too short for an id.
        for bad in [
            [&u32::MAX.to_be_bytes()[..], b"moov"].concat(),
            [&4u32.to_be_bytes()[..], b"moov"].concat(),
            boxed(b"uuid", &[1, 2, 3]),
        ] {
            let found = contents(Cursor::new([&ftyp[..], &bad].concat())).unwrap();
            assert!(found.jpeg.is_none() && found.exif.is_none());
        }

        // A track that points outside the file.
        let moov = boxed(b"moov", &track(u64::MAX - 5, 1000));
        assert!(contents(Cursor::new([&ftyp[..], &moov].concat())).unwrap().jpeg.is_none());

        // An Exif block whose IFD runs past its end is left out, and IFD0
        // is kept.
        let mut cut = cmt2();
        cut[8] = 200;
        let joined = join_exif(&cmt1(), Some(&cut), None).unwrap();
        let ExifData::Present(exif) = parse_exif(joined) else {
            panic!("IFD0 alone did not parse");
        };
        assert_eq!(exif.camera.as_deref(), Some("Canon"));
        assert_eq!(exif.shutter, None);
    }
}
