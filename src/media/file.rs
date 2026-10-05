//! What a file is, from its bytes: never from its name or from what the
//! provider said it was. And the thumbnail made of a picture kept.

use std::io::Cursor;

use anyhow::{Result, bail};
use sha2::{Digest as _, Sha256};

use crate::db::repo::asset::{Kind, Thumb};

/// The most a picture may weigh. A poster from Fanart.tv runs to a few
/// megabytes; twenty-five is a bound, not a budget.
pub const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
/// A theme is a song: a Fan-Kai's runs to a few minutes of MP3.
pub const MAX_AUDIO_BYTES: u64 = 60 * 1024 * 1024;

/// Thumbnails: as wide as a card is drawn on a dense screen, and no wider.
const THUMB_PORTRAIT_WIDTH: u32 = 480;
const THUMB_LANDSCAPE_WIDTH: u32 = 960;

/// The largest picture kept at all: a side no screen draws, or a canvas
/// whose pixels alone would take gigabytes, is not artwork but a trap — for
/// this server's decoder, and for every client it would be handed to.
const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 120_000_000;

/// The largest picture a thumbnail is made of: an 8K backdrop, a poster from
/// a good camera. Decoding one takes a few hundred megabytes — a progressive
/// JPEG's coefficients are held whole beside its pixels — so a larger one is
/// served as it is rather than decoded.
const THUMB_MAX_PIXELS: u64 = 36_000_000;
/// What a decoder may set aside for the pixels of one.
const THUMB_MAX_ALLOC: u64 = 256 * 1024 * 1024;

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

/// The key of a key's thumbnail, when one was made: the same hash, `-t`,
/// and the thumbnail's own extension.
pub fn thumb_key(key: &str, thumb: Thumb) -> Option<String> {
    let stem = key.split('.').next().unwrap_or(key);
    thumb.ext().map(|ext| format!("{stem}-t.{ext}"))
}

/// Whether a name is a key this server files under: sixty-four hex digits,
/// `-t` for a thumbnail, and one of the extensions kept. Nothing else reaches
/// the store from a request, so nothing else can name a path in it.
pub fn valid_key(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let stem = stem.strip_suffix("-t").unwrap_or(stem);
    stem.len() == 64
        && stem
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        && EXTENSIONS.iter().any(|(e, _, _)| *e == ext)
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
/// their thumbnail's.
pub fn stem_of(key: &str) -> &str {
    let stem = key.split('.').next().unwrap_or(key);
    stem.strip_suffix("-t").unwrap_or(stem)
}

/// Every key the bytes of a hash could be filed under: one per extension
/// kept.
pub fn keys_of_stem(stem: &str) -> Vec<String> {
    EXTENSIONS
        .iter()
        .map(|(ext, _, _)| format!("{stem}.{ext}"))
        .collect()
}

/// Whether a key names a thumbnail.
pub fn is_thumb(key: &str) -> bool {
    key.split('.')
        .next()
        .is_some_and(|stem| stem.ends_with("-t"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A small copy of the picture, no wider than a card is drawn, when the
/// picture is wider than that: a JPEG, or a PNG where the picture is
/// transparent somewhere — a logo laid over a backdrop. `None` for one
/// already small, and for one the decoders here cannot read: the picture is
/// kept as it is, and served whole.
///
/// Decoded within limits: a picture's header can promise a canvas that
/// would take every byte of memory to draw, so the header is read first, by
/// the decoder that would draw it, and a picture past
/// [`THUMB_MAX_PIXELS`] is not decoded at all.
pub fn thumbnail(bytes: &[u8]) -> Option<(Vec<u8>, Thumb)> {
    thumbnail_within(bytes, THUMB_MAX_PIXELS)
}

fn thumbnail_within(bytes: &[u8], max_pixels: u64) -> Option<(Vec<u8>, Thumb)> {
    let reader = || {
        image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()
    };
    let (width, height) = reader()?.into_dimensions().ok()?;
    if u64::from(width) * u64::from(height) > max_pixels {
        tracing::debug!(
            width,
            height,
            "no thumbnail: the picture is too large to decode"
        );
        return None;
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(THUMB_MAX_ALLOC);

    let mut reader = reader()?;
    reader.limits(limits);
    let picture = match reader.decode() {
        Ok(picture) => picture,
        Err(e) => {
            tracing::debug!(error = %e, "no thumbnail: the picture could not be decoded");
            return None;
        }
    };

    let (width, height) = (picture.width(), picture.height());
    let target = if width > height {
        THUMB_LANDSCAPE_WIDTH
    } else {
        THUMB_PORTRAIT_WIDTH
    };
    if width <= target {
        return None;
    }

    let scaled_height = u32::try_from(u64::from(height) * u64::from(target) / u64::from(width))
        .unwrap_or(1)
        .max(1);
    let small = picture.resize_exact(target, scaled_height, image::imageops::FilterType::Triangle);

    // A picture with an alpha channel that is opaque everywhere is a JPEG
    // like any other; only real transparency needs keeping.
    let transparent = picture.color().has_alpha() && {
        let rgba = small.to_rgba8();
        rgba.pixels().any(|p| p[3] < 255)
    };

    let mut out = Vec::new();
    let encoded = if transparent {
        small
            .to_rgba8()
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .map(|()| Thumb::Png)
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82)
            .encode_image(&small.to_rgb8())
            .map(|()| Thumb::Jpeg)
    };
    match encoded {
        Ok(thumb) => Some((out, thumb)),
        Err(e) => {
            tracing::debug!(error = %e, "no thumbnail: it could not be encoded");
            None
        }
    }
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
        assert_eq!(content_type_of(&format!("{sha}.m4a")), Some("audio/mp4"));
        assert_eq!(content_type_of(&format!("{sha}.html")), None);
        assert_eq!(content_type_of(sha), None);

        assert_eq!(stem_of(&format!("{sha}.jpg")), sha);
        assert_eq!(stem_of(&format!("{sha}-t.jpg")), sha);
        let keys = keys_of_stem(sha);
        assert!(keys.contains(&format!("{sha}.webp")) && keys.contains(&format!("{sha}.mp3")));
    }

    #[test]
    fn a_picture_too_large_to_decode_is_served_without_a_thumbnail() {
        let wide = png(1200, 1800);
        assert!(thumbnail_within(&wide, 1_000_000).is_none());
        assert!(thumbnail_within(&wide, 2_160_000).is_some());
    }

    #[test]
    fn a_wide_picture_gets_a_thumbnail_and_a_small_one_does_not() {
        let wide = png(1200, 1800);
        let (thumb, kind) = thumbnail(&wide).expect("a thumbnail");
        let size = imagesize::blob_size(&thumb).unwrap();
        assert_eq!((size.width, size.height), (480, 720));
        assert_eq!(inspect(&thumb, Kind::Image).unwrap().ext, "jpg");
        assert_eq!(kind, Thumb::Jpeg);

        assert!(thumbnail(&png(300, 450)).is_none());
        assert!(thumbnail(b"not a picture at all").is_none());

        let landscape = png(1920, 1080);
        let (thumb, _) = thumbnail(&landscape).expect("a thumbnail");
        assert_eq!(imagesize::blob_size(&thumb).unwrap().width, 960);

        // Transparent somewhere: a PNG, with its transparency.
        let mut out = Vec::new();
        let mut logo = image::RgbaImage::from_pixel(1000, 400, image::Rgba([255, 255, 255, 255]));
        logo.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        logo.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        let (thumb, kind) = thumbnail(&out).expect("a thumbnail");
        assert_eq!(kind, Thumb::Png);
        assert_eq!(inspect(&thumb, Kind::Image).unwrap().ext, "png");

        // Opaque everywhere, alpha channel or not: a JPEG.
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(1000, 400, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(thumbnail(&out).expect("a thumbnail").1, Thumb::Jpeg);
    }
}
