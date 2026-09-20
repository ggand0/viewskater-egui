//! Fujifilm RAF. The file starts with a fixed header: 16 bytes of magic
//! text, and at byte 84 the offset and at byte 88 the length of one JPEG,
//! both big-endian. That JPEG is a complete camera JPEG, 1920x1280 from
//! older bodies and full size from newer ones, with its own EXIF block.
//! So the orientation and the EXIF are read from the JPEG and not from
//! the container.

use std::io::{self, Read, Seek};

use super::{Found, Source, Span};

const MAGIC: &[u8; 16] = b"FUJIFILMCCD-RAW ";
const JPEG_OFFSET_AT: usize = 84;
const JPEG_LENGTH_AT: usize = 88;

/// The JPEG of a RAF file. `None` when the file is not a RAF.
pub(super) fn find<R: Read + Seek>(source: &mut Source<R>) -> io::Result<Option<Found>> {
    let mut header = [0; JPEG_LENGTH_AT + 4];
    if !source.contains(0, header.len() as u64) {
        return Ok(None);
    }
    source.read_at(0, &mut header)?;
    if &header[..16] != MAGIC {
        return Ok(None);
    }
    let be = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]) as u64;
    let mut found = Found::nothing();
    found.jpegs.push(Span { offset: be(JPEG_OFFSET_AT), len: be(JPEG_LENGTH_AT) });
    found.exif_in_jpeg = true;
    Ok(Some(found))
}

#[cfg(test)]
pub(in crate::raw) mod test_files {
    use super::*;

    /// A RAF header followed by `jpeg`.
    pub(crate) fn raf(jpeg: &[u8]) -> Vec<u8> {
        let mut file = MAGIC.to_vec();
        file.resize(148, 0);
        file[JPEG_OFFSET_AT..JPEG_OFFSET_AT + 4].copy_from_slice(&148u32.to_be_bytes());
        file[JPEG_LENGTH_AT..JPEG_LENGTH_AT + 4].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
        file.extend_from_slice(jpeg);
        file
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::test_files::raf;
    use crate::raw::contents;
    use crate::raw::test_files::jpeg;

    #[test]
    fn raf_jpeg_is_at_the_offset_in_the_header() {
        let picture = jpeg(30, 20);
        let found = contents(Cursor::new(raf(&picture))).unwrap();
        assert_eq!(found.jpeg.as_deref(), Some(&picture[..]));
        assert!(found.exif_in_jpeg);
        assert!(found.exif.is_none());
    }

    #[test]
    fn broken_raf_files_give_nothing() {
        // The header is cut short.
        let found = contents(Cursor::new(b"FUJIFILMCCD-RAW 0201".to_vec())).unwrap();
        assert!(found.jpeg.is_none() && !found.exif_in_jpeg);

        // The JPEG is said to be longer than the file.
        let mut file = raf(&jpeg(30, 20));
        file[88..92].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(contents(Cursor::new(file)).unwrap().jpeg.is_none());

        // The bytes at the offset are not a JPEG.
        assert!(contents(Cursor::new(raf(&[0x11; 500]))).unwrap().jpeg.is_none());
    }
}
