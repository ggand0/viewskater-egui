//! Camera RAW files, shown through the JPEG the camera stored inside them.
//!
//! A camera renders a JPEG when the shot is taken and writes it into the
//! RAW file next to the sensor data. This module finds that JPEG and
//! reads it. The sensor data is never read.
//!
//! `tiff` finds it in the formats that are TIFF files: ARW, CR2, DNG, NEF
//! and RW2. Each finder returns every place that may hold a JPEG, and the
//! frame header of each candidate decides here which one is shown.

use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use image::metadata::Orientation;

mod tiff;

#[cfg(test)]
pub(crate) use tiff::test_files;

pub const EXTENSIONS: &[&str] = &["arw", "cr2", "dng", "nef", "rw2"];

/// The JPEG shown for a RAW file is the smallest embedded one with at
/// least this many pixels on its short side, or the largest when none
/// has. Cameras store a JPEG of about 1080 lines for their own screen:
/// 1616x1080 from Sony, 1620x1080 from Nikon and Canon, 1440x1080 from a
/// Canon with a 4:3 sensor, 1920 wide from Panasonic. Those pass, and the
/// 160x120 thumbnails do not. The long side would not do as the measure,
/// because 1440 is less than 1600 and that file's other JPEG is full
/// size.
const MIN_SHORT_SIDE: u32 = 1000;

/// The file is untrusted input, so the walk over a JPEG's segments has a
/// limit like the walks over the containers.
const MAX_JPEG_SEGMENTS: usize = 256;

/// What is read from a RAW file.
pub struct RawContents {
    /// The JPEG to show, or `None` when the file has none.
    pub jpeg: Option<Vec<u8>>,
    /// The turn the RAW file's orientation tag asks for. The embedded
    /// JPEG of an ARW, CR2, DNG or NEF has no EXIF block of its own.
    pub orientation: Orientation,
    /// The start of the file, up to the end of its EXIF values, as a
    /// TIFF block that `metadata::parse_exif` reads. `None` when the file
    /// is not TIFF-based.
    pub exif: Option<Vec<u8>>,
}

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.iter().any(|raw| ext.eq_ignore_ascii_case(raw)))
}

pub fn read(path: &Path) -> io::Result<RawContents> {
    contents(std::fs::File::open(path)?)
}

fn contents(file: impl Read + Seek) -> io::Result<RawContents> {
    let mut source = Source::new(file)?;
    let found = match tiff::find(&mut source)? {
        Some(found) => found,
        None => Found::nothing(),
    };
    let jpeg = match pick_for_display(&mut source, &found.jpegs)? {
        Some(span) => {
            let mut bytes = vec![0; span.len as usize];
            source.read_at(span.offset, &mut bytes)?;
            Some(bytes)
        }
        None => None,
    };
    Ok(RawContents { jpeg, orientation: found.orientation, exif: found.exif })
}

/// Where a JPEG is inside the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    offset: u64,
    len: u64,
}

/// What the walk over a container found.
struct Found {
    /// Every place that may hold a JPEG. `pick_for_display` checks them.
    jpegs: Vec<Span>,
    orientation: Orientation,
    /// The file's EXIF as a TIFF block for `metadata::parse_exif`.
    exif: Option<Vec<u8>>,
}

impl Found {
    fn nothing() -> Self {
        Self { jpegs: Vec::new(), orientation: Orientation::NoTransforms, exif: None }
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

/// The smallest JPEG with `MIN_SHORT_SIDE` pixels on its short side, or
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
        if width.min(height) >= MIN_SHORT_SIDE {
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
