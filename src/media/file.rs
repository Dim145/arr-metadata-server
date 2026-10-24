//! What a file is, from its bytes: never from its name or from what the
//! provider said it was. How the copies made of a picture kept are named,
//! and how a picture is read to make them.

use std::io::Cursor;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

use crate::db::repo::asset::{Kind, Thumb};

/// The most a picture may weigh. A poster from Fanart.tv runs to a few
/// megabytes; twenty-five is a bound, not a budget.
pub const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
/// A theme is a song: a Fan-Kai's runs to a few minutes of MP3.
pub const MAX_AUDIO_BYTES: u64 = 60 * 1024 * 1024;

/// The largest picture kept at all: a side no screen draws, or a canvas
/// whose pixels alone would take gigabytes, is not artwork but a trap — for
/// this server's decoder, and for every client it would be handed to.
pub const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 120_000_000;

/// The largest picture this server decodes to make its copies: an 8K
/// backdrop is a third of it. A larger one is kept and served as it is,
/// and the interface is given the original — never a canvas a header
/// promises and the memory cannot hold.
pub const DECODE_MAX_PIXELS: u64 = 100_000_000;
/// What a decoder may set aside, its pixels and its own working buffers
/// alike: a progressive JPEG's coefficients are held whole beside them.
const DECODE_MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// The kinds of file kept, by the extension their key carries.
///
/// No SVG, though a logo could be one: a document that can carry a script,
/// served from this server's origin, would run it as this server. Nothing a
/// poster needs.
const EXTENSIONS: &[(&str, &str, Kind)] = &[
    ("jpg", "image/jpeg", Kind::Image),
    ("png", "image/png", Kind::Image),
    ("webp", "image/webp", Kind::Image),
    ("gif", "image/gif", Kind::Image),
    ("avif", "image/avif", Kind::Image),
    ("mp3", "audio/mpeg", Kind::Audio),
    ("ogg", "audio/ogg", Kind::Audio),
    ("flac", "audio/flac", Kind::Audio),
    ("m4a", "audio/mp4", Kind::Audio),
    ("wav", "audio/wav", Kind::Audio),
    ("aac", "audio/aac", Kind::Audio),
];

/// What the bytes turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspected {
    pub content_type: &'static str,
    pub ext: &'static str,
    pub kind: Kind,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// What the bytes are, and that they are what was wanted.
///
/// The sniffing and the reading of a header are done here, on bytes
/// somebody else chose: a parser that panics on them is a file refused, not
/// a worker or a request brought down.
pub fn inspect(bytes: &[u8], wanted: Kind) -> Result<Inspected> {
    std::panic::catch_unwind(|| inspect_unguarded(bytes, wanted))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("a file this server could not read")))
}

fn inspect_unguarded(bytes: &[u8], wanted: Kind) -> Result<Inspected> {
    let Some(found) = infer::get(bytes) else {
        bail!("not a picture or a sound this server keeps");
    };

    let ext = match found.mime_type() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/x-flac" | "audio/flac" => "flac",
        // An M4A is an MP4 box: the sniffer calls some of them video.
        "audio/m4a" | "audio/mp4" | "video/mp4" if wanted == Kind::Audio => "m4a",
        "audio/x-wav" | "audio/wav" => "wav",
        "audio/aac" => "aac",
        other => bail!("{other} is not a picture or a sound this server keeps"),
    };
    let (_, content_type, kind) = EXTENSIONS
        .iter()
        .find(|(e, _, _)| *e == ext)
        .copied()
        .expect("every extension above is listed");
    if kind != wanted {
        bail!("a {} where {} was wanted", kind.as_str(), wanted.as_str());
    }

    let (width, height) = match kind {
        Kind::Image => match imagesize::blob_size(bytes) {
            Ok(size) => (
                u32::try_from(size.width).ok(),
                u32::try_from(size.height).ok(),
            ),
            Err(_) => (None, None),
        },
        Kind::Audio => (None, None),
    };
    if let (Some(w), Some(h)) = (width, height)
        && (w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS)
    {
        bail!("a picture of {w}×{h}, larger than any this server keeps");
    }

    Ok(Inspected {
        content_type,
        ext,
        kind,
        width,
        height,
    })
}

/// The key the bytes are filed under: their hash, and what they are.
pub fn key_for(bytes: &[u8], ext: &str) -> (String, String) {
    let sha = hex(&Sha256::digest(bytes));
    (format!("{sha}.{ext}"), sha)
}

/// What a key names: a file's own bytes, or a copy made of them — named by
/// the same hash, a mark after it, and the copy's own extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Made {
    /// The bytes themselves: `<hash>.jpg`.
    Own,
    /// The thumbnail earlier versions made of a picture: `<hash>-t.jpg`.
    /// Still served; no new one is made.
    Thumb,
    /// A copy at a width, for the interface: `<hash>-480.webp`.
    Width(u32),
    /// A square cut of a background, for Plex: `<hash>-square.jpg`.
    Square,
}

/// A key, read: the hash, what it names, and its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key<'a> {
    pub stem: &'a str,
    pub made: Made,
    pub ext: &'a str,
}

/// A key read for what it names, when it is one this server files under:
/// sixty-four hex digits; nothing more, `-t`, a width or `-square`; and one
/// of the extensions kept — a WebP or an AVIF for a width, a JPEG for a
/// square. Nothing else reaches the store from a request, so nothing else
/// can name a path in it.
pub fn parse_key(name: &str) -> Option<Key<'_>> {
    let (head, ext) = name.rsplit_once('.')?;
    if !EXTENSIONS.iter().any(|(e, _, _)| *e == ext) {
        return None;
    }
    let (stem, made) = match head.split_once('-') {
        None => (head, Made::Own),
        Some((stem, "t")) => (stem, Made::Thumb),
        Some((stem, "square")) => (stem, Made::Square),
        Some((stem, digits)) => (stem, Made::Width(width_in(digits)?)),
    };
    let fits = match made {
        Made::Own | Made::Thumb => true,
        Made::Width(_) => matches!(ext, "webp" | "avif"),
        Made::Square => ext == "jpg",
    };
    let hashed = stem.len() == 64
        && stem
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    (fits && hashed).then_some(Key { stem, made, ext })
}

/// A width as a key writes it: digits, no leading nought, no wider than a
/// picture this server keeps.
fn width_in(digits: &str) -> Option<u32> {
    if digits.is_empty()
        || digits.len() > 5
        || digits.starts_with('0')
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok().filter(|w| (1..=MAX_SIDE).contains(w))
}

/// The key of a key's thumbnail, when one was made: the same hash, `-t`,
/// and the thumbnail's own extension.
pub fn thumb_key(key: &str, thumb: Thumb) -> Option<String> {
    let stem = key.split('.').next().unwrap_or(key);
    thumb.ext().map(|ext| format!("{stem}-t.{ext}"))
}

/// The key a copy of the bytes of `stem` is filed under.
pub fn made_key(stem: &str, made: Made, ext: &str) -> String {
    match made {
        Made::Own => format!("{stem}.{ext}"),
        Made::Thumb => format!("{stem}-t.{ext}"),
        Made::Width(width) => format!("{stem}-{width}.{ext}"),
        Made::Square => format!("{stem}-square.{ext}"),
    }
}

/// Whether a name is a key this server files under: see [`parse_key`].
pub fn valid_key(name: &str) -> bool {
    parse_key(name).is_some()
}

/// The content type a key's bytes are served as: the one its extension
/// stands for, and nothing else — not what the bytes were said to be by
/// whoever told this server of them.
pub fn content_type_of(key: &str) -> Option<&'static str> {
    let (_, ext) = key.rsplit_once('.')?;
    EXTENSIONS
        .iter()
        .find(|(e, _, _)| *e == ext)
        .map(|(_, content_type, _)| *content_type)
}

/// The hash a key files its bytes under, whether it is the bytes' key or
/// a copy's.
pub fn stem_of(key: &str) -> &str {
    if let Some(parsed) = parse_key(key) {
        return parsed.stem;
    }
    let stem = key.split('.').next().unwrap_or(key);
    stem.split('-').next().unwrap_or(stem)
}

/// Every key the bytes of a hash could be filed under: one per extension
/// kept.
pub fn keys_of_stem(stem: &str) -> Vec<String> {
    EXTENSIONS
        .iter()
        .map(|(ext, _, _)| format!("{stem}.{ext}"))
        .collect()
}

/// Whether a key names a thumbnail of earlier versions'.
pub fn is_thumb(key: &str) -> bool {
    parse_key(key).is_some_and(|parsed| parsed.made == Made::Thumb)
}

/// Whether a key names a copy made of other bytes — a thumbnail, a width,
/// a square — rather than bytes of their own.
pub fn is_made(key: &str) -> bool {
    parse_key(key).is_some_and(|parsed| parsed.made != Made::Own)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A picture's pixels, read within bounds — or why not.
///
/// The header is read first, by the decoder that would draw it: a picture
/// whose side is past [`MAX_SIDE`] or whose pixels are past
/// [`DECODE_MAX_PIXELS`] is refused before a byte of it is decoded, and the
/// buffer the pixels need is reserved against [`DECODE_MAX_ALLOC`] before it
/// is allocated. Turned the way its EXIF orientation says, as a camera's
/// photograph is shown. Pure Rust: no C decoder ever reads bytes somebody
/// else chose. The caller runs it off the server's threads and catches what
/// a decoder may still panic on.
pub fn decode(bytes: &[u8]) -> Result<image::DynamicImage> {
    use image::ImageDecoder as _;

    let reader = || {
        image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .context("the picture could not be read")
    };
    let (width, height) = reader()?
        .into_dimensions()
        .context("the picture's header could not be read")?;
    if width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) > DECODE_MAX_PIXELS
    {
        bail!("a picture of {width}×{height} is larger than this server decodes");
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);

    let mut reader = reader()?;
    reader.limits(limits.clone());
    let mut decoder = reader
        .into_decoder()
        .context("the picture could not be read")?;
    // What the pixels take is reserved against the bound first, and the
    // decoder is left what remains of it for its own buffers.
    limits
        .reserve(decoder.total_bytes())
        .context("the picture's pixels would take more than this server sets aside")?;
    decoder
        .set_limits(limits)
        .context("the picture is larger than this server decodes")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture =
        image::DynamicImage::from_decoder(decoder).context("the picture could not be decoded")?;
    picture.apply_orientation(orientation);
    Ok(picture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 30]))
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn a_picture_is_known_by_its_bytes() {
        let bytes = png(20, 30);
        let found = inspect(&bytes, Kind::Image).unwrap();
        assert_eq!(found.ext, "png");
        assert_eq!(found.content_type, "image/png");
        assert_eq!((found.width, found.height), (Some(20), Some(30)));

        assert!(
            inspect(&bytes, Kind::Audio).is_err(),
            "a picture is not a sound"
        );
        assert!(inspect(b"<svg xmlns='http://www.w3.org/2000/svg'/>", Kind::Image).is_err());
        assert!(inspect(b"hello", Kind::Image).is_err());
    }

    #[test]
    fn keys_are_hashes_and_nothing_else_passes_for_one() {
        let (key, sha) = key_for(b"abc", "jpg");
        assert_eq!(
            sha,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(key, format!("{sha}.jpg"));
        assert!(valid_key(&key));
        let thumb = thumb_key(&key, Thumb::Jpeg).unwrap();
        assert!(valid_key(&thumb));
        assert!(is_thumb(&thumb) && !is_thumb(&key));
        assert_eq!(
            thumb_key(&key, Thumb::Png).as_deref(),
            Some(&*format!("{sha}-t.png"))
        );
        assert!(thumb_key(&key, Thumb::None).is_none());

        for bad in [
            "../etc/passwd",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.svg",
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD.jpg",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015a.jpg",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "",
        ] {
            assert!(!valid_key(bad), "{bad}");
        }
    }

    /// A copy is named by its picture's hash, what it is, and its own
    /// extension — and only the copies this server makes pass for one.
    #[test]
    fn a_copys_key_names_its_picture_and_its_width() {
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for (key, made) in [
            (format!("{sha}-480.webp"), Made::Width(480)),
            (format!("{sha}-1920.webp"), Made::Width(1920)),
            (format!("{sha}-960.avif"), Made::Width(960)),
            (format!("{sha}-square.jpg"), Made::Square),
            (format!("{sha}-t.jpg"), Made::Thumb),
            (format!("{sha}.png"), Made::Own),
        ] {
            let parsed = parse_key(&key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((parsed.stem, parsed.made), (sha, made), "{key}");
            assert_eq!(made_key(sha, made, parsed.ext), key);
            assert_eq!(stem_of(&key), sha);
            assert_eq!(is_made(&key), made != Made::Own);
        }
        for bad in [
            format!("{sha}-480.jpg"),
            format!("{sha}-0480.webp"),
            format!("{sha}-0.webp"),
            format!("{sha}-99999.webp"),
            format!("{sha}--480.webp"),
            format!("{sha}-480x480.webp"),
            format!("{sha}-square.webp"),
            format!("{sha}-+480.webp"),
            format!("{sha}-480.webp.webp"),
            "480.webp".to_string(),
        ] {
            assert!(!valid_key(&bad), "{bad}");
        }
    }

    /// A PNG's signature and header, promising a canvas of this size, and
    /// nothing after: enough for the sniffer and the header reader.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        out
    }

    #[test]
    fn a_canvas_no_screen_draws_is_not_kept() {
        assert!(inspect(&png_header(4000, 6000), Kind::Image).is_ok());
        for (width, height) in [(20_000, 100), (100, 40_000), (12_000, 12_000)] {
            let refused = inspect(&png_header(width, height), Kind::Image).unwrap_err();
            assert!(
                refused.to_string().contains("larger than any"),
                "{width}×{height}: {refused}"
            );
        }
    }

    #[test]
    fn a_key_is_served_as_its_extension_says() {
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(content_type_of(&format!("{sha}.jpg")), Some("image/jpeg"));
        assert_eq!(content_type_of(&format!("{sha}-t.png")), Some("image/png"));
        assert_eq!(
            content_type_of(&format!("{sha}-480.webp")),
            Some("image/webp")
        );
        assert_eq!(
            content_type_of(&format!("{sha}-960.avif")),
            Some("image/avif")
        );
        assert_eq!(content_type_of(&format!("{sha}.m4a")), Some("audio/mp4"));
        assert_eq!(content_type_of(&format!("{sha}.html")), None);
        assert_eq!(content_type_of(sha), None);

        assert_eq!(stem_of(&format!("{sha}.jpg")), sha);
        assert_eq!(stem_of(&format!("{sha}-t.jpg")), sha);
        let keys = keys_of_stem(sha);
        assert!(keys.contains(&format!("{sha}.webp")) && keys.contains(&format!("{sha}.mp3")));
    }

    /// The same header as a decoder reads it — its checksums right, a PNG
    /// chunk ending with the CRC-32 of its type and its data — then the
    /// start of the pixels, empty: as far as a decoder reads before it says
    /// what the picture is.
    fn checked_png_header(width: u32, height: u32, colour: u8) -> Vec<u8> {
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            let mut crc = !0u32;
            for byte in kind.iter().chain(data) {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&(!crc).to_be_bytes());
        }
        let mut header = Vec::new();
        header.extend_from_slice(&width.to_be_bytes());
        header.extend_from_slice(&height.to_be_bytes());
        header.extend_from_slice(&[8, colour, 0, 0, 0]);
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, b"IHDR", &header);
        chunk(&mut out, b"IDAT", &[]);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// A header that promises a canvas larger than the decoder is let set
    /// aside is refused before the pixels are asked for: the bytes are a
    /// few dozen, the canvas would be hundreds of megabytes.
    #[test]
    fn a_decompression_bomb_is_refused_before_it_is_decoded() {
        // Within what is kept, past what is decoded: 10 000 × 10 001.
        let refused = decode(&checked_png_header(10_000, 10_001, 2)).unwrap_err();
        assert!(
            format!("{refused:#}").contains("larger than this server decodes"),
            "{refused:#}"
        );
        // Under the count of pixels, past what may be set aside for them:
        // 9 000 × 9 000 of RGBA is 324 megabytes.
        let refused = decode(&checked_png_header(9_000, 9_000, 6)).unwrap_err();
        assert!(format!("{refused:#}").contains("sets aside"), "{refused:#}");
        // And a real picture is read.
        let picture = decode(&png(30, 20)).expect("decoded");
        assert_eq!((picture.width(), picture.height()), (30, 20));
        assert!(decode(b"not a picture at all").is_err());
    }
}
