//! The glass look's backdrop: the current track's cover shrunk to a few
//! dozen pixels, blurred and darkened enough for light text on it, and the
//! cover's accent color. Made once per track; the window stretches the small
//! picture over itself, which smooths it further.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use image::imageops::{self, FilterType};
use image::{ImageReader, RgbImage};

/// The longer side of a backdrop, in pixels.
pub const SIDE: u32 = 64;
/// The blur's standard deviation, in pixels of the backdrop.
const BLUR_SIGMA: f32 = 3.0;
/// Every pixel's luminance is scaled by this…
const DIM: f32 = 0.55;
/// …and kept at most this (relative luminance). The window's text, #e8e8f0,
/// keeps a contrast of nearly 8:1 on the brightest pixel, the dim text,
/// #9a9aae, about 3.5:1.
const MAX_LUMINANCE: f32 = 0.06;
/// A cover with less color than this share of its pixels (weighted by
/// chroma) has no accent.
const MIN_COLOR: f32 = 0.02;
/// Backdrops kept on disk; the least recently used go beyond this.
const KEEP: usize = 100;
/// Bumped whenever the picture is made differently, so old files are not
/// taken for new ones.
const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum BackdropError {
    #[error("backdrop: {0}")]
    Io(#[from] std::io::Error),
    #[error("backdrop: {0}")]
    Image(#[from] image::ImageError),
}

/// A cover's backdrop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backdrop {
    /// The PNG file, at most [`SIDE`] pixels on the longer side.
    pub file: PathBuf,
    /// A color of the cover light enough for text and marks on the dark
    /// window, or `None` when the cover has hardly any color.
    pub accent: Option<[u8; 3]>,
}

/// Makes the backdrop of the picture in `cover` in the folder `dir`
/// (created if missing), or finds the one made before. Blocking: decodes the
/// cover and writes a file.
pub fn make(cover: &Path, dir: &Path) -> Result<Backdrop, BackdropError> {
    make_keeping(cover, dir, KEEP)
}

fn make_keeping(cover: &Path, dir: &Path, keep: usize) -> Result<Backdrop, BackdropError> {
    let picture = ImageReader::open(cover)?
        .with_guessed_format()?
        .decode()?
        .into_rgb8();
    let small = shrink(&picture);
    let accent = accent(&small);
    let stem = cover
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or("cover");
    let file = dir.join(format!("{stem}-v{VERSION}.png"));
    if file.exists() {
        touch(&file);
    } else {
        std::fs::create_dir_all(dir)?;
        let part = file.with_extension("part");
        backdrop(&small).save_with_format(&part, image::ImageFormat::Png)?;
        std::fs::rename(&part, &file)?;
        prune(dir, keep);
    }
    Ok(Backdrop { file, accent })
}

/// `picture` shrunk to at most [`SIDE`] pixels on the longer side.
fn shrink(picture: &RgbImage) -> RgbImage {
    let (w, h) = picture.dimensions();
    let longer = w.max(h).max(1);
    let scale = |side: u32| (u64::from(side) * u64::from(SIDE)).div_ceil(u64::from(longer)) as u32;
    let (new_w, new_h) = (scale(w).clamp(1, SIDE), scale(h).clamp(1, SIDE));
    imageops::resize(picture, new_w, new_h, FilterType::Triangle)
}

fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The backdrop of a shrunk cover: blurred in linear light, edges repeated
/// outwards (so they do not darken), then darkened for light text.
fn backdrop(small: &RgbImage) -> RgbImage {
    let (w, h) = (small.width() as usize, small.height() as usize);
    let pixels: Vec<[f32; 3]> = small.pixels().map(|p| p.0.map(to_linear)).collect();
    let radius = (BLUR_SIGMA * 3.0).ceil() as isize;
    let kernel: Vec<f32> = {
        let raw: Vec<f32> = (-radius..=radius)
            .map(|i| (-((i * i) as f32) / (2.0 * BLUR_SIGMA * BLUR_SIGMA)).exp())
            .collect();
        let sum: f32 = raw.iter().sum();
        raw.into_iter().map(|k| k / sum).collect()
    };
    let pass = |source: &[[f32; 3]], horizontal: bool| -> Vec<[f32; 3]> {
        let mut out = vec![[0.0; 3]; source.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                for (k, weight) in kernel.iter().enumerate() {
                    let offset = k as isize - radius;
                    let (sx, sy) = if horizontal {
                        ((x as isize + offset).clamp(0, w as isize - 1) as usize, y)
                    } else {
                        (x, (y as isize + offset).clamp(0, h as isize - 1) as usize)
                    };
                    let p = source[sy * w + sx];
                    for c in 0..3 {
                        acc[c] += p[c] * weight;
                    }
                }
                out[y * w + x] = acc;
            }
        }
        out
    };
    let blurred = pass(&pass(&pixels, true), false);
    let mut out = RgbImage::new(w as u32, h as u32);
    for (pixel, rgb) in out.pixels_mut().zip(blurred) {
        let y = luminance(rgb);
        let scale = if y > 0.0 {
            (y * DIM).min(MAX_LUMINANCE) / y
        } else {
            0.0
        };
        pixel.0 = rgb.map(|c| to_srgb(c * scale));
    }
    out
}

/// Hue in turns (0..1) of an sRGB color with the given maximum and chroma.
fn hue([r, g, b]: [f32; 3], max: f32, chroma: f32) -> f32 {
    let sixths = if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    sixths / 6.0
}

/// The cover's most prominent vivid color, made readable on the dark window:
/// hues are counted by chroma, the strongest hue's average color is taken and
/// given a fixed lightness.
fn accent(small: &RgbImage) -> Option<[u8; 3]> {
    const HUES: usize = 12;
    let mut weight = [0.0f32; HUES];
    let mut sum = [[0.0f32; 3]; HUES];
    for pixel in small.pixels() {
        let rgb = pixel.0.map(|c| f32::from(c) / 255.0);
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        let chroma = max - rgb[0].min(rgb[1]).min(rgb[2]);
        // Dark and greyish pixels say nothing of the cover's color.
        if max < 0.2 || chroma < 0.25 * max {
            continue;
        }
        let bucket = ((hue(rgb, max, chroma) * HUES as f32) as usize).min(HUES - 1);
        weight[bucket] += chroma;
        for c in 0..3 {
            sum[bucket][c] += rgb[c] * chroma;
        }
    }
    // A hue that straddles two buckets still wins: neighbours count half.
    let score =
        |i: usize| weight[i] + 0.5 * (weight[(i + HUES - 1) % HUES] + weight[(i + 1) % HUES]);
    let best = (0..HUES).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    let pixels = (small.width() * small.height()) as f32;
    if weight[best] < MIN_COLOR * pixels {
        return None;
    }
    let color = sum[best].map(|c| c / weight[best]);
    Some(readable(color))
}

/// `rgb` (sRGB, 0..1) with the lightness of a mark on the dark window and a
/// saturation neither dull nor glaring.
fn readable([r, g, b]: [f32; 3]) -> [u8; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    let chroma = max - min;
    let saturation = if chroma == 0.0 {
        0.0
    } else {
        chroma / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    let hue = if chroma == 0.0 {
        0.0
    } else {
        hue([r, g, b], max, chroma)
    };
    hsl(hue, saturation.clamp(0.45, 0.85), 0.7)
}

/// An sRGB color from hue (turns), saturation and lightness.
fn hsl(hue: f32, saturation: f32, lightness: f32) -> [u8; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sixths = hue.rem_euclid(1.0) * 6.0;
    let x = chroma * (1.0 - (sixths % 2.0 - 1.0).abs());
    let (r, g, b) = match sixths as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [r, g, b].map(|c| ((c + m).clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Marks a backdrop as just used, so pruning keeps it.
fn touch(file: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(file) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Removes the least recently used backdrops beyond `keep`.
fn prune(dir: &Path, keep: usize) {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = listing
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    let excess = files.len() - keep;
    for (_, file) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use image::Rgb;

    fn save(dir: &Path, name: &str, picture: &RgbImage) -> PathBuf {
        let file = dir.join(name);
        picture.save(&file).expect("save test picture");
        file
    }

    fn open(file: &Path) -> RgbImage {
        image::open(file).expect("open backdrop").into_rgb8()
    }

    /// Black with a red square in the top left quarter.
    fn red_on_black(side: u32) -> RgbImage {
        RgbImage::from_fn(side, side, |x, y| {
            if x < side / 2 && y < side / 2 {
                Rgb([220, 30, 40])
            } else {
                Rgb([0, 0, 0])
            }
        })
    }

    #[test]
    fn bright_covers_are_darkened_for_text() {
        let dir = tempfile::tempdir().unwrap();
        let white = RgbImage::from_pixel(120, 120, Rgb([255, 255, 255]));
        let cover = save(dir.path(), "white.png", &white);
        let made = make(&cover, &dir.path().join("backdrops")).unwrap();
        let picture = open(&made.file);
        assert_eq!(picture.dimensions(), (SIDE, SIDE));
        for pixel in picture.pixels() {
            let y = luminance(pixel.0.map(to_linear));
            assert!((y - MAX_LUMINANCE).abs() < 0.005, "pixel {pixel:?}: {y}");
        }
    }

    #[test]
    fn backdrops_are_soft_up_to_their_edges() {
        let dir = tempfile::tempdir().unwrap();
        // Grey dark enough to stay under the cap, next to black.
        let halves = RgbImage::from_fn(120, 120, |x, _| {
            if x < 60 {
                Rgb([90, 90, 90])
            } else {
                Rgb([0, 0, 0])
            }
        });
        let cover = save(dir.path(), "halves.png", &halves);
        let picture = open(&make(&cover, dir.path()).unwrap().file);
        let row: Vec<f32> = (0..SIDE)
            .map(|x| luminance(picture.get_pixel(x, SIDE / 2).0.map(to_linear)))
            .collect();
        let grey = luminance([to_linear(90); 3]) * DIM;
        // A hard edge would jump by the whole grey in one pixel.
        let steepest = row
            .windows(2)
            .map(|w| (w[0] - w[1]).abs())
            .fold(0.0, f32::max);
        assert!(
            steepest < grey / 3.0,
            "edge not blurred: {steepest} of {grey}"
        );
        // The picture's own edges are repeated outwards, not faded to black.
        assert!(
            (row[0] - grey).abs() < grey * 0.05,
            "left edge {} of {grey}",
            row[0]
        );
    }

    #[test]
    fn wide_covers_keep_their_shape() {
        let dir = tempfile::tempdir().unwrap();
        let wide = RgbImage::from_pixel(320, 180, Rgb([90, 90, 90]));
        let cover = save(dir.path(), "wide.jpg", &wide);
        let made = make(&cover, dir.path()).unwrap();
        assert_eq!(open(&made.file).dimensions(), (64, 36));
    }

    #[test]
    fn the_accent_comes_from_the_colorful_part() {
        let dir = tempfile::tempdir().unwrap();
        let cover = save(dir.path(), "red.png", &red_on_black(120));
        let [r, g, b] = make(&cover, dir.path()).unwrap().accent.expect("an accent");
        assert!(r > 200 && g < 140 && b < 140, "not red: {r} {g} {b}");
        // Light enough for marks on the dark window.
        let lightness = (f32::from(r.max(g).max(b)) + f32::from(r.min(g).min(b))) / 510.0;
        assert!((lightness - 0.7).abs() < 0.02, "lightness {lightness}");
    }

    #[test]
    fn grey_covers_have_no_accent() {
        let dir = tempfile::tempdir().unwrap();
        let grey = RgbImage::from_fn(100, 100, |x, _| {
            let v = (x * 2) as u8;
            Rgb([v, v, v.saturating_add(8)])
        });
        let cover = save(dir.path(), "grey.png", &grey);
        assert_eq!(make(&cover, dir.path()).unwrap().accent, None);
    }

    #[test]
    fn a_backdrop_is_made_once_and_old_ones_go() {
        let dir = tempfile::tempdir().unwrap();
        let backdrops = dir.path().join("backdrops");
        let first = save(dir.path(), "aaaa.png", &red_on_black(60));
        let made = make_keeping(&first, &backdrops, 2).unwrap();
        assert_eq!(made.file, backdrops.join("aaaa-v1.png"));
        // The same cover again: the file is reused, not written anew.
        std::fs::write(&made.file, b"kept").unwrap();
        assert_eq!(make_keeping(&first, &backdrops, 2).unwrap(), made);
        assert_eq!(std::fs::read(&made.file).unwrap(), b"kept");

        for name in ["bbbb.png", "cccc.png", "dddd.png"] {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let cover = save(dir.path(), name, &red_on_black(60));
            make_keeping(&cover, &backdrops, 2).unwrap();
        }
        let mut left: Vec<String> = std::fs::read_dir(&backdrops)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["cccc-v1.png", "dddd-v1.png"]);
    }

    #[test]
    fn hsl_round_trips_pure_colors() {
        assert_eq!(hsl(0.0, 1.0, 0.5), [255, 0, 0]);
        assert_eq!(hsl(1.0 / 3.0, 1.0, 0.5), [0, 255, 0]);
        assert_eq!(hsl(2.0 / 3.0, 1.0, 0.5), [0, 0, 255]);
        assert_eq!(readable([1.0, 0.0, 0.0]), hsl(0.0, 0.85, 0.7));
    }
}
