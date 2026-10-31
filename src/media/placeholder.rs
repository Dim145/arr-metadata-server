//! What a picture is drawn as before it is there: the colour most of it is,
//! painted at once behind its frame, and a ThumbHash — a blurred sketch of
//! it in two dozen bytes, its proportions and its transparency included —
//! which the interface draws under the real picture until it arrives.
//!
//! Worked out once, as the picture is stored, and kept with it. Integer
//! arithmetic to the sketch, so that the same picture gives the same colour
//! on every machine, whatever its processor.

use anyhow::{Result, ensure};
use base64::Engine as _;

use super::variants::Decoded;

/// The largest side ThumbHash takes; a sketch larger gives it nothing more.
const SKETCH_SIDE: u32 = 100;

/// The side a picture is decoded at, at the least, for its sketch alone:
/// twice the sketch's, so that each of its pixels is the average of four of
/// the picture's at the least.
pub const DRAWN_FROM: u32 = 2 * SKETCH_SIDE;

/// A picture's placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placeholder {
    pub color: [u8; 3],
    pub thumbhash: Vec<u8>,
}

impl Placeholder {
    /// The colour as CSS writes it: `#1a2b3c`, seven bytes.
    pub fn color_hex(&self) -> String {
        let [r, g, b] = self.color;
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// The ThumbHash as the native API carries it: standard base64.
    pub fn thumbhash_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.thumbhash)
    }
}

/// A colour as CSS writes it, read back: `#1a2b3c`.
pub fn parse_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// A ThumbHash as the native API carries it, read back.
pub fn parse_thumbhash(text: &str) -> Option<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    (5..=32).contains(&bytes.len()).then_some(bytes)
}

/// The placeholder of a decoded picture.
pub fn of(decoded: &Decoded) -> Result<Placeholder> {
    let rgba = decoded.rgba();
    let (width, height) = fit(decoded.width, decoded.height);
    let sketch = area_average(&rgba, decoded.width, decoded.height, width, height)?;
    let thumbhash = thumbhash::rgba_to_thumb_hash(width as usize, height as usize, &sketch);
    let color = dominant(&sketch).unwrap_or([0, 0, 0]);
    Ok(Placeholder { color, thumbhash })
}

/// The sketch's size: the picture's proportions, its longer side no more
/// than [`SKETCH_SIDE`].
fn fit(width: u32, height: u32) -> (u32, u32) {
    let longer = width.max(height);
    if longer <= SKETCH_SIDE {
        return (width.max(1), height.max(1));
    }
    let scale = |side: u32| {
        let scaled =
            (u64::from(side) * u64::from(SKETCH_SIDE) + u64::from(longer) / 2) / u64::from(longer);
        u32::try_from(scaled).unwrap_or(1).max(1)
    };
    (scale(width), scale(height))
}

/// A picture made smaller by averaging every pixel of each area into one —
/// the colours weighed by their alpha, so that what is transparent lends
/// none of its colour — in whole numbers, the same everywhere.
fn area_average(rgba: &[u8], width: u32, height: u32, to_w: u32, to_h: u32) -> Result<Vec<u8>> {
    ensure!(
        rgba.len() == width as usize * height as usize * 4 && to_w > 0 && to_h > 0,
        "the pixels are not the size the picture says"
    );
    let (width, height, to_w, to_h) = (
        width as usize,
        height as usize,
        to_w as usize,
        to_h as usize,
    );
    let mut out = Vec::with_capacity(to_w * to_h * 4);
    for dy in 0..to_h {
        let (y0, y1) = (
            dy * height / to_h,
            ((dy + 1) * height / to_h).max(dy * height / to_h + 1),
        );
        for dx in 0..to_w {
            let (x0, x1) = (
                dx * width / to_w,
                ((dx + 1) * width / to_w).max(dx * width / to_w + 1),
            );
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for y in y0..y1.min(height) {
                for x in x0..x1.min(width) {
                    let at = (y * width + x) * 4;
                    let alpha = u64::from(rgba[at + 3]);
                    r += u64::from(rgba[at]) * alpha;
                    g += u64::from(rgba[at + 1]) * alpha;
                    b += u64::from(rgba[at + 2]) * alpha;
                    a += alpha;
                    n += 1;
                }
            }
            // Nothing drawn at all, no colour: a transparent area is black.
            let channel = |sum: u64| (sum + a / 2).checked_div(a).map_or(0, |c| c as u8);
            out.extend_from_slice(&[
                channel(r),
                channel(g),
                channel(b),
                ((a + n / 2) / n.max(1)) as u8,
            ]);
        }
    }
    Ok(out)
}

/// The colour most of a sketch is: its pixels sorted into sixteen levels a
/// channel, each weighed by its alpha — what is transparent is not drawn —
/// and the average of the fullest of those boxes. The lowest box wins a
/// tie, so the answer never depends on anything but the pixels.
fn dominant(rgba: &[u8]) -> Option<[u8; 3]> {
    let mut boxes = vec![(0u64, [0u64; 3]); 1 << 12];
    for pixel in rgba.as_chunks::<4>().0 {
        let index = (usize::from(pixel[0] >> 4) << 8)
            | (usize::from(pixel[1] >> 4) << 4)
            | usize::from(pixel[2] >> 4);
        let weight = u64::from(pixel[3]);
        let (count, sums) = &mut boxes[index];
        *count += weight;
        for channel in 0..3 {
            sums[channel] += u64::from(pixel[channel]) * weight;
        }
    }
    let (count, sums) = boxes
        .iter()
        .rev()
        .max_by_key(|(count, _)| *count)
        .filter(|(count, _)| *count > 0)?;
    Some(sums.map(|sum| ((sum + count / 2) / count) as u8))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn decoded(picture: image::RgbaImage) -> Decoded {
        let mut bytes = Vec::new();
        picture
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        Decoded::read(&bytes).unwrap()
    }

    /// A poster: two thirds of a dark blue, a band of orange.
    fn poster() -> image::RgbaImage {
        image::RgbaImage::from_fn(600, 900, |_, y| {
            if y < 600 {
                image::Rgba([20, 40, 90, 255])
            } else {
                image::Rgba([230, 120, 30, 255])
            }
        })
    }

    #[test]
    fn a_pictures_placeholder_is_the_same_every_time() {
        let first = of(&decoded(poster())).unwrap();
        let again = of(&decoded(poster())).unwrap();
        assert_eq!(first, again);

        // The colour most of it is, exactly: integer arithmetic throughout.
        assert_eq!(first.color_hex(), "#14285a");
        assert_eq!(parse_color(&first.color_hex()), Some(first.color));

        // The sketch keeps the picture's proportions and its average.
        assert!(first.thumbhash.len() <= 25, "{}", first.thumbhash.len());
        let ratio = thumbhash::thumb_hash_to_approximate_aspect_ratio(&first.thumbhash).unwrap();
        assert!((ratio - 600.0 / 900.0).abs() < 0.05, "{ratio}");
        let (r, g, b, a) = thumbhash::thumb_hash_to_average_rgba(&first.thumbhash).unwrap();
        let near = |got: f32, want: f32| (got * 255.0 - want).abs() < 6.0;
        assert!(
            near(r, 90.0) && near(g, 66.7) && near(b, 70.0) && a > 0.99,
            "{r} {g} {b} {a}"
        );
        assert_eq!(
            parse_thumbhash(&first.thumbhash_base64()).as_deref(),
            Some(&first.thumbhash[..])
        );
    }

    #[test]
    fn a_transparent_pictures_colour_is_what_is_drawn() {
        // A logo: white lettering on nothing.
        let logo = image::RgbaImage::from_fn(800, 310, |x, _| {
            if x % 4 == 0 {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let placeholder = of(&decoded(logo)).unwrap();
        assert_eq!(placeholder.color_hex(), "#ffffff");
        let (_, _, _, a) = thumbhash::thumb_hash_to_average_rgba(&placeholder.thumbhash).unwrap();
        assert!(a < 0.5, "{a}");
    }

    #[test]
    fn a_sketch_keeps_proportions_within_a_hundred_pixels() {
        assert_eq!(fit(2000, 3000), (67, 100));
        assert_eq!(fit(3840, 2160), (100, 56));
        assert_eq!(fit(40, 20), (40, 20));
        assert_eq!(fit(16_000, 10), (100, 1));
        assert!(parse_color("#12345").is_none() && parse_color("123456").is_none());
        assert!(parse_thumbhash("not base64!").is_none());
    }
}
