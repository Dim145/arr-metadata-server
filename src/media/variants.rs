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
    sync::{Arc, LazyLock},
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
/// picture's own — `None` for a width the interface never asks for.
pub fn width_made(asked: u32, own: u32) -> Option<u32> {
    (WIDTHS.contains(&asked) || asked == own).then(|| asked.min(own))
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
}

impl Decoded {
    /// Read a picture's bytes — see [`file::decode`] for the bounds.
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let picture = file::decode(bytes)?;
        let (width, height) = (picture.width(), picture.height());
        // An alpha channel that is opaque everywhere is a photograph like
        // any other; only real transparency is kept.
        if picture.color().has_alpha() {
            let rgba = picture.into_rgba8().into_raw();
            if rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255) {
                return Ok(Self {
                    pixels: rgba,
                    width,
                    height,
                    alpha: true,
                });
            }
            let rgb = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            return Ok(Self {
                pixels: rgb,
                width,
                height,
                alpha: false,
            });
        }
        Ok(Self {
            pixels: picture.into_rgb8().into_raw(),
            width,
            height,
            alpha: false,
        })
    }

    fn as_pixels(&self) -> Pixels<'_> {
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
        }
    }

    /// Whether it is taller than it is wide: a poster's lettering is what
    /// libwebp's sharper colour conversion keeps.
    fn is_portrait(&self) -> bool {
        self.height > self.width
    }

    /// RGBA, whatever it is: what the placeholder is worked out from.
    pub fn rgba(&self) -> Vec<u8> {
        if self.alpha {
            return self.pixels.clone();
        }
        self.pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    }
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
            let small = decoded.resized(width, height)?;
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
    let decoded = Decoded::read(bytes)?;
    let placeholder = super::placeholder::of(&decoded)
        .inspect_err(|e| tracing::debug!(error = format_args!("{e:#}"), "no placeholder"))
        .ok();
    let mut copies = Vec::new();
    if webp && let Some(width) = eager_width(cover) {
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
/// on its first visit are a dozen at a time.
const AT_ONCE: usize = 2;
static DECODING: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(AT_ONCE)));
/// The night's AVIF copies, one at a time and apart from the rest: one
/// takes seconds, and the copies a page waits for are not to wait on it.
static AVIF: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

/// Run `work` — a decoding, a resizing, an encoding — off the server's
/// threads, as many at once as [`AT_ONCE`] allows, and a decoder that
/// panics on the bytes it was given as a picture refused rather than a
/// thread brought down.
pub async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    run_under(&DECODING, work).await
}

/// The same, for an AVIF copy: one at a time.
pub async fn off_thread_avif<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    run_under(&AVIF, work).await
}

async fn run_under<T: Send + 'static>(
    gate: &Arc<tokio::sync::Semaphore>,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    // Held by the work itself, so that a request given up on meanwhile does
    // not let another start beside it.
    let permit = gate
        .clone()
        .acquire_owned()
        .await
        .context("the pictures' gate is closed")?;
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

        // An alpha channel opaque everywhere is no transparency.
        let mut opaque = Vec::new();
        image::RgbaImage::from_pixel(50, 50, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut Cursor::new(&mut opaque), image::ImageFormat::Png)
            .unwrap();
        assert!(!Decoded::read(&opaque).unwrap().alpha);
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

    #[tokio::test]
    async fn work_that_panics_is_a_picture_refused() {
        let refused = off_thread(|| -> Result<()> { panic!("a decoder's bug") }).await;
        assert!(refused.unwrap_err().to_string().contains("gave up"));
        assert_eq!(off_thread(|| Ok(7)).await.unwrap(), 7);
    }
}
