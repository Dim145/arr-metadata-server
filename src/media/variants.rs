//! The copies made of a picture kept: at the widths the interface draws it,
//! as WebP — and, at night, as AVIF — and a square cut of a background for
//! Plex. The original stays what it was, for every client that reads
//! pictures: Sonarr, Radarr, Kodi, Jellyfin and Plex are given it, never a
//! copy.
//!
//! A copy is named by the picture's hash and its width, `<hash>-480.webp`,
//! and kept beside it; it is made the first time it is asked for, and the
//! one the interface asks for first — a poster's 480, a background's 1280 —
//! as the picture is stored. A picture is never made larger than it is: a
//! width past its own is its own width.
//!
//! The WebP copies are made only while `media.webp` is on: off, as it is
//! until somebody sets it, the pages are given the pictures themselves, and
//! no WebP is made — not as a picture is stored, not when one is asked for.
//! The square Plex is given is made whatever it says.

use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Condvar, LazyLock, Mutex, PoisonError},
};

use anyhow::{Context as _, Result, bail};
use fast_image_resize as resize;

use super::{
    encode::{self, Pixels},
    file::{self, Made},
    placeholder::Placeholder,
};
use crate::domain::CoverType;

/// Every width a copy is made at: the widths the interface draws each kind
/// of picture at, once and twice over for a dense screen, together —
/// `frontend/src/lib/media.ts` asks for these and no others: a poster's
/// 240, 480 and 960; a background's 640, 1280 and 1920; a landscape's and
/// a banner's 500 and 1000; a logo's 400 and 800; a still's 320 and 640; a
/// photograph's 120 and 240. Any other width asked for — but the picture's
/// own — is refused, so that a picture has a dozen copies at the most,
/// whoever asks and however often.
pub const WIDTHS: [u32; 12] = [
    120, 240, 320, 400, 480, 500, 640, 800, 960, 1000, 1280, 1920,
];

/// The widths the night's AVIF copies are made at: a poster's, and a
/// background's — what the pages lead with. A page offers an AVIF only where
/// the copies reach as wide as its ladder, and a dense screen takes a
/// background's 1920: without it, no background would ever be drawn from
/// an AVIF there.
pub const AVIF_POSTER_WIDTHS: [u32; 2] = [480, 960];
pub const AVIF_BACKDROP_WIDTHS: [u32; 2] = [1280, 1920];

/// The side of the square Plex is given, cut from the middle of a
/// background: a large television's tile.
pub const SQUARE_SIDE: u32 = 1080;

/// What has been done to a picture as it was stored, by this version: its
/// placeholder worked out, and — while the WebP copies are on — the copy
/// its kind is first asked for made. A picture prepared by an earlier
/// version — or by none — is the task `media.variants`' to prepare.
pub const PREPARED: i64 = 1;

/// The copy made of a picture as it is stored, by what the work it was
/// fetched for has it as: a poster's 480, a background's 1280.
pub fn eager_width(cover: Option<CoverType>) -> Option<u32> {
    match cover? {
        CoverType::Poster | CoverType::Keyart => Some(480),
        CoverType::Fanart => Some(1280),
        _ => None,
    }
}

/// The widths of the night's AVIF copies, by what the picture is.
pub fn avif_widths(cover: Option<CoverType>) -> &'static [u32] {
    match cover {
        Some(CoverType::Poster | CoverType::Keyart) => &AVIF_POSTER_WIDTHS,
        Some(CoverType::Fanart) => &AVIF_BACKDROP_WIDTHS,
        _ => &[],
    }
}

/// The width a copy asked for is made at: the width asked, never past the
/// picture's own — `None` for a width the interface never asks for. The
/// picture's own width is one it asks for, and one made, only where the
/// picture is no wider than the widest copy: past that, "its own" is a
/// request for the whole of a picture of any size, drawn and written again
/// at full size.
pub fn width_made(asked: u32, own: u32) -> Option<u32> {
    let widest = WIDTHS[WIDTHS.len() - 1];
    (WIDTHS.contains(&asked) || (asked == own && own <= widest)).then(|| asked.min(own))
}

/// What a copy is written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Webp,
    Avif,
    Jpeg,
}

impl Format {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Jpeg => "jpeg",
        }
    }

    pub const fn ext(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Jpeg => "jpg",
        }
    }

    pub fn of_ext(ext: &str) -> Option<Self> {
        match ext {
            "webp" => Some(Self::Webp),
            "avif" => Some(Self::Avif),
            "jpg" => Some(Self::Jpeg),
            _ => None,
        }
    }
}

/// A picture's pixels, decoded: RGB, or RGBA where it is transparent
/// somewhere.
pub struct Decoded {
    pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    /// What reading the picture reserved of the memory the pictures decoding
    /// may hold together, kept for as long as the picture is: given back
    /// when it is dropped. Nothing for a copy of it made smaller, which is
    /// within what was reserved for the whole.
    _held: Option<Held>,
}

impl Decoded {
    /// Read a picture's bytes — see [`file::decode_with`] for the bounds.
    /// Waits, on the thread it is called from, for the memory its picture
    /// needs to be there: see [`Budget`].
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let (picture, held) = file::decode_with(bytes, true, |weight| BUDGET.take(weight))?;
        Ok(Self::of(picture, Some(held)))
    }

    /// Read a picture's bytes `side` or more on one of its sides, smaller
    /// than it is where its format allows: see [`file::decode_small_with`].
    pub fn read_small(bytes: &[u8], side: u32) -> Result<Self> {
        let (picture, held) = file::decode_small_with(bytes, side, |weight| BUDGET.take(weight))?;
        Ok(Self::of(picture, Some(held)))
    }

    fn of(picture: image::DynamicImage, held: Option<Held>) -> Self {
        let (width, height) = (picture.width(), picture.height());
        // An alpha channel that is opaque everywhere is a photograph like
        // any other; only real transparency is kept.
        if picture.color().has_alpha() {
            let mut rgba = picture.into_rgba8().into_raw();
            let transparent = rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
            if !transparent {
                drop_alpha(&mut rgba);
            }
            return Self {
                pixels: rgba,
                width,
                height,
                alpha: transparent,
                _held: held,
            };
        }
        Self {
            pixels: picture.into_rgb8().into_raw(),
            width,
            height,
            alpha: false,
            _held: held,
        }
    }

    pub fn as_pixels(&self) -> Pixels<'_> {
        Pixels {
            data: &self.pixels,
            width: self.width,
            height: self.height,
            alpha: self.alpha,
        }
    }

    /// The same picture at another size: Lanczos3, with the colours of a
    /// transparent one weighed by their alpha, so that a logo's edges do
    /// not darken. Cut from the middle where the proportions differ.
    pub fn resized(&self, width: u32, height: u32) -> Result<Self> {
        if (width, height) == (self.width, self.height) {
            return Ok(Self {
                pixels: self.pixels.clone(),
                width,
                height,
                alpha: self.alpha,
                _held: None,
            });
        }
        let kind = if self.alpha {
            resize::PixelType::U8x4
        } else {
            resize::PixelType::U8x3
        };
        let from = resize::images::ImageRef::new(self.width, self.height, &self.pixels, kind)
            .context("the pixels could not be read for resizing")?;
        let mut to = resize::images::Image::new(width, height, kind);
        resize::Resizer::new()
            .resize(
                &from,
                &mut to,
                &resize::ResizeOptions::new()
                    .resize_alg(resize::ResizeAlg::Convolution(resize::FilterType::Lanczos3))
                    .use_alpha(true)
                    .fit_into_destination(Some((0.5, 0.5))),
            )
            .context("the picture could not be resized")?;
        Ok(Self {
            pixels: to.into_vec(),
            width,
            height,
            alpha: self.alpha,
            _held: None,
        })
    }

    /// The pixels with any transparency laid over black: what a format
    /// with none is given.
    fn flattened(&self) -> Self {
        if !self.alpha {
            return Self {
                pixels: self.pixels.clone(),
                width: self.width,
                height: self.height,
                alpha: false,
                _held: None,
            };
        }
        let pixels = self
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let over = |c: u8| ((u16::from(c) * u16::from(p[3]) + 127) / 255) as u8;
                [over(p[0]), over(p[1]), over(p[2])]
            })
            .collect();
        Self {
            pixels,
            width: self.width,
            height: self.height,
            alpha: false,
            _held: None,
        }
    }

    /// Whether it is taller than it is wide: a poster's lettering is what
    /// libwebp's sharper colour conversion keeps.
    fn is_portrait(&self) -> bool {
        self.height > self.width
    }
}

/// RGBA made RGB where it lies, each pixel's three colours moved down over
/// the alpha of those before it: a second buffer would be held beside the
/// first at the moment both are whole, a third as much again as the picture.
fn drop_alpha(pixels: &mut Vec<u8>) {
    let count = pixels.len() / 4;
    for at in 0..count {
        pixels.copy_within(at * 4..at * 4 + 3, at * 3);
    }
    pixels.truncate(count * 3);
}

/// A copy made.
pub struct Copy {
    pub made: Made,
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

impl Copy {
    /// Its key, beside the picture of `stem`.
    pub fn key(&self, stem: &str) -> String {
        file::made_key(stem, self.made, self.format.ext())
    }

    pub fn content_type(&self) -> &'static str {
        match self.format {
            Format::Webp => "image/webp",
            Format::Avif => "image/avif",
            Format::Jpeg => "image/jpeg",
        }
    }
}

/// The height of a copy `width` wide, in the picture's proportions.
fn height_at(decoded: &Decoded, width: u32) -> u32 {
    let height = (u64::from(decoded.height) * u64::from(width) + u64::from(decoded.width) / 2)
        / u64::from(decoded.width);
    u32::try_from(height).unwrap_or(u32::MAX).max(1)
}

/// Whether a picture is drawn art — a logo, clear art, character art, a
/// disc — whose edges are its transparency, and whose few flat colours a
/// lossless copy keeps in fewer bytes than a lossy one. A photograph that
/// is transparent somewhere — a poster's rounded corners — is copied as any
/// photograph, its transparency kept exact beside its lossy colours.
pub fn is_drawn(cover: Option<CoverType>) -> bool {
    matches!(
        cover,
        Some(
            CoverType::Clearlogo
                | CoverType::Clearart
                | CoverType::Characterart
                | CoverType::Discart
        )
    )
}

/// Make one copy of a decoded picture: at a width, as a WebP — lossless
/// for drawn art that is transparent somewhere (see [`is_drawn`]) — or an
/// AVIF; or the square, as a JPEG. A width past the picture's own is its
/// own. `cover` is what the picture is to its work, where that is known.
pub fn make(
    decoded: &Decoded,
    made: Made,
    format: Format,
    cover: Option<CoverType>,
) -> Result<Copy> {
    match (made, format) {
        (Made::Width(asked), Format::Webp | Format::Avif) => {
            let width = asked.min(decoded.width);
            let height = height_at(decoded, width);
            let mut resized = None;
            let small = drawn_at(decoded, width, height, &mut resized)?;
            let bytes = if format == Format::Avif {
                encode::avif(&small.as_pixels())?
            } else {
                let lossless = small.alpha && is_drawn(cover);
                encode::webp(&small.as_pixels(), lossless, small.is_portrait())?
            };
            Ok(Copy {
                made: Made::Width(width),
                format,
                width,
                height,
                bytes,
            })
        }
        (Made::Square, Format::Jpeg) => {
            if decoded.height > decoded.width {
                bail!("a square is cut from a background, not from a picture taller than wide");
            }
            let side = SQUARE_SIDE.min(decoded.height);
            let square = decoded.resized(side, side)?.flattened();
            Ok(Copy {
                made: Made::Square,
                format,
                width: side,
                height: side,
                bytes: encode::jpeg(&square.as_pixels())?,
            })
        }
        _ => bail!("{made:?} is not made as {}", format.as_str()),
    }
}

/// A picture at the size a copy is made at: itself where it is that size
/// already — a copy at a picture's own width copies none of its pixels, which
/// would be a second of them beside the first — and otherwise resized into
/// `resized`, whose picture is returned.
fn drawn_at<'a>(
    decoded: &'a Decoded,
    width: u32,
    height: u32,
    resized: &'a mut Option<Decoded>,
) -> Result<&'a Decoded> {
    if (width, height) == (decoded.width, decoded.height) {
        return Ok(decoded);
    }
    Ok(resized.insert(decoded.resized(width, height)?))
}

/// What preparing a picture as it is stored came to: its placeholder, and
/// the copy its kind is first asked for, when one was made.
pub struct Prepared {
    pub placeholder: Option<Placeholder>,
    pub copies: Vec<Copy>,
}

/// Prepare a picture: its placeholder and, where its kind has one and
/// `webp` says the WebP copies are on (`media.webp`), its first copy — the
/// picture decoded once for both. `cover` is what the picture is to its
/// work, where that is known.
pub fn prepare(bytes: &[u8], cover: Option<CoverType>, webp: bool) -> Result<Prepared> {
    let copy = eager_width(cover).filter(|_| webp);
    // The placeholder alone, when no copy is made: drawn from the picture
    // decoded small, whose sketch is the same for a fraction of the memory.
    let decoded = match copy {
        Some(_) => Decoded::read(bytes)?,
        None => Decoded::read_small(bytes, super::placeholder::DRAWN_FROM)?,
    };
    let placeholder = super::placeholder::of(&decoded)
        .inspect_err(|e| tracing::debug!(error = format_args!("{e:#}"), "no placeholder"))
        .ok();
    let mut copies = Vec::new();
    if let Some(width) = copy {
        copies.push(make(&decoded, Made::Width(width), Format::Webp, cover)?);
    }
    Ok(Prepared {
        placeholder,
        copies,
    })
}

/// How many pictures are decoded at once, whoever asks — the worker's
/// fetches, an upload, a page asking for a copy, the task: a large picture
/// takes a few hundred megabytes to decode, and the copies a page asks for
/// on its first visit are a dozen at a time. What they hold together is
/// bounded besides, by [`Budget`], whatever their sizes.
const AT_ONCE: usize = 2;
static DECODING: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(AT_ONCE)));
/// The night's AVIF copies, one at a time and apart from the rest: one
/// takes seconds, and the copies a page waits for are not to wait on it.
static AVIF: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

/// What the pictures being decoded may hold in memory together
/// ([`file::DECODE_BUDGET`]), however many the gate lets through and
/// whatever they are: each takes the share its picture's worst case needs
/// ([`file::decode_with`] counts it from the header) before a byte is
/// allocated for it, and gives it back when what it decoded is dropped.
/// Taken in turn, so that a large picture is not kept waiting for ever by a
/// string of small ones; the thread that waits is the one the work runs on
/// (see [`Turn::run`]), of which [`AT_ONCE`] and the night's one are all
/// there are.
struct Budget {
    limit: u64,
    shares: Mutex<Shares>,
    changed: Condvar,
}

#[derive(Default)]
struct Shares {
    /// What is held.
    used: u64,
    /// The next place in line to hand out, and the one whose turn it is.
    next: u64,
    serving: u64,
}

static BUDGET: LazyLock<Arc<Budget>> = LazyLock::new(|| Arc::new(Budget::new(file::DECODE_BUDGET)));

impl Budget {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            shares: Mutex::new(Shares::default()),
            changed: Condvar::new(),
        }
    }

    /// A share of `bytes`, waited for until it fits beside those held and
    /// every share asked for before it has been given. One larger than the
    /// whole is the whole: the pictures that need more are refused before
    /// they ask.
    fn take(self: &Arc<Self>, bytes: u64) -> Held {
        let bytes = bytes.min(self.limit);
        let mut shares = self.shares.lock().unwrap_or_else(PoisonError::into_inner);
        let place = shares.next;
        shares.next += 1;
        while shares.serving != place || shares.used + bytes > self.limit {
            shares = self
                .changed
                .wait(shares)
                .unwrap_or_else(PoisonError::into_inner);
        }
        shares.used += bytes;
        shares.serving += 1;
        // The one behind may fit beside this.
        self.changed.notify_all();
        Held {
            budget: self.clone(),
            bytes,
        }
    }
}

/// A share of the [`Budget`], given back when it is dropped.
pub struct Held {
    budget: Arc<Budget>,
    bytes: u64,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut shares = self
            .budget
            .shares
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        shares.used = shares.used.saturating_sub(self.bytes);
        self.budget.changed.notify_all();
    }
}

/// A place among the pictures decoded at once ([`AT_ONCE`]), held until the
/// work it was waited for is done. Taken before the picture is read from
/// the store, so that no more pictures sit in memory waiting to be decoded
/// than are being decoded.
pub struct Turn(tokio::sync::OwnedSemaphorePermit);

/// Wait for a place among the pictures decoded at once.
pub async fn turn() -> Result<Turn> {
    let permit = DECODING
        .clone()
        .acquire_owned()
        .await
        .context("the pictures' gate is closed")?;
    Ok(Turn(permit))
}

/// Every place at once, for a test that fills the gate: held by one test at a
/// time, since two of them, each holding one of the two places, would wait
/// for each other for ever.
#[cfg(test)]
pub async fn every_turn() -> (tokio::sync::MutexGuard<'static, ()>, [Turn; AT_ONCE]) {
    static FILLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let filling = FILLING.lock().await;
    let mut turns = Vec::new();
    for _ in 0..AT_ONCE {
        turns.push(turn().await.expect("a place"));
    }
    let turns = turns.try_into().ok().expect("as many places as there are");
    (filling, turns)
}

impl Turn {
    /// Run `work` — a decoding, a resizing, an encoding — off the server's
    /// threads, holding this place until it is done, and a decoder that
    /// panics on the bytes it was given as a picture refused rather than a
    /// thread brought down.
    pub async fn run<T: Send + 'static>(
        self,
        work: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        run_holding(self.0, work).await
    }
}

/// Run `work` off the server's threads, as many at once as [`AT_ONCE`]
/// allows, and a decoder that panics on the bytes it was given as a picture
/// refused rather than a thread brought down.
pub async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    turn().await?.run(work).await
}

/// The same, for an AVIF copy: one at a time.
pub async fn off_thread_avif<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = AVIF
        .clone()
        .acquire_owned()
        .await
        .context("the pictures' gate is closed")?;
    run_holding(permit, work).await
}

async fn run_holding<T: Send + 'static>(
    permit: tokio::sync::OwnedSemaphorePermit,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    // Held by the work itself, so that a request given up on meanwhile does
    // not let another start beside it.
    let joined = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        std::panic::catch_unwind(AssertUnwindSafe(work))
    })
    .await
    .context("the work on the picture was lost")?;
    match joined {
        Ok(done) => done,
        Err(_) => bail!("the decoder gave up on the picture"),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 120])
        })
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
        out
    }

    fn size_of(bytes: &[u8]) -> (u32, u32) {
        let size = imagesize::blob_size(bytes).expect("a picture");
        (size.width as u32, size.height as u32)
    }

    /// The interface's ladders, as `frontend/src/lib/media.ts` has them,
    /// are the widths made, and nothing else is.
    #[test]
    fn the_widths_are_every_kinds_together_and_nothing_else() {
        let poster = [240, 480, 960];
        let backdrop = [640, 1280, 1920];
        let wide = [500, 1000];
        let logo = [400, 800];
        let still = [320, 640];
        let person = [120, 240];
        let mut all: Vec<u32> = [&poster[..], &backdrop, &wide, &logo, &still, &person].concat();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all, WIDTHS);
        for width in AVIF_POSTER_WIDTHS.iter().chain(&AVIF_BACKDROP_WIDTHS) {
            assert!(WIDTHS.contains(width));
        }
    }

    #[test]
    fn a_copy_is_never_wider_than_its_picture() {
        assert_eq!(width_made(480, 2000), Some(480));
        assert_eq!(width_made(1920, 1280), Some(1280));
        assert_eq!(
            width_made(680, 680),
            Some(680),
            "the picture's own width is asked for by a page that knows it"
        );
        assert_eq!(
            width_made(481, 2000),
            None,
            "no width the interface never asks for"
        );
        assert_eq!(width_made(10_000, 20_000), None);
        // Its own width is a copy only where it is no wider than the widest:
        // past that it would be a request for the whole of a picture of any
        // size, drawn and written again.
        assert_eq!(width_made(1920, 1920), Some(1920));
        assert_eq!(width_made(2000, 2000), None);
        assert_eq!(width_made(9_000, 9_000), None);
        assert_eq!(width_made(1920, 3000), Some(1920));
    }

    #[test]
    fn each_width_is_made_at_its_size_as_a_webp() {
        let decoded = Decoded::read(&png(1000, 1500)).unwrap();
        for (asked, made) in [(240, (240, 360)), (480, (480, 720)), (960, (960, 1440))] {
            let copy = make(&decoded, Made::Width(asked), Format::Webp, None).unwrap();
            assert_eq!(copy.made, Made::Width(asked));
            assert_eq!((copy.width, copy.height), made);
            assert_eq!(size_of(&copy.bytes), made);
            assert_eq!(&copy.bytes[8..16], b"WEBPVP8 ", "lossy");
            assert_eq!(copy.key("ab"), format!("ab-{asked}.webp"));
        }
        // Past its own width, a picture is copied at its own.
        let copy = make(&decoded, Made::Width(1920), Format::Webp, None).unwrap();
        assert_eq!(
            (copy.made, copy.width, copy.height),
            (Made::Width(1000), 1000, 1500)
        );
    }

    /// Drawn art — a logo — is copied without loss, its transparency
    /// exact; a photograph transparent somewhere keeps its transparency
    /// exact beside lossy colours, a quarter of the weight.
    #[test]
    fn a_transparent_picture_keeps_its_transparency() {
        let mut logo = image::RgbaImage::from_pixel(800, 310, image::Rgba([250, 250, 250, 255]));
        for x in 0..400 {
            for y in 0..310 {
                logo.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
        let mut bytes = Vec::new();
        logo.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let decoded = Decoded::read(&bytes).unwrap();
        assert!(decoded.alpha);
        let copy = make(
            &decoded,
            Made::Width(400),
            Format::Webp,
            Some(CoverType::Clearlogo),
        )
        .unwrap();
        assert_eq!(&copy.bytes[8..16], b"WEBPVP8L", "lossless");
        let back = image::load_from_memory(&copy.bytes).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (400, 155));
        assert_eq!(back.get_pixel(20, 70)[3], 0, "transparent where it was");
        assert_eq!(back.get_pixel(380, 70)[3], 255, "opaque where it was");

        // The same as a poster: lossy colours, with their transparency kept
        // apart, whole.
        let copy = make(
            &decoded,
            Made::Width(400),
            Format::Webp,
            Some(CoverType::Poster),
        )
        .unwrap();
        assert_eq!(&copy.bytes[8..16], b"WEBPVP8X", "lossy, with alpha");
        let back = image::load_from_memory(&copy.bytes).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(20, 70)[3], 0, "transparent where it was");
        assert_eq!(back.get_pixel(380, 70)[3], 255, "opaque where it was");

        // An alpha channel opaque everywhere is no transparency, and the
        // colours are what they were, closed up where they lay.
        let mut opaque = Vec::new();
        image::RgbaImage::from_fn(50, 40, |x, y| {
            image::Rgba([(x * 5) as u8, (y * 6) as u8, 3, 255])
        })
        .write_to(&mut Cursor::new(&mut opaque), image::ImageFormat::Png)
        .unwrap();
        let decoded = Decoded::read(&opaque).unwrap();
        assert!(!decoded.alpha);
        let pixels = decoded.as_pixels();
        assert_eq!((pixels.data.len(), pixels.channels()), (50 * 40 * 3, 3));
        assert_eq!(&pixels.data[(7 * 50 + 9) * 3..][..3], [45, 42, 3]);
        assert_eq!(&pixels.data[(39 * 50 + 49) * 3..][..3], [245, 234, 3]);
    }

    /// RGBA made RGB where it lies keeps every pixel's colours, in order.
    #[test]
    fn the_alpha_is_dropped_where_the_pixels_lie() {
        let mut pixels = vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255];
        drop_alpha(&mut pixels);
        assert_eq!(pixels, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
        let mut none: Vec<u8> = Vec::new();
        drop_alpha(&mut none);
        assert!(none.is_empty());
    }

    /// A copy at the picture's own size draws the picture itself: no second
    /// buffer of its pixels beside the first; any other size is resized.
    #[test]
    fn a_copy_at_the_pictures_own_size_copies_none_of_its_pixels() {
        let decoded = Decoded::read(&png(300, 200)).unwrap();
        let mut resized = None;
        let same = drawn_at(&decoded, 300, 200, &mut resized).unwrap();
        assert!(std::ptr::eq(same, &decoded));
        assert!(resized.is_none());
        let smaller = drawn_at(&decoded, 150, 100, &mut resized).unwrap();
        assert!(!std::ptr::eq(smaller, &decoded));
        assert_eq!((smaller.width, smaller.height), (150, 100));
        // And the copy made is the same as before it was spared.
        let copy = make(&decoded, Made::Width(300), Format::Webp, None).unwrap();
        assert_eq!((copy.width, copy.height), (300, 200));
        assert_eq!(size_of(&copy.bytes), (300, 200));
    }

    #[test]
    fn the_square_is_cut_from_the_middle_of_a_background() {
        let decoded = Decoded::read(&png(1920, 1080)).unwrap();
        let square = make(&decoded, Made::Square, Format::Jpeg, None).unwrap();
        assert_eq!((square.width, square.height), (1080, 1080));
        assert_eq!(size_of(&square.bytes), (1080, 1080));
        assert_eq!(&square.bytes[..2], b"\xff\xd8");

        let small = Decoded::read(&png(600, 400)).unwrap();
        let square = make(&small, Made::Square, Format::Jpeg, None).unwrap();
        assert_eq!(
            (square.width, square.height),
            (400, 400),
            "never larger than it is"
        );

        let poster = Decoded::read(&png(400, 600)).unwrap();
        assert!(make(&poster, Made::Square, Format::Jpeg, None).is_err());
        assert!(make(&decoded, Made::Square, Format::Webp, None).is_err());
        assert!(make(&decoded, Made::Own, Format::Webp, None).is_err());
    }

    #[test]
    fn a_picture_is_prepared_with_the_copy_its_kind_is_first_asked_for() {
        let poster = png(1000, 1500);
        let prepared = prepare(&poster, Some(CoverType::Poster), true).unwrap();
        assert!(prepared.placeholder.is_some());
        assert_eq!(prepared.copies.len(), 1);
        assert_eq!(
            (prepared.copies[0].width, prepared.copies[0].height),
            (480, 720)
        );
        assert_eq!(prepared.copies[0].format, Format::Webp);

        let backdrop = png(1600, 900);
        let prepared = prepare(&backdrop, Some(CoverType::Fanart), true).unwrap();
        assert_eq!(prepared.copies[0].width, 1280);

        let still = prepare(&png(640, 360), Some(CoverType::Screenshot), true).unwrap();
        assert!(still.copies.is_empty() && still.placeholder.is_some());

        assert!(prepare(b"not a picture", Some(CoverType::Poster), true).is_err());
    }

    /// With the WebP copies off, a picture is prepared all the same — its
    /// placeholder worked out — and no copy is made of it.
    #[test]
    fn a_picture_is_prepared_without_a_copy_while_the_webp_copies_are_off() {
        for (picture, cover) in [
            (png(1000, 1500), CoverType::Poster),
            (png(1600, 900), CoverType::Fanart),
        ] {
            let prepared = prepare(&picture, Some(cover), false).unwrap();
            assert!(prepared.placeholder.is_some(), "{cover:?}");
            assert!(prepared.copies.is_empty(), "{cover:?}");
        }
        assert!(prepare(b"not a picture", Some(CoverType::Poster), false).is_err());
    }

    /// Pictures decoding together hold no more than the budget: one that
    /// does not fit waits for those that do to be done, and those behind it
    /// wait their turn, though they would fit — so that a large picture is
    /// not kept waiting by a string of small ones. A share is given back
    /// when it is dropped.
    #[test]
    fn pictures_decoding_together_hold_no_more_than_the_budget_and_wait_in_turn() {
        use std::{sync::mpsc, thread, time::Duration};

        let budget = Arc::new(Budget::new(100));
        let first = budget.take(60);
        let held = |budget: &Budget| budget.shares.lock().unwrap().used;
        assert_eq!(held(&budget), 60);

        let (told, heard) = mpsc::channel();
        let ask = |name: &'static str, bytes: u64| {
            let (budget, told) = (budget.clone(), told.clone());
            thread::spawn(move || {
                let share = budget.take(bytes);
                told.send(name).unwrap();
                // Held a moment, as a picture is while it is decoded.
                thread::sleep(Duration::from_millis(50));
                drop(share);
            })
        };
        // 60 does not fit beside 60; 30 would, and is behind it.
        let large = ask("large", 60);
        thread::sleep(Duration::from_millis(100));
        let small = ask("small", 30);
        assert!(
            heard.recv_timeout(Duration::from_millis(300)).is_err(),
            "nobody goes while the first holds its share, and the small one is in turn"
        );
        assert_eq!(held(&budget), 60);

        drop(first);
        let mut told_in_the_end = vec![
            heard.recv_timeout(Duration::from_secs(5)).unwrap(),
            heard.recv_timeout(Duration::from_secs(5)).unwrap(),
        ];
        told_in_the_end.sort_unstable();
        assert_eq!(told_in_the_end, ["large", "small"]);
        large.join().unwrap();
        small.join().unwrap();
        assert_eq!(held(&budget), 0, "every share is given back");

        // A share larger than the whole is the whole, and never waits for
        // more than there is.
        let all = budget.take(500);
        assert_eq!(all.bytes, 100);
        assert_eq!(held(&budget), 100);
        drop(all);
        assert_eq!(held(&budget), 0);
    }

    /// What a picture reserves is its worst case from the header, taken
    /// before it is decoded, held for as long as the picture is and given
    /// back with it.
    #[test]
    fn what_a_picture_reserves_is_given_back_once_it_is_dropped() {
        let budget = Arc::new(Budget::new(file::DECODE_BUDGET));
        let used = || budget.shares.lock().unwrap().used;
        let bytes = png(64, 48);
        let (picture, held) =
            file::decode_with(&bytes, true, |weight| budget.take(weight)).unwrap();
        assert_eq!((picture.width(), picture.height()), (64, 48));
        // 3 072 pixels of RGB, 9 bytes each with the copy to be made of
        // them, the file's own bytes and the fixed ones.
        assert_eq!(
            held.bytes,
            64 * 48 * 9 + bytes.len() as u64 + 8 * 1024 * 1024
        );
        assert_eq!(used(), held.bytes);
        drop(held);
        assert_eq!(used(), 0);

        // A picture read for the copies carries its share, a copy of it
        // made smaller none, being within it.
        let decoded = Decoded::read(&bytes).unwrap();
        assert!(decoded._held.is_some());
        assert!(decoded.resized(32, 24).unwrap()._held.is_none());
    }

    #[tokio::test]
    async fn a_turn_is_waited_for_and_held_by_the_work() {
        // Every place taken: another waits.
        let (filling, [one, two]) = every_turn().await;
        let mut third = Box::pin(turn());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut third)
                .await
                .is_err(),
            "no more than {AT_ONCE} are decoded at once"
        );
        // The work holds its place until it is done, whoever stops waiting.
        let done = one.run(|| Ok(1)).await.unwrap();
        assert_eq!(done, 1);
        let third = tokio::time::timeout(std::time::Duration::from_secs(5), third)
            .await
            .expect("a place freed by the work")
            .unwrap();
        drop((two, third, filling));
    }

    #[tokio::test]
    async fn work_that_panics_is_a_picture_refused() {
        let refused = off_thread(|| -> Result<()> { panic!("a decoder's bug") }).await;
        assert!(refused.unwrap_err().to_string().contains("gave up"));
        assert_eq!(off_thread(|| Ok(7)).await.unwrap(), 7);
    }

    #[test]
    fn a_placeholder_drawn_from_a_small_decode_is_the_one_drawn_from_the_whole() {
        // A poster: two thirds of a dark blue, a band of orange, as a JPEG.
        let mut bytes = Vec::new();
        image::RgbImage::from_fn(2000, 3000, |_, y| {
            image::Rgb(if y < 2000 {
                [20, 40, 90]
            } else {
                [230, 120, 30]
            })
        })
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .unwrap();

        let small = Decoded::read_small(&bytes, super::super::placeholder::DRAWN_FROM).unwrap();
        assert_eq!((small.width, small.height), (250, 375));
        let small = super::super::placeholder::of(&small).unwrap();
        let whole = super::super::placeholder::of(&Decoded::read(&bytes).unwrap()).unwrap();

        // The colour within what a JPEG's rounding moves, the sketch's
        // proportions and average alike.
        let near = |a: u8, b: u8| a.abs_diff(b) <= 3;
        assert!(
            (0..3).all(|c| near(small.color[c], whole.color[c])),
            "{:?} {:?}",
            small.color,
            whole.color
        );
        let ratio = |hash: &[u8]| thumbhash::thumb_hash_to_approximate_aspect_ratio(hash).unwrap();
        assert!((ratio(&small.thumbhash) - ratio(&whole.thumbhash)).abs() < 0.01);
        let average = |hash: &[u8]| thumbhash::thumb_hash_to_average_rgba(hash).unwrap();
        let (sa, wa) = (average(&small.thumbhash), average(&whole.thumbhash));
        assert!(
            (sa.0 - wa.0).abs() < 0.02 && (sa.1 - wa.1).abs() < 0.02 && (sa.2 - wa.2).abs() < 0.02
        );

        // And what prepares a picture with no copy to make draws it so.
        let prepared = prepare(&bytes, Some(CoverType::Poster), false).unwrap();
        assert_eq!(prepared.placeholder, Some(small));
        assert!(prepared.copies.is_empty());
    }
}
