//! Writing pixels out as the files this server makes of a picture: WebP for
//! the interface — lossy for a photograph, lossless for a picture that is
//! transparent somewhere — AVIF for the night's copies, and JPEG for the
//! square Plex is given, as every client that reads pictures reads JPEG.
//!
//! WebP goes through libwebp, called directly: its encoder, and nothing
//! else. The pixels it is given are this server's own, decoded in Rust;
//! libwebp's decoder — where CVE-2023-4863 was — is never called, so no byte
//! somebody else chose reaches C here. AVIF is rav1e's, in Rust.

use std::ffi::c_int;

use anyhow::{Result, bail, ensure};
use libwebp_sys as webp;

/// How a lossy WebP is made: libwebp's quality scale, where 75 is `cwebp`'s
/// default; 80 keeps a poster's grain and its lettering.
const WEBP_QUALITY: f32 = 80.0;
/// libwebp's effort, 0 to 6: 4 is its default, the balance between the
/// time a copy takes and its size.
const WEBP_METHOD: c_int = 4;
/// The effort of a lossless one, 0 to 9: 6 is `cwebp -z`'s default.
const WEBP_LOSSLESS_LEVEL: c_int = 6;
/// The largest side WebP can hold.
const WEBP_MAX_SIDE: u32 = 16_383;

/// How an AVIF is made: see `docs/media.md` for what these cost on real
/// posters. rav1e's speed 1 is its slowest and smallest, 10 its quickest.
pub const AVIF_QUALITY: f32 = 68.0;
pub const AVIF_SPEED: u8 = 6;

/// A JPEG's quality: `image`'s scale, the same as libjpeg's.
const JPEG_QUALITY: u8 = 85;

/// Pixels to write: rows of RGB, or of RGBA where the picture is
/// transparent somewhere, eight bits a channel.
pub struct Pixels<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
}

impl Pixels<'_> {
    fn channels(&self) -> usize {
        if self.alpha { 4 } else { 3 }
    }

    /// The pixels hold as many bytes as their size says, and the size is
    /// one the encoders take.
    fn check(&self, most: u32) -> Result<()> {
        ensure!(
            self.width > 0 && self.height > 0 && self.width <= most && self.height <= most,
            "a picture of {}×{} cannot be written",
            self.width,
            self.height
        );
        let rows = usize::try_from(self.height)?;
        let row = usize::try_from(self.width)? * self.channels();
        ensure!(
            self.data.len() == rows * row,
            "the pixels are not the size the picture says"
        );
        Ok(())
    }
}

/// A WebP: lossy, at [`WEBP_QUALITY`], its colours converted with libwebp's
/// sharper conversion where `sharp` asks — a poster's thin lettering keeps
/// its edges — or lossless, for a picture with transparency: a logo's edges
/// are its alpha, which a lossy copy would blur.
pub fn webp(pixels: &Pixels<'_>, lossless: bool, sharp: bool) -> Result<Vec<u8>> {
    pixels.check(WEBP_MAX_SIDE)?;
    let width = c_int::try_from(pixels.width)?;
    let height = c_int::try_from(pixels.height)?;
    let stride = c_int::try_from(pixels.width as usize * pixels.channels())?;
    let version = webp::WEBP_ENCODER_ABI_VERSION as c_int;

    let mut config = std::mem::MaybeUninit::<webp::WebPConfig>::uninit();
    // SAFETY: libwebp writes every field of the configuration it is handed
    // before it answers yes, and checks the version against its own.
    let ready = unsafe {
        webp::WebPConfigInitInternal(
            config.as_mut_ptr(),
            webp::WebPPreset::WEBP_PRESET_DEFAULT,
            WEBP_QUALITY,
            version,
        )
    };
    ensure!(ready != 0, "libwebp refused its own configuration");
    // SAFETY: written whole just above, and libwebp said so.
    let mut config = unsafe { config.assume_init() };
    config.method = WEBP_METHOD;
    config.use_sharp_yuv = c_int::from(sharp && !lossless);
    if lossless {
        // SAFETY: the configuration is a whole one; the preset writes a few
        // of its fields and reads nothing else.
        let set = unsafe { webp::WebPConfigLosslessPreset(&mut config, WEBP_LOSSLESS_LEVEL) };
        ensure!(set != 0, "libwebp refused its lossless preset");
    }
    // SAFETY: reads the whole configuration, which is initialised.
    ensure!(
        unsafe { webp::WebPValidateConfig(&config) } != 0,
        "libwebp refused the configuration"
    );

    let mut picture = std::mem::MaybeUninit::<webp::WebPPicture>::uninit();
    // SAFETY: as for the configuration: every field written before a yes.
    let ready = unsafe { webp::WebPPictureInitInternal(picture.as_mut_ptr(), version) };
    ensure!(ready != 0, "libwebp refused its own picture");
    // SAFETY: written whole just above. Freed by the guard from here on.
    let mut picture = Picture(unsafe { picture.assume_init() });
    picture.0.use_argb = c_int::from(lossless);
    picture.0.width = width;
    picture.0.height = height;
    // SAFETY: the buffer holds `height` rows of `stride` bytes, as checked
    // above; libwebp copies them into memory of the picture's own, which the
    // guard frees, and keeps no pointer into the buffer.
    let imported = unsafe {
        if pixels.alpha {
            webp::WebPPictureImportRGBA(&mut picture.0, pixels.data.as_ptr(), stride)
        } else {
            webp::WebPPictureImportRGB(&mut picture.0, pixels.data.as_ptr(), stride)
        }
    };
    ensure!(imported != 0, "libwebp could not take the pixels");

    // On the heap, so that the address the picture is given to write to
    // stays where it is for as long as the encoding runs.
    // SAFETY: a memory writer is pointers and sizes: all zero is a valid
    // one, and libwebp initialises it again just below.
    let mut written = Written(Box::new(unsafe { std::mem::zeroed() }));
    // SAFETY: initialises the writer it is handed.
    unsafe { webp::WebPMemoryWriterInit(&mut *written.0) };
    picture.0.writer = Some(webp::WebPMemoryWrite);
    picture.0.custom_ptr = std::ptr::from_mut::<webp::WebPMemoryWriter>(&mut *written.0).cast();
    // SAFETY: the configuration and the picture are whole and valid; the
    // picture's writer appends to the memory writer `custom_ptr` names,
    // which lives on the heap until its guard is dropped, after this call.
    let encoded = unsafe { webp::WebPEncode(&config, &mut picture.0) };
    if encoded == 0 {
        bail!(
            "libwebp could not encode the picture: {:?}",
            picture.0.error_code
        );
    }
    // SAFETY: once the encoding succeeded the writer holds `size` bytes at
    // `mem`, its own until it is cleared; they are copied out before that.
    let bytes = unsafe { std::slice::from_raw_parts(written.0.mem, written.0.size) }.to_vec();
    Ok(bytes)
}

/// A libwebp picture, freed whatever happens to the encoding.
struct Picture(webp::WebPPicture);

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: frees what libwebp allocated for this picture, once; a
        // picture that holds nothing is freed as nothing.
        unsafe { webp::WebPPictureFree(&mut self.0) }
    }
}

/// libwebp's memory writer, cleared whatever happens to the encoding.
struct Written(Box<webp::WebPMemoryWriter>);

impl Drop for Written {
    fn drop(&mut self) {
        // SAFETY: frees the memory the writer gathered, once; an empty
        // writer is cleared as nothing.
        unsafe { webp::WebPMemoryWriterClear(&mut *self.0) }
    }
}

/// An AVIF, at [`AVIF_QUALITY`] and [`AVIF_SPEED`], ten bits a channel
/// inside — what its encoder advises, eight-bit pixels or not — on one
/// thread: the night's copies are made one after another, and leave the
/// rest of the machine to the server.
pub fn avif(pixels: &Pixels<'_>) -> Result<Vec<u8>> {
    pixels.check(u32::from(u16::MAX))?;
    let (width, height) = (
        usize::try_from(pixels.width)?,
        usize::try_from(pixels.height)?,
    );
    let encoder = ravif::Encoder::new()
        .with_quality(AVIF_QUALITY)
        .with_speed(AVIF_SPEED)
        .with_bit_depth(ravif::BitDepth::Ten)
        .with_num_threads(Some(1));
    let encoded = if pixels.alpha {
        let rgba: Vec<ravif::RGBA8> = pixels
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| ravif::RGBA8::new(p[0], p[1], p[2], p[3]))
            .collect();
        encoder.encode_rgba(ravif::Img::new(&rgba[..], width, height))
    } else {
        let rgb: Vec<ravif::RGB8> = pixels
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| ravif::RGB8::new(p[0], p[1], p[2]))
            .collect();
        encoder.encode_rgb(ravif::Img::new(&rgb[..], width, height))
    };
    match encoded {
        Ok(encoded) => Ok(encoded.avif_file),
        Err(e) => bail!("the picture could not be encoded as AVIF: {e}"),
    }
}

/// A JPEG of opaque pixels, at [`JPEG_QUALITY`].
pub fn jpeg(pixels: &Pixels<'_>) -> Result<Vec<u8>> {
    ensure!(!pixels.alpha, "a JPEG has no transparency to hold");
    pixels.check(u32::from(u16::MAX))?;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode(
        pixels.data,
        pixels.width,
        pixels.height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient, so that the encoders have something to keep.
    fn gradient(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&[(x * 255 / width) as u8, (y * 255 / height) as u8, 90]);
                if alpha {
                    out.push(if x < width / 4 { 0 } else { 255 });
                }
            }
        }
        out
    }

    fn decoded(bytes: &[u8]) -> image::DynamicImage {
        image::load_from_memory(bytes).expect("a picture image reads")
    }

    #[test]
    fn a_lossy_webp_is_a_picture_of_the_size_asked() {
        let data = gradient(64, 96, false);
        let pixels = Pixels {
            data: &data,
            width: 64,
            height: 96,
            alpha: false,
        };
        for sharp in [false, true] {
            let bytes = webp(&pixels, false, sharp).expect("encoded");
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..16], b"WEBPVP8 ", "lossy");
            let back = decoded(&bytes);
            assert_eq!((back.width(), back.height()), (64, 96));
        }
    }

    #[test]
    fn a_transparent_picture_keeps_its_transparency_exactly() {
        let data = gradient(40, 20, true);
        let pixels = Pixels {
            data: &data,
            width: 40,
            height: 20,
            alpha: true,
        };
        let bytes = webp(&pixels, true, true).expect("encoded");
        assert_eq!(&bytes[8..16], b"WEBPVP8L", "lossless");
        let back = decoded(&bytes).to_rgba8();
        for (x, y, pixel) in back.enumerate_pixels() {
            let alpha = if x < 10 { 0 } else { 255 };
            assert_eq!(pixel[3], alpha, "({x}, {y})");
            if alpha == 255 {
                let at = ((y * 40 + x) * 4) as usize;
                assert_eq!(&pixel.0[..3], &data[at..at + 3], "({x}, {y})");
            }
        }
    }

    #[test]
    fn pixels_that_are_not_their_size_are_refused() {
        let data = vec![0u8; 10];
        let pixels = Pixels {
            data: &data,
            width: 4,
            height: 4,
            alpha: false,
        };
        assert!(webp(&pixels, false, false).is_err());
        assert!(jpeg(&pixels).is_err());
        assert!(avif(&pixels).is_err());
    }

    #[test]
    fn an_avif_and_a_jpeg_are_what_they_say() {
        let data = gradient(32, 24, false);
        let pixels = Pixels {
            data: &data,
            width: 32,
            height: 24,
            alpha: false,
        };
        let avif = avif(&pixels).expect("encoded");
        assert_eq!(&avif[4..12], b"ftypavif");
        let jpeg = jpeg(&pixels).expect("encoded");
        assert_eq!(&jpeg[..2], b"\xff\xd8");
        assert_eq!(decoded(&jpeg).width(), 32);
    }
}
