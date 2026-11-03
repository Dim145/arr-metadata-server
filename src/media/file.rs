//! What a file is, from its bytes: never from its name or from what the
//! provider said it was. How the copies made of a picture kept are named,
//! and how a picture is read to make them.

use std::{io::Cursor, sync::LazyLock};

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
/// What the pictures being decoded may hold in memory together: each one's
/// worst case, pixels and copies and its decoder's own buffers alike, counted
/// from its header by [`working_set`] before a byte of it is decoded. A
/// picture whose worst case alone is more than this is not decoded; the rest
/// are held to it together by `variants::Budget`, so that a large one waits
/// for the others and small ones go side by side.
pub const DECODE_BUDGET: u64 = 256 * 1024 * 1024;
/// How many bytes of a file are enough to tell what it is.
pub const HEAD_BYTES: usize = 512;

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

/// What a file's first bytes say it is, as one of the kinds kept: its
/// extension, its content type and whether it is a picture or a sound.
/// `wanted` is what it is to be, where that is known: an MP4 box is a sound
/// unless a picture was asked for.
fn sniff(bytes: &[u8], wanted: Option<Kind>) -> Result<(&'static str, &'static str, Kind)> {
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
        "audio/m4a" | "audio/mp4" | "video/mp4" if wanted != Some(Kind::Image) => "m4a",
        "audio/x-wav" | "audio/wav" => "wav",
        "audio/aac" => "aac",
        other => bail!("{other} is not a picture or a sound this server keeps"),
    };
    let (_, content_type, kind) = EXTENSIONS
        .iter()
        .find(|(e, _, _)| *e == ext)
        .copied()
        .expect("every extension above is listed");
    Ok((ext, content_type, kind))
}

/// How much a file may weigh, from its first bytes alone — what an upload
/// is held to as it is read, before the rest of it has come. The most a
/// picture may weigh when it begins as one, the most a sound may when it
/// begins as one, and a refusal when it is neither, or is not what `wanted`
/// says. Longer than [`HEAD_BYTES`] is not needed; the whole file is checked
/// by [`inspect`] once it is read.
pub fn limit_for_head(head: &[u8], wanted: Option<Kind>) -> Result<(Kind, u64)> {
    std::panic::catch_unwind(|| {
        let (_, _, kind) = sniff(head, wanted)?;
        if let Some(wanted) = wanted
            && kind != wanted
        {
            bail!("a {} where {} was wanted", kind.as_str(), wanted.as_str());
        }
        Ok((kind, max_bytes(kind)))
    })
    .unwrap_or_else(|_| Err(anyhow::anyhow!("a file this server could not read")))
}

/// The most a file of a kind may weigh.
pub fn max_bytes(kind: Kind) -> u64 {
    match kind {
        Kind::Image => MAX_IMAGE_BYTES,
        Kind::Audio => MAX_AUDIO_BYTES,
    }
}

fn inspect_unguarded(bytes: &[u8], wanted: Kind) -> Result<Inspected> {
    let (ext, content_type, kind) = sniff(bytes, Some(wanted))?;
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

/// The most memory reading a picture can hold at once, in bytes, counted
/// from what its header says, before a byte of it is decoded: so many bytes
/// a pixel at the worst moment of the whole way from the file to the copy
/// made of it, and the file's own bytes, which a decoder may copy.
///
/// The moments are the decoder's buffer for the pixels with what it keeps
/// beside it (`inner` bytes a pixel); the copy a quarter turn makes
/// (`turned`) beside the pixels it turns; the copy that makes the pixels
/// eight bits a channel, RGB or RGBA, beside those it is made from — unless
/// they are that already and are moved; what is held at the end; and, where
/// a copy is to be made (`resized`), the resizer's, which holds beside the
/// picture the copy it draws, a strip as tall as the picture and as wide as
/// the copy — as wide as the picture, at the worst — and, where there is an
/// alpha channel, the pixels premultiplied by it. So a grey picture comes to
/// 9 bytes a pixel, an RGB one to 9, an RGBA one to 16, a sixteen-bit one to
/// 9 or 16; with no copy to make, to 4, 3, 4 and 9 or 12. A progressive
/// JPEG's coefficients add 6 to its 3, and a quarter turn doubles what the
/// decoder gave. [`FIXED`] is counted besides, whatever the picture is.
///
/// The decoders' own limits cover their buffers alone, and not all of them
/// even those: the one that reads JPEGs holds its coefficients under none.
/// What a hostile header may ask for is what is counted, never what a
/// smaller picture would take. What an encoder holds to write the copy is
/// not: it is a copy at 1920 pixels wide at the most, and its own matter.
fn working_set(
    pixels: u64,
    color: image::ColorType,
    inner: u64,
    turned: bool,
    resized: bool,
    file: u64,
) -> u64 {
    use image::ColorType::{Rgb8, Rgba8};

    let native = u64::from(color.bytes_per_pixel());
    // What a picture is held as: eight bits a channel, with an alpha channel
    // where it has one.
    let kept = if color.has_alpha() { 4 } else { 3 };
    let decoding = native + inner;
    let turning = if turned { 2 * native } else { 0 };
    // Already the shape it is held in, it is moved and not copied.
    let converting = if matches!(color, Rgb8 | Rgba8) {
        native
    } else {
        native + kept
    };
    let resizing = if resized {
        let premultiplied = if color.has_alpha() { 4 } else { 0 };
        3 * kept + premultiplied
    } else {
        0
    };
    let per_pixel = decoding
        .max(turning)
        .max(converting)
        .max(resizing)
        .max(kept);
    pixels
        .saturating_mul(per_pixel)
        .saturating_add(file)
        .saturating_add(FIXED)
}

/// What reading a picture holds whatever it is: its sketch, what an encoder
/// writes, the tables of a decoder.
const FIXED: u64 = 8 * 1024 * 1024;

/// The bytes a pixel a decoder keeps beside the pixels it hands over, at
/// the worst, by the format it reads: measured, and rounded up. A PNG's are
/// a few rows; a lossy WebP keeps its YUV planes, a pixel and a half, and a
/// lossless one next to nothing; a GIF's frame is a byte a pixel before it
/// is drawn; a JPEG's are in [`jpeg_coefficients`].
fn inner_bytes(format: Option<image::ImageFormat>, bytes: &[u8]) -> u64 {
    match format {
        Some(image::ImageFormat::Jpeg) => jpeg_coefficients(bytes),
        Some(image::ImageFormat::WebP) => 3,
        Some(image::ImageFormat::Gif) => 2,
        _ => 0,
    }
}

/// What a JPEG's decoder keeps beside the pixels, a pixel at a time: a
/// sequential one, a few rows; a progressive one, every coefficient — two
/// bytes a sample, of each colour, as 4:4:4 would have them — until it
/// draws. A header this decoder cannot read is taken for the worst.
fn jpeg_coefficients(bytes: &[u8]) -> u64 {
    use jpeg_decoder::{CodingProcess, PixelFormat};

    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let info = decoder.read_info().ok().and_then(|()| decoder.info());
    let colours = |format| match format {
        PixelFormat::L8 | PixelFormat::L16 => 1,
        PixelFormat::RGB24 => 3,
        _ => 4,
    };
    match info {
        Some(info) if info.coding_process == CodingProcess::DctSequential => 0,
        Some(info) => 2 * colours(info.pixel_format),
        None => 2 * 4,
    }
}

/// Whether a picture is turned a quarter, which copies its pixels: the
/// others are turned where they are.
fn is_quarter_turn(orientation: image::metadata::Orientation) -> bool {
    use image::metadata::Orientation::{Rotate90, Rotate90FlipH, Rotate270, Rotate270FlipH};
    matches!(
        orientation,
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH
    )
}

/// What a picture whose worst case is `weight` is refused with: more than
/// this server sets aside, said in the unit the documentation does.
fn too_heavy(width: u32, height: u32, weight: u64) -> anyhow::Error {
    anyhow::anyhow!(
        "a picture of {width}×{height} would take up to {} MiB to decode, more than this server sets aside",
        weight.div_ceil(1024 * 1024)
    )
}

/// A picture's pixels, read within bounds — or why not.
///
/// The header is read first, by the decoder that would draw it: a picture
/// whose side is past [`MAX_SIDE`] or whose pixels are past
/// [`DECODE_MAX_PIXELS`] is refused before a byte of it is decoded; so is
/// one whose worst case in memory ([`working_set`]) is past
/// [`DECODE_BUDGET`] — `resized` saying that a copy is to be made of it,
/// which holds more than a sketch drawn from it does. What it needs is handed
/// to `reserve` — which may wait, for the pictures decoding beside it to be
/// done — before the first buffer is allocated, and what `reserve` gives
/// back is returned with the picture, to be kept for as long as the picture
/// and what is made of it are.
/// Turned the way its EXIF orientation says, as a camera's photograph is
/// shown. Pure Rust: no C decoder ever reads bytes somebody else chose. The
/// caller runs it off the server's threads and catches what a decoder may
/// still panic on.
pub fn decode_with<H>(
    bytes: &[u8],
    resized: bool,
    reserve: impl FnOnce(u64) -> H,
) -> Result<(image::DynamicImage, H)> {
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
    limits.max_alloc = Some(DECODE_BUDGET);

    let mut reader = reader()?;
    reader.limits(limits.clone());
    let format = reader.format();
    let mut decoder = reader
        .into_decoder()
        .context("the picture could not be read")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    // What the whole of reading it can hold, counted before anything is
    // allocated: the decoders' own limits cover their buffers alone, and
    // not every one of them even that.
    let weight = working_set(
        u64::from(width) * u64::from(height),
        decoder.color_type(),
        inner_bytes(format, bytes),
        is_quarter_turn(orientation),
        resized,
        bytes.len() as u64,
    );
    if weight > DECODE_BUDGET {
        return Err(too_heavy(width, height, weight));
    }
    // What the pixels take is reserved against the bound first, and the
    // decoder is left what remains of it for its own buffers.
    limits
        .reserve(decoder.total_bytes())
        .context("the picture's pixels would take more than this server sets aside")?;
    decoder
        .set_limits(limits)
        .context("the picture is larger than this server decodes")?;
    let held = reserve(weight);
    let mut picture =
        image::DynamicImage::from_decoder(decoder).context("the picture could not be decoded")?;
    picture.apply_orientation(orientation);
    Ok((picture, held))
}

/// What drawing a JPEG small holds whatever it is: the rows and the tables
/// its decoder works from, counted as a megabyte.
const SMALL_FLOOR: u64 = 1024 * 1024;

/// The threads a JPEG is drawn small on: `jpeg-decoder` hands each of a
/// picture's colours to a thread, and without threads given it starts one
/// for each and lets it go, every picture — whose leavings the allocator
/// kept, some 20 MB more under a library's import. Two, made once: as many
/// as pictures are decoded at once (`variants::AT_ONCE`). None if they
/// could not be made, when rayon's own are drawn on.
static DRAWING: LazyLock<Option<rayon::ThreadPool>> = LazyLock::new(|| {
    rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .thread_name(|n| format!("jpeg-{n}"))
        .build()
        .ok()
});

/// A picture's pixels, `side` or more on one of its sides where it can be
/// drawn that small without being drawn whole first — and whole otherwise,
/// within [`decode_with`]'s bounds either way, and handed to `reserve` as
/// it is.
///
/// A JPEG in colour or in grey is: its inverse transform draws it at half
/// its sides, a quarter or an eighth, the smallest of those that keeps
/// `side`. TMDB's poster of 2000×3000, drawn for its placeholder, took
/// 42 MB to decode and to hold as RGBA, and takes 0.7 now; a progressive
/// JPEG, whose every coefficient is read before any of it is drawn, half of
/// what it took. Turned the way its EXIF orientation says, as
/// [`decode_with`] turns it. Pure Rust too.
pub fn decode_small_with<H>(
    bytes: &[u8],
    side: u32,
    reserve: impl FnOnce(u64) -> H,
) -> Result<(image::DynamicImage, H)> {
    use jpeg_decoder::{CodingProcess, PixelFormat};

    if image::guess_format(bytes).ok() != Some(image::ImageFormat::Jpeg) {
        return decode_with(bytes, false, reserve);
    }
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder
        .read_info()
        .context("the picture's header could not be read")?;
    let info = decoder
        .info()
        .context("the picture's header could not be read")?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) > DECODE_MAX_PIXELS
    {
        bail!("a picture of {width}×{height} is larger than this server decodes");
    }
    // CMYK, and twelve bits a channel, are read whole, as they always were.
    if !matches!(info.pixel_format, PixelFormat::RGB24 | PixelFormat::L8) {
        return decode_with(bytes, false, reserve);
    }

    let asked = u16::try_from(side).unwrap_or(u16::MAX);
    let (drawn_w, drawn_h) = decoder
        .scale(asked, asked)
        .context("the picture could not be read")?;
    // What drawing it small holds: the pixels drawn, and the grey made RGB
    // from them — and, for a progressive JPEG, every coefficient beside
    // them, two bytes a sample of each colour.
    let colours = if info.pixel_format == PixelFormat::RGB24 {
        3
    } else {
        1
    };
    let drawn = u64::from(drawn_w) * u64::from(drawn_h) * 4;
    let coefficients = if info.coding_process == CodingProcess::DctSequential {
        0
    } else {
        2 * colours * u64::from(width) * u64::from(height)
    };
    let weight = drawn + coefficients + SMALL_FLOOR;
    if weight > DECODE_BUDGET {
        return Err(too_heavy(width, height, weight));
    }
    decoder.set_max_decoding_buffer_size(usize::try_from(DECODE_BUDGET).unwrap_or(usize::MAX));
    let held = reserve(weight);
    let pixels = match DRAWING.as_ref() {
        Some(threads) => threads.install(|| decoder.decode()),
        None => decoder.decode(),
    }
    .context("the picture could not be decoded")?;
    let (drawn_w, drawn_h) = (u32::from(drawn_w), u32::from(drawn_h));
    let mut picture = match info.pixel_format {
        PixelFormat::RGB24 => {
            image::RgbImage::from_raw(drawn_w, drawn_h, pixels).map(image::DynamicImage::from)
        }
        _ => image::GrayImage::from_raw(drawn_w, drawn_h, pixels).map(image::DynamicImage::from),
    }
    .context("the picture's pixels are not the size its header says")?;
    // Read with the pixels, after them.
    if let Some(orientation) = decoder
        .exif_data()
        .and_then(image::metadata::Orientation::from_exif_chunk)
    {
        picture.apply_orientation(orientation);
    }
    Ok((picture, held))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture's pixels, with nothing held for them: what the tests ask.
    fn decode(bytes: &[u8]) -> Result<image::DynamicImage> {
        decode_with(bytes, true, |_| ()).map(|(picture, ())| picture)
    }

    fn decode_small(bytes: &[u8], side: u32) -> Result<image::DynamicImage> {
        decode_small_with(bytes, side, |_| ()).map(|(picture, ())| picture)
    }

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
    fn checked_png_header(width: u32, height: u32, depth: u8, colour: u8) -> Vec<u8> {
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
        header.extend_from_slice(&[depth, colour, 0, 0, 0]);
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
        let refused = decode(&checked_png_header(10_000, 10_001, 8, 2)).unwrap_err();
        assert!(
            format!("{refused:#}").contains("larger than this server decodes"),
            "{refused:#}"
        );
        // Under the count of pixels, past what may be set aside for them:
        // 9 000 × 9 000 of RGBA is 324 megabytes.
        let refused = decode(&checked_png_header(9_000, 9_000, 8, 6)).unwrap_err();
        assert!(format!("{refused:#}").contains("sets aside"), "{refused:#}");
        // And a real picture is read.
        let picture = decode(&png(30, 20)).expect("decoded");
        assert_eq!((picture.width(), picture.height()), (30, 20));
        assert!(decode(b"not a picture at all").is_err());
    }

    /// The decoder's own buffer is a hundred megabytes for a grey canvas of
    /// 10 000 × 10 000, which no limit on the buffer minds: made RGB beside
    /// it and, for a placeholder, RGBA beside that, it was 700. A header
    /// that promises one is refused before the pixels are asked for, as
    /// one whose whole way through memory is more than the budget.
    #[test]
    fn a_grey_canvas_the_copies_would_swell_is_refused_before_it_is_decoded() {
        for (width, height, depth, colour) in [
            // 8 bits of grey: 4 bytes a pixel at the worst, 400 MB.
            (10_000, 10_000, 8, 0),
            (8_500, 8_500, 8, 0),
            // 16 bits of grey: 5 bytes a pixel.
            (8_000, 8_000, 16, 0),
            // Grey with alpha: 6.
            (7_000, 7_000, 8, 4),
            // 16 bits of RGB: 9; of RGBA: 12.
            (6_000, 6_000, 16, 2),
            (5_000, 5_000, 16, 6),
        ] {
            let refused = decode(&checked_png_header(width, height, depth, colour)).unwrap_err();
            let refused = format!("{refused:#}");
            assert!(
                refused.contains("sets aside") && refused.contains("MiB to decode"),
                "{width}×{height}: {refused}"
            );
        }
        // What fits is let through to the decoder, which finds no pixels.
        let refused = decode(&checked_png_header(3_000, 3_000, 8, 0)).unwrap_err();
        assert!(
            !format!("{refused:#}").contains("sets aside"),
            "{refused:#}"
        );
    }

    /// A picture is counted at its worst moment, so many bytes a pixel by
    /// what it is: the decoder's buffer and what it keeps beside it, the
    /// copy a quarter turn makes, the copy that makes it eight bits a
    /// channel, what remains, and the resizer's — which keeps an alpha
    /// channel's pixels premultiplied beside them, and a strip as wide as
    /// the picture at the worst.
    #[test]
    fn a_pictures_working_set_is_its_worst_moment_by_the_byte_a_pixel() {
        use image::ColorType::{L8, L16, La8, La16, Rgb8, Rgb16, Rgba8, Rgba16};

        // With no copy to make: the sketch alone.
        let sketched =
            |color, inner, turned| working_set(1, color, inner, turned, false, 0) - FIXED;
        // Grey is made RGB beside itself, 1 and 3; sixteen bits of RGBA,
        // RGBA beside it, 8 and 4. RGB and RGBA go on as they are.
        assert_eq!(sketched(L8, 0, false), 4);
        assert_eq!(sketched(La8, 0, false), 6);
        assert_eq!(sketched(Rgb8, 0, false), 3);
        assert_eq!(sketched(Rgba8, 0, false), 4);
        assert_eq!(sketched(L16, 0, false), 5);
        assert_eq!(sketched(La16, 0, false), 8);
        assert_eq!(sketched(Rgb16, 0, false), 9);
        assert_eq!(sketched(Rgba16, 0, false), 12);
        // A quarter turn doubles what the decoder gave; a progressive
        // JPEG's coefficients, a WebP's planes, come beside it.
        assert_eq!(sketched(Rgb8, 0, true), 6);
        assert_eq!(sketched(Rgba16, 0, true), 16);
        assert_eq!(sketched(Rgb8, 6, false), 9);
        assert_eq!(sketched(Rgba8, 3, false), 7);

        // With a copy to make: the picture held, a strip as wide beside it,
        // and where there is alpha the premultiplied pixels too.
        let copied = |color, inner, turned| working_set(1, color, inner, turned, true, 0) - FIXED;
        assert_eq!(copied(Rgb8, 0, false), 9);
        assert_eq!(copied(L8, 0, false), 9);
        assert_eq!(copied(Rgba8, 0, false), 16);
        assert_eq!(copied(La8, 0, false), 16);
        assert_eq!(copied(Rgb16, 0, false), 9);
        assert_eq!(copied(Rgba16, 0, false), 16);
        // What was worse without it stays so.
        assert_eq!(copied(Rgb8, 7, false), 10);
        assert_eq!(copied(Rgba16, 0, true), 16);

        // The file's own bytes and the fixed ones come besides, and nothing
        // overflows.
        assert_eq!(working_set(10, Rgb8, 0, false, false, 1000), 1030 + FIXED);
        assert_eq!(
            working_set(u64::MAX, Rgba16, 0, true, true, u64::MAX),
            u64::MAX
        );
    }

    /// A JPEG's header as far as `jpeg-decoder` reads to say how it is
    /// coded: its frame.
    fn jpeg_frame(progressive: bool, colours: u8) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8, 0xFF, if progressive { 0xC2 } else { 0xC0 }];
        out.extend_from_slice(&(8 + 3 * u16::from(colours)).to_be_bytes());
        out.extend_from_slice(&[8, 1, 0, 1, 0, colours]);
        for id in 1..=colours {
            out.extend_from_slice(&[id, 0x11, 0]);
        }
        out
    }

    #[test]
    fn a_jpeg_is_counted_by_how_it_is_coded() {
        // A sequential one keeps a few rows; a progressive one, two bytes
        // a sample of each colour, and a header that cannot be read, the
        // worst of them.
        assert_eq!(jpeg_coefficients(&jpeg_frame(false, 3)), 0);
        assert_eq!(jpeg_coefficients(&jpeg_frame(true, 3)), 6);
        assert_eq!(jpeg_coefficients(&jpeg_frame(true, 1)), 2);
        assert_eq!(jpeg_coefficients(&jpeg_frame(true, 4)), 8);
        assert_eq!(jpeg_coefficients(b"\xFF\xD8 and nothing else"), 8);
        assert_eq!(jpeg_coefficients(&jpeg(40, 30)), 0);
        assert_eq!(
            inner_bytes(Some(image::ImageFormat::Jpeg), &jpeg_frame(true, 3)),
            6
        );
        assert_eq!(inner_bytes(Some(image::ImageFormat::WebP), b""), 3);
        assert_eq!(inner_bytes(Some(image::ImageFormat::Gif), b""), 2);
        assert_eq!(inner_bytes(Some(image::ImageFormat::Png), b""), 0);
    }

    /// What a picture reserves is its worst case, handed to the caller
    /// before a byte is decoded and handed back with the picture: nothing
    /// is asked for one that is refused.
    #[test]
    fn what_a_picture_reserves_is_handed_back_with_it() {
        let reserved = std::cell::Cell::new(0);
        let (picture, held) = decode_with(&png(30, 20), true, |weight| {
            reserved.set(weight);
            weight
        })
        .expect("decoded");
        assert_eq!((picture.width(), picture.height()), (30, 20));
        // 600 pixels of RGB, 9 bytes each with the copy to be made of it,
        // the file's own bytes and the fixed ones.
        assert_eq!(held, 600 * 9 + png(30, 20).len() as u64 + FIXED);
        assert_eq!(reserved.get(), held);
        // Less where no copy is: 3 bytes each.
        let (_, held) = decode_with(&png(30, 20), false, |weight| weight).unwrap();
        assert_eq!(held, 600 * 3 + png(30, 20).len() as u64 + FIXED);
        // Never asked of a picture refused.
        let asked = std::cell::Cell::new(false);
        let bomb = checked_png_header(10_000, 10_000, 8, 0);
        assert!(decode_with(&bomb, true, |_| asked.set(true)).is_err());
        assert!(!asked.get(), "a picture refused is reserved nothing");
    }

    /// Where a file is cut as it is read: by what its first bytes say it
    /// is, a picture or a sound — and refused at once when it is neither.
    #[test]
    fn a_file_is_held_to_what_its_first_bytes_say_it_is() {
        let picture = png(8, 8);
        assert_eq!(
            limit_for_head(&picture, None).unwrap(),
            (Kind::Image, MAX_IMAGE_BYTES)
        );
        assert_eq!(
            limit_for_head(&picture, Some(Kind::Image)).unwrap(),
            (Kind::Image, MAX_IMAGE_BYTES)
        );
        assert!(limit_for_head(&picture, Some(Kind::Audio)).is_err());
        for sound in [
            &b"ID3\x03\0\0\0\0\0\0"[..],
            b"OggS\0\x02\0\0\0\0\0\0",
            b"fLaC\0\0\0\x22",
            b"RIFF\x24\0\0\0WAVEfmt ",
            b"\0\0\0\x20ftypM4A \0\0\0\0",
        ] {
            assert_eq!(
                limit_for_head(sound, None).unwrap(),
                (Kind::Audio, MAX_AUDIO_BYTES),
                "{sound:?}"
            );
            assert!(
                limit_for_head(sound, Some(Kind::Image)).is_err(),
                "{sound:?}"
            );
        }
        assert!(limit_for_head(b"<html><script>alert(1)</script></html>", None).is_err());
        assert!(limit_for_head(b"", None).is_err());
        assert_eq!(max_bytes(Kind::Image), MAX_IMAGE_BYTES);
        assert_eq!(max_bytes(Kind::Audio), MAX_AUDIO_BYTES);
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_fn(width, height, |x, _| {
            image::Rgb(if x < width / 2 {
                [20, 40, 90]
            } else {
                [230, 120, 30]
            })
        })
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
        .unwrap();
        out
    }

    /// The same JPEG, its EXIF saying it is shown turned a quarter clockwise:
    /// an APP1 segment after the SOI, a little-endian TIFF header and an IFD
    /// of one entry, the orientation (0x0112), a SHORT, 6.
    fn turned(jpeg: &[u8]) -> Vec<u8> {
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        exif.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);
        let length = u16::try_from(exif.len() + 2).unwrap().to_be_bytes();
        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1, length[0], length[1]]);
        out.extend_from_slice(&exif);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn a_jpeg_is_decoded_small_and_anything_else_whole() {
        let side = |picture: image::DynamicImage| (picture.width(), picture.height());
        // An eighth of its sides keeps 200 on both.
        assert_eq!(
            side(decode_small(&jpeg(3840, 2160), 200).unwrap()),
            (480, 270)
        );
        // An eighth would leave both under 200: a quarter.
        assert_eq!(
            side(decode_small(&jpeg(1000, 1500), 200).unwrap()),
            (250, 375)
        );
        // Smaller than asked already: as it is.
        assert_eq!(
            side(decode_small(&jpeg(150, 100), 200).unwrap()),
            (150, 100)
        );
        // A PNG has no smaller drawing: whole.
        assert_eq!(side(decode_small(&png(800, 600), 200).unwrap()), (800, 600));
        assert!(decode_small(b"not a picture at all", 200).is_err());
    }

    #[test]
    fn a_jpeg_decoded_small_is_turned_as_its_exif_says() {
        let bytes = turned(&jpeg(1600, 800));
        // As the whole decode turns it, which reads the same EXIF.
        let whole = decode(&bytes).unwrap();
        assert_eq!((whole.width(), whole.height()), (800, 1600));
        let small = decode_small(&bytes, 200).unwrap();
        assert_eq!((small.width(), small.height()), (100, 200));
    }
}
