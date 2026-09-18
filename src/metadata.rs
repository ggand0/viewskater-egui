//! What the metadata panel shows about the current image: facts from the
//! file system and from the EXIF block.
//!
//! Everything here runs on the decode thread, right after the file is
//! opened and before the pixels are decoded. The values are formatted
//! into owned strings there, so the panel does no parsing and no disk
//! access per frame. The record travels with the texture (see
//! `cache::Loaded`) and is dropped with it.

use std::fmt::{self, Write as _};

use exif::{Exif, Field, In, Rational, SRational, Tag, Value};

/// Longest display value kept for a tag in the All EXIF list. Formatting
/// stops at this point rather than producing the whole value and cutting
/// it: some tags are arrays of thousands of numbers.
const MAX_VALUE_CHARS: usize = 200;

/// Byte and Undefined values longer than this are shown as a byte count
/// and never formatted. MakerNote is shown as a count at any size.
const MAX_BINARY_BYTES: usize = 64;

/// Facts about one image file, formatted for display.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MetadataRecord {
    pub file_size: Option<u64>,
    /// Modification time in local time, "2026-03-27 14:05".
    pub modified: Option<String>,
    /// Container format as the content sniffer saw it: "JPEG", "PNG", "JXL".
    pub format: Option<String>,
    pub exif: ExifData,
}

/// The EXIF part of a record.
#[derive(Debug, Default, Clone, PartialEq)]
pub enum ExifData {
    /// The file carries no EXIF block.
    #[default]
    None,
    /// The file carries an EXIF block that could not be parsed.
    Unreadable,
    /// Boxed: the summary is a few hundred bytes of Options and the other
    /// variants carry nothing.
    Present(Box<ExifSummary>),
}

/// The curated values the panel sections show, plus every tag for the
/// All EXIF list. Each string is ready to display.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExifSummary {
    /// Make and model, deduplicated: "NIKON D750", "Apple iPhone 15".
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// "50 mm", or "24 mm (6.86 mm)" when the 35 mm equivalent exists
    /// and differs: the equivalent first, the real length in brackets.
    pub focal_length: Option<String>,
    /// The f-number as stored, "f/2.8", "f/1.78".
    pub aperture: Option<String>,
    /// "1/250 s" under 0.3 s, "0.5 s", "2 s" above.
    pub shutter: Option<String>,
    /// "ISO 400"
    pub iso: Option<String>,
    /// Exposure compensation, signed: "+0.3 EV", "-1.7 EV". None when
    /// it is zero, which is most photos.
    pub exposure_bias: Option<String>,
    /// "2026-03-27 14:05:12", with " +08:00" when the offset tag exists.
    pub date_taken: Option<String>,
    /// The orientation tag as text, "Rotate 90° CW". The app does not
    /// apply it yet, so this says how the picture on screen is turned.
    pub orientation: Option<String>,
    pub location: Option<Location>,
    /// Every tag as (name, value) in file order.
    pub tags: Vec<(String, String)>,
}

/// GPS position in decimal degrees, south and west negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude_m: Option<f64>,
}

impl Location {
    /// "-8.4095, 115.1889"
    pub fn text(&self) -> String {
        format!("{:.4}, {:.4}", self.latitude, self.longitude)
    }

    /// "12 m" or "-3 m", when the altitude tag exists.
    pub fn altitude_text(&self) -> Option<String> {
        self.altitude_m.map(|a| format!("{} m", trim_decimal(a, 0)))
    }

    /// OpenStreetMap with a marker at the point.
    pub fn map_url(&self) -> String {
        format!(
            "https://www.openstreetmap.org/?mlat={:.6}&mlon={:.6}#map=16/{:.6}/{:.6}",
            self.latitude, self.longitude, self.latitude, self.longitude
        )
    }
}

/// Parse an EXIF block as the decoder handed it over and format it.
///
/// `bytes` starts at the TIFF header, or at an "Exif\0\0" prefix some
/// WebP encoders keep; the prefix is removed. Tags that kamadak cannot
/// parse are skipped and the rest is kept; a block that cannot be parsed
/// at all is `Unreadable`.
pub fn parse_exif(bytes: Vec<u8>) -> ExifData {
    let bytes = strip_exif_prefix(bytes);
    if bytes.is_empty() {
        return ExifData::None;
    }
    let mut reader = exif::Reader::new();
    reader.continue_on_error(true);
    let parsed = reader.read_raw(bytes).or_else(|e| {
        e.distill_partial_result(|errors| {
            log::debug!("EXIF partially parsed, {} tags skipped", errors.len());
        })
    });
    match parsed {
        Ok(exif) => ExifData::Present(Box::new(summarize(&exif))),
        Err(e) => {
            log::debug!("EXIF unreadable: {e}");
            ExifData::Unreadable
        }
    }
}

fn strip_exif_prefix(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.starts_with(b"Exif\0\0") {
        bytes.drain(..6);
    }
    bytes
}

fn summarize(exif: &Exif) -> ExifSummary {
    let field = |tag: Tag| exif.get_field(tag, In::PRIMARY);
    let ascii = |tag: Tag| field(tag).and_then(ascii_value);
    let uint = |tag: Tag| field(tag).and_then(|f| f.value.get_uint(0));
    let rational = |tag: Tag| field(tag).and_then(first_rational);
    let srational = |tag: Tag| field(tag).and_then(first_srational);

    ExifSummary {
        camera: camera_text(ascii(Tag::Make).as_deref(), ascii(Tag::Model).as_deref()),
        lens: ascii(Tag::LensModel),
        focal_length: rational(Tag::FocalLength)
            .and_then(|r| focal_text(r, uint(Tag::FocalLengthIn35mmFilm))),
        aperture: rational(Tag::FNumber).and_then(aperture_text),
        shutter: rational(Tag::ExposureTime).and_then(shutter_text),
        iso: uint(Tag::PhotographicSensitivity).map(iso_text),
        exposure_bias: srational(Tag::ExposureBiasValue).and_then(ev_text),
        date_taken: date_text(exif),
        orientation: uint(Tag::Orientation).and_then(orientation_text),
        location: location(exif),
        tags: exif
            .fields()
            .map(|f| (tag_name(f), tag_value(f, exif)))
            .collect(),
    }
}

// ---- value access --------------------------------------------------

fn ascii_bytes(f: &Field) -> Option<&[u8]> {
    match &f.value {
        Value::Ascii(v) => v.first().map(|b| b.as_slice()),
        _ => None,
    }
}

/// The first ASCII string of a field, trimmed of blanks and NULs; None
/// when empty.
fn ascii_value(f: &Field) -> Option<String> {
    let bytes = ascii_bytes(f)?;
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn first_rational(f: &Field) -> Option<Rational> {
    match &f.value {
        Value::Rational(v) => v.first().copied(),
        _ => None,
    }
}

fn first_srational(f: &Field) -> Option<SRational> {
    match &f.value {
        Value::SRational(v) => v.first().copied(),
        _ => None,
    }
}

// ---- curated formatters -------------------------------------------

/// `v` with `decimals` places, then without trailing zeros: 2.0 -> "2",
/// 2.80 -> "2.8", 11.00 -> "11".
fn trim_decimal(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Make and model as one line. Makers repeat themselves in the model
/// ("NIKON CORPORATION" / "NIKON D750", "Canon" / "Canon EOS R5"), so
/// when the model already contains the first word of the make, the model
/// alone is shown.
pub(crate) fn camera_text(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let make = make.map(str::trim).filter(|s| !s.is_empty());
    let model = model.map(str::trim).filter(|s| !s.is_empty());
    match (make, model) {
        (None, None) => None,
        (Some(make), None) => Some(make.to_string()),
        (None, Some(model)) => Some(model.to_string()),
        (Some(make), Some(model)) => {
            let first_word = make.split_whitespace().next().unwrap_or(make).to_lowercase();
            if model.to_lowercase().contains(&first_word) {
                Some(model.to_string())
            } else {
                Some(format!("{make} {model}"))
            }
        }
    }
}

/// Exposure time the way cameras print it: fractions below 0.3 s
/// ("1/250 s"), decimals from there up ("0.5 s", "2 s", "30 s").
pub(crate) fn shutter_text(r: Rational) -> Option<String> {
    if r.num == 0 || r.denom == 0 {
        return None;
    }
    let seconds = r.to_f64();
    if seconds >= 0.3 {
        Some(format!("{} s", trim_decimal(seconds, 1)))
    } else {
        Some(format!("1/{} s", (1.0 / seconds).round() as u32))
    }
}

/// The f-number as the camera wrote it: "f/2.8", "f/11", "f/1.78".
pub(crate) fn aperture_text(r: Rational) -> Option<String> {
    if r.num == 0 || r.denom == 0 {
        return None;
    }
    Some(format!("f/{}", trim_decimal(r.to_f64(), 2)))
}

/// "50 mm", or "24 mm (6.86 mm)" when the 35 mm equivalent tag exists and
/// differs. The equivalent comes first because it is the number that
/// compares across cameras and the one Photos shows for a phone; the real
/// length follows in brackets.
pub(crate) fn focal_text(r: Rational, equivalent_35mm: Option<u32>) -> Option<String> {
    if r.num == 0 || r.denom == 0 {
        return None;
    }
    let mm = r.to_f64();
    let real = format!("{} mm", trim_decimal(mm, 2));
    match equivalent_35mm {
        Some(eq) if eq > 0 && (eq as f64 - mm).abs() >= 0.5 => Some(format!("{eq} mm ({real})")),
        _ => Some(real),
    }
}

pub(crate) fn iso_text(iso: u32) -> String {
    format!("ISO {iso}")
}

/// Exposure compensation, signed: "+0.3 EV", "-1.7 EV". None at zero,
/// so the line only appears when the photographer dialled something in.
pub(crate) fn ev_text(r: SRational) -> Option<String> {
    if r.denom == 0 {
        return None;
    }
    let ev = r.to_f64();
    if ev.abs() < 0.05 {
        return None;
    }
    let sign = if ev < 0.0 { '-' } else { '+' };
    Some(format!("{sign}{} EV", trim_decimal(ev.abs(), 1)))
}

/// The orientation tag as the turn that would show the picture upright.
pub(crate) fn orientation_text(v: u32) -> Option<String> {
    let s = match v {
        1 => "Normal",
        2 => "Mirror horizontal",
        3 => "Rotate 180°",
        4 => "Mirror vertical",
        5 => "Mirror horizontal, rotate 270° CW",
        6 => "Rotate 90° CW",
        7 => "Mirror horizontal, rotate 90° CW",
        8 => "Rotate 90° CCW",
        _ => return None,
    };
    Some(s.to_string())
}

/// DateTimeOriginal with OffsetTimeOriginal, else DateTime with
/// OffsetTime. "2026-03-27 14:05:12 +08:00".
fn date_text(exif: &Exif) -> Option<String> {
    let (date_tag, offset_tag) = if exif.get_field(Tag::DateTimeOriginal, In::PRIMARY).is_some() {
        (Tag::DateTimeOriginal, Tag::OffsetTimeOriginal)
    } else {
        (Tag::DateTime, Tag::OffsetTime)
    };
    let raw = exif.get_field(date_tag, In::PRIMARY).and_then(ascii_bytes)?;
    let mut date = exif::DateTime::from_ascii(raw).ok()?;
    if let Some(offset) = exif.get_field(offset_tag, In::PRIMARY).and_then(ascii_bytes) {
        let _ = date.parse_offset(offset);
    }
    Some(datetime_text(&date))
}

pub(crate) fn datetime_text(date: &exif::DateTime) -> String {
    let mut s = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        date.year, date.month, date.day, date.hour, date.minute, date.second
    );
    if let Some(offset) = date.offset {
        let sign = if offset < 0 { '-' } else { '+' };
        let minutes = offset.unsigned_abs();
        let _ = write!(s, " {sign}{:02}:{:02}", minutes / 60, minutes % 60);
    }
    s
}

// ---- GPS ------------------------------------------------------------

fn location(exif: &Exif) -> Option<Location> {
    let latitude = gps_coordinate(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S')?;
    let longitude = gps_coordinate(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W')?;
    let altitude_m = exif
        .get_field(Tag::GPSAltitude, In::PRIMARY)
        .and_then(first_rational)
        .filter(|r| r.denom != 0)
        .map(|r| {
            let below_sea_level = exif
                .get_field(Tag::GPSAltitudeRef, In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
                == Some(1);
            if below_sea_level { -r.to_f64() } else { r.to_f64() }
        });
    Some(Location { latitude, longitude, altitude_m })
}

/// Degrees, minutes, seconds as three rationals plus the hemisphere
/// letter, to signed decimal degrees.
fn gps_coordinate(exif: &Exif, tag: Tag, ref_tag: Tag, negative_ref: u8) -> Option<f64> {
    let dms = match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Rational(v) => v.as_slice(),
        _ => return None,
    };
    let degrees = dms_to_decimal(dms)?;
    let negative = exif
        .get_field(ref_tag, In::PRIMARY)
        .and_then(ascii_bytes)
        .and_then(|b| b.first().copied())
        .is_some_and(|c| c.eq_ignore_ascii_case(&negative_ref));
    Some(if negative { -degrees } else { degrees })
}

pub(crate) fn dms_to_decimal(dms: &[Rational]) -> Option<f64> {
    if dms.len() != 3 || dms.iter().any(|r| r.denom == 0) {
        return None;
    }
    Some(dms[0].to_f64() + dms[1].to_f64() / 60.0 + dms[2].to_f64() / 3600.0)
}

// ---- the All EXIF list ---------------------------------------------

fn tag_name(f: &Field) -> String {
    let mut name = f.tag.to_string();
    match f.ifd_num {
        In::PRIMARY => {}
        In::THUMBNAIL => name.push_str(" (thumbnail)"),
        other => {
            let _ = write!(name, " (IFD{})", other.index());
        }
    }
    name
}

/// kamadak's display text for a field, with its unit, cut at
/// `MAX_VALUE_CHARS`. Binary values over `MAX_BINARY_BYTES` and MakerNote
/// are a byte count.
fn tag_value(f: &Field, exif: &Exif) -> String {
    let binary_len = match &f.value {
        Value::Undefined(v, _) | Value::Byte(v) => Some(v.len()),
        _ => None,
    };
    if let Some(n) = binary_len {
        if f.tag == Tag::MakerNote || n > MAX_BINARY_BYTES {
            return format!("{n} bytes");
        }
    }
    let mut out = Bounded::new(MAX_VALUE_CHARS);
    let _ = write!(out, "{}", f.display_value().with_unit(exif));
    out.finish()
}

/// A `fmt::Write` target that accepts `max` bytes and then returns an
/// error, which makes the formatter stop. Formatting a value of 4000
/// numbers costs as much as formatting 200 characters of it.
struct Bounded {
    buf: String,
    max: usize,
    cut: bool,
}

impl Bounded {
    fn new(max: usize) -> Self {
        Self { buf: String::new(), max, cut: false }
    }

    fn finish(self) -> String {
        let mut text = self.buf;
        if self.cut {
            text.push('…');
        }
        text
    }
}

impl fmt::Write for Bounded {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.buf.len() + s.len() > self.max {
            let mut end = self.max - self.buf.len();
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            self.buf.push_str(&s[..end]);
            self.cut = true;
            return Err(fmt::Error);
        }
        self.buf.push_str(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(num: u32, denom: u32) -> Rational {
        Rational { num, denom }
    }

    fn sr(num: i32, denom: i32) -> SRational {
        SRational { num, denom }
    }

    #[test]
    fn shutter_fractions_below_a_third_of_a_second_decimals_above() {
        assert_eq!(shutter_text(r(1, 250)).unwrap(), "1/250 s");
        assert_eq!(shutter_text(r(10, 2500)).unwrap(), "1/250 s");
        assert_eq!(shutter_text(r(1, 3)).unwrap(), "0.3 s");
        assert_eq!(shutter_text(r(1, 4)).unwrap(), "1/4 s");
        assert_eq!(shutter_text(r(1, 2)).unwrap(), "0.5 s");
        assert_eq!(shutter_text(r(2, 1)).unwrap(), "2 s");
        assert_eq!(shutter_text(r(13, 10)).unwrap(), "1.3 s");
        assert_eq!(shutter_text(r(30, 1)).unwrap(), "30 s");
        assert_eq!(shutter_text(r(0, 1)), None);
        assert_eq!(shutter_text(r(1, 0)), None);
    }

    #[test]
    fn aperture_drops_trailing_zero() {
        assert_eq!(aperture_text(r(28, 10)).unwrap(), "f/2.8");
        assert_eq!(aperture_text(r(2, 1)).unwrap(), "f/2");
        assert_eq!(aperture_text(r(56, 10)).unwrap(), "f/5.6");
        assert_eq!(aperture_text(r(11, 1)).unwrap(), "f/11");
        assert_eq!(aperture_text(r(95, 100)).unwrap(), "f/0.95");
        assert_eq!(aperture_text(r(89, 50)).unwrap(), "f/1.78");
        assert_eq!(aperture_text(r(0, 10)), None);
    }

    #[test]
    fn focal_length_with_and_without_equivalent() {
        assert_eq!(focal_text(r(50, 1), None).unwrap(), "50 mm");
        assert_eq!(focal_text(r(50, 1), Some(75)).unwrap(), "75 mm (50 mm)");
        assert_eq!(focal_text(r(50, 1), Some(50)).unwrap(), "50 mm");
        assert_eq!(focal_text(r(50, 1), Some(0)).unwrap(), "50 mm");
        assert_eq!(focal_text(r(42, 10), Some(26)).unwrap(), "26 mm (4.2 mm)");
        assert_eq!(focal_text(r(343, 50), Some(24)).unwrap(), "24 mm (6.86 mm)");
        assert_eq!(focal_text(r(0, 1), None), None);
    }

    #[test]
    fn exposure_bias_is_signed_and_hidden_at_zero() {
        assert_eq!(ev_text(sr(0, 1)), None);
        assert_eq!(ev_text(sr(0, 3)), None);
        assert_eq!(ev_text(sr(1, 3)).unwrap(), "+0.3 EV");
        assert_eq!(ev_text(sr(-5, 3)).unwrap(), "-1.7 EV");
        assert_eq!(ev_text(sr(1, 1)).unwrap(), "+1 EV");
        assert_eq!(ev_text(sr(-1, 2)).unwrap(), "-0.5 EV");
        assert_eq!(ev_text(sr(1, 0)), None);
    }

    #[test]
    fn camera_dedupes_make_repeated_in_model() {
        assert_eq!(camera_text(Some("Canon"), Some("Canon EOS R5")).unwrap(), "Canon EOS R5");
        assert_eq!(
            camera_text(Some("NIKON CORPORATION"), Some("NIKON D750")).unwrap(),
            "NIKON D750"
        );
        assert_eq!(camera_text(Some("SONY"), Some("ILCE-7M4")).unwrap(), "SONY ILCE-7M4");
        assert_eq!(camera_text(Some("Apple"), Some("iPhone 15")).unwrap(), "Apple iPhone 15");
        assert_eq!(camera_text(Some("SONY  "), None).unwrap(), "SONY");
        assert_eq!(camera_text(None, Some(" X100V")).unwrap(), "X100V");
        assert_eq!(camera_text(Some(""), Some("")), None);
        assert_eq!(camera_text(None, None), None);
    }

    #[test]
    fn orientation_as_a_turn() {
        assert_eq!(orientation_text(1).unwrap(), "Normal");
        assert_eq!(orientation_text(6).unwrap(), "Rotate 90° CW");
        assert_eq!(orientation_text(8).unwrap(), "Rotate 90° CCW");
        assert_eq!(orientation_text(3).unwrap(), "Rotate 180°");
        assert_eq!(orientation_text(9), None);
    }

    #[test]
    fn dms_to_decimal_degrees() {
        // 8° 24' 34.2"
        let d = dms_to_decimal(&[r(8, 1), r(24, 1), r(342, 10)]).unwrap();
        assert!((d - 8.4095).abs() < 1e-9, "{d}");
        assert_eq!(dms_to_decimal(&[r(8, 1), r(24, 1)]), None);
        assert_eq!(dms_to_decimal(&[r(8, 1), r(24, 0), r(0, 1)]), None);
    }

    #[test]
    fn datetime_with_offset() {
        let mut date = exif::DateTime::from_ascii(b"2026:03:27 14:05:12").unwrap();
        assert_eq!(datetime_text(&date), "2026-03-27 14:05:12");
        date.parse_offset(b"+08:00").unwrap();
        assert_eq!(datetime_text(&date), "2026-03-27 14:05:12 +08:00");
        date.parse_offset(b"-03:30").unwrap();
        assert_eq!(datetime_text(&date), "2026-03-27 14:05:12 -03:30");
    }

    #[test]
    fn bounded_writer_stops_the_formatter() {
        let mut out = Bounded::new(10);
        let result = write!(out, "abcdefghijklmnop");
        assert!(result.is_err());
        assert_eq!(out.finish(), "abcdefghij…");

        let mut out = Bounded::new(10);
        write!(out, "short").unwrap();
        assert_eq!(out.finish(), "short");

        // Cuts on a character boundary.
        let mut out = Bounded::new(4);
        let _ = write!(out, "ééé");
        assert_eq!(out.finish(), "éé…");
    }

    // ---- whole blocks through kamadak's writer ----------------------

    fn field(tag: Tag, value: Value) -> Field {
        Field { tag, ifd_num: In::PRIMARY, value }
    }

    fn ascii(tag: Tag, s: &str) -> Field {
        field(tag, Value::Ascii(vec![s.as_bytes().to_vec()]))
    }

    fn short(tag: Tag, v: u16) -> Field {
        field(tag, Value::Short(vec![v]))
    }

    fn rational(tag: Tag, num: u32, denom: u32) -> Field {
        field(tag, Value::Rational(vec![r(num, denom)]))
    }

    fn exif_block(fields: &[Field]) -> Vec<u8> {
        let mut writer = exif::experimental::Writer::new();
        for f in fields {
            writer.push_field(f);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        writer.write(&mut buf, false).unwrap();
        buf.into_inner()
    }

    fn summary(fields: &[Field]) -> ExifSummary {
        match parse_exif(exif_block(fields)) {
            ExifData::Present(s) => *s,
            other => panic!("expected parsed EXIF, got {other:?}"),
        }
    }

    #[test]
    fn camera_section_from_a_block() {
        let s = summary(&[
            ascii(Tag::Make, "NIKON CORPORATION"),
            ascii(Tag::Model, "NIKON D750"),
            ascii(Tag::LensModel, "24.0-70.0 mm f/2.8"),
            rational(Tag::ExposureTime, 1, 250),
            rational(Tag::FNumber, 28, 10),
            short(Tag::PhotographicSensitivity, 400),
            rational(Tag::FocalLength, 50, 1),
            short(Tag::FocalLengthIn35mmFilm, 50),
            field(Tag::ExposureBiasValue, Value::SRational(vec![sr(1, 3)])),
            short(Tag::ExposureProgram, 3),
            short(Tag::MeteringMode, 5),
            short(Tag::WhiteBalance, 0),
            short(Tag::Flash, 0x10),
            short(Tag::Orientation, 6),
            ascii(Tag::DateTimeOriginal, "2026:03:27 14:05:12"),
            ascii(Tag::OffsetTimeOriginal, "+08:00"),
        ]);
        assert_eq!(s.camera.as_deref(), Some("NIKON D750"));
        assert_eq!(s.lens.as_deref(), Some("24.0-70.0 mm f/2.8"));
        assert_eq!(s.shutter.as_deref(), Some("1/250 s"));
        assert_eq!(s.aperture.as_deref(), Some("f/2.8"));
        assert_eq!(s.iso.as_deref(), Some("ISO 400"));
        assert_eq!(s.focal_length.as_deref(), Some("50 mm"));
        assert_eq!(s.exposure_bias.as_deref(), Some("+0.3 EV"));
        // Program, metering, white balance and flash are not curated;
        // they stay reachable in the tag list.
        assert!(s.tags.iter().any(|(n, v)| n == "ExposureProgram" && v == "aperture priority"));
        assert!(s.tags.iter().any(|(n, _)| n == "MeteringMode"));
        assert!(s.tags.iter().any(|(n, _)| n == "WhiteBalance"));
        assert!(s.tags.iter().any(|(n, _)| n == "Flash"));
        assert_eq!(s.orientation.as_deref(), Some("Rotate 90° CW"));
        assert_eq!(s.date_taken.as_deref(), Some("2026-03-27 14:05:12 +08:00"));
        assert_eq!(s.location, None);
        assert!(s.tags.iter().any(|(n, v)| n == "Model" && v == "\"NIKON D750\""));
        assert!(s.tags.iter().any(|(n, v)| n == "FNumber" && v == "f/2.8"));
    }

    #[test]
    fn date_falls_back_to_the_tiff_datetime() {
        let s = summary(&[
            ascii(Tag::DateTime, "2025:12:31 23:59:59"),
            ascii(Tag::OffsetTime, "-05:00"),
        ]);
        assert_eq!(s.date_taken.as_deref(), Some("2025-12-31 23:59:59 -05:00"));
        assert_eq!(s.camera, None);
    }

    #[test]
    fn location_from_gps_tags() {
        let s = summary(&[
            ascii(Tag::GPSLatitudeRef, "S"),
            field(Tag::GPSLatitude, Value::Rational(vec![r(8, 1), r(24, 1), r(342, 10)])),
            ascii(Tag::GPSLongitudeRef, "E"),
            field(Tag::GPSLongitude, Value::Rational(vec![r(115, 1), r(11, 1), r(2004, 100)])),
            field(Tag::GPSAltitudeRef, Value::Byte(vec![1])),
            rational(Tag::GPSAltitude, 123, 10),
        ]);
        let loc = s.location.unwrap();
        assert!((loc.latitude + 8.4095).abs() < 1e-9, "{}", loc.latitude);
        assert!((loc.longitude - 115.1889).abs() < 1e-9, "{}", loc.longitude);
        assert_eq!(loc.altitude_m, Some(-12.3));
        assert_eq!(loc.text(), "-8.4095, 115.1889");
        assert_eq!(loc.altitude_text().as_deref(), Some("-12 m"));
        assert!(loc.map_url().starts_with("https://www.openstreetmap.org/?mlat=-8.409500&mlon=115.188900"));
    }

    #[test]
    fn location_needs_both_coordinates() {
        let s = summary(&[
            ascii(Tag::GPSLatitudeRef, "N"),
            field(Tag::GPSLatitude, Value::Rational(vec![r(8, 1), r(24, 1), r(342, 10)])),
        ]);
        assert_eq!(s.location, None);
    }

    #[test]
    fn binary_values_are_a_byte_count_and_long_values_are_cut() {
        let long_text = "x".repeat(500);
        let s = summary(&[
            field(Tag::MakerNote, Value::Undefined(vec![0u8; 10], 0)),
            field(Tag::UserComment, Value::Undefined(vec![0u8; 300], 0)),
            field(Tag::ExifVersion, Value::Undefined(b"0231".to_vec(), 0)),
            ascii(Tag::ImageDescription, &long_text),
            field(Tag::ISOSpeed, Value::Long((0..4000).collect())),
        ]);
        let value = |name: &str| {
            s.tags.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap()
        };
        assert_eq!(value("MakerNote"), "10 bytes");
        assert_eq!(value("UserComment"), "300 bytes");
        assert_eq!(value("ExifVersion"), "2.31");
        let description = value("ImageDescription");
        assert!(description.ends_with('…'), "{description}");
        assert!(description.chars().count() <= MAX_VALUE_CHARS + 1);
        let iso = value("ISOSpeed");
        assert!(iso.ends_with('…'));
        assert!(iso.len() <= MAX_VALUE_CHARS + '…'.len_utf8());
    }

    #[test]
    fn prefix_is_stripped_and_garbage_is_unreadable() {
        let block = exif_block(&[ascii(Tag::Make, "Canon")]);
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend_from_slice(&block);
        match parse_exif(prefixed) {
            ExifData::Present(s) => assert_eq!(s.camera.as_deref(), Some("Canon")),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse_exif(b"not exif at all".to_vec()), ExifData::Unreadable);
        assert_eq!(parse_exif(Vec::new()), ExifData::None);
        assert_eq!(parse_exif(b"Exif\0\0".to_vec()), ExifData::None);
    }
}
