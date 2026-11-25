//! The cover inside the file: the first picture of a list that loads, as a
//! square of at most 1200×1200: the originals reach 3543×3543 and 5 MB, a
//! copy in every track.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader};
use lofty::picture::MimeType;

/// The largest side of a cover, in pixels.
pub const COVER_SIZE: u32 = 1200;
/// JPEG quality of a cover that has to be encoded anew.
const QUALITY: u8 = 90;

/// A picture ready to go into the tags.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cover {
    pub bytes: Vec<u8>,
    pub mime: MimeType,
}

/// `bytes` as a square cover. A square JPEG or PNG no larger than
/// [`COVER_SIZE`] is kept as it is; anything else — a video's 16:9 frame,
/// a larger picture, WebP — becomes its middle square, scaled down to
/// [`COVER_SIZE`], as JPEG.
pub(crate) fn square(bytes: &[u8]) -> Result<Cover, image::ImageError> {
    let format = image::guess_format(bytes)?;
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format).into_dimensions()?;
    let mime = match format {
        ImageFormat::Jpeg => Some(MimeType::Jpeg),
        ImageFormat::Png => Some(MimeType::Png),
        _ => None,
    };
    if let Some(mime) = mime
        && width == height
        && width <= COVER_SIZE
    {
        return Ok(Cover {
            bytes: bytes.to_vec(),
            mime,
        });
    }
    let picture = image::load_from_memory_with_format(bytes, format)?;
    let side = width.min(height);
    let mut picture = picture.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    if side > COVER_SIZE {
        picture = picture.resize_exact(COVER_SIZE, COVER_SIZE, FilterType::Lanczos3);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, QUALITY).encode_image(&picture.to_rgb8())?;
    Ok(Cover {
        bytes: out,
        mime: MimeType::Jpeg,
    })
}

/// The cover from the first of `urls` that loads and is a picture; `None`
/// when none does (the track is saved without a cover).
pub(crate) async fn fetch(http: &reqwest::Client, urls: &[String]) -> Option<Cover> {
    for url in urls {
        let Ok(response) = http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        else {
            continue;
        };
        let Ok(bytes) = response.bytes().await else {
            continue;
        };
        if let Ok(Ok(cover)) = tokio::task::spawn_blocking(move || square(&bytes)).await {
            return Some(cover);
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use image::{DynamicImage, ImageEncoder, RgbImage};

    use super::*;
    use crate::fetch::tests::range_server;

    /// A JPEG of `width`×`height`, red on the left half and blue on the right.
    pub(crate) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let picture = RgbImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                image::Rgb([200, 20, 20])
            } else {
                image::Rgb([20, 20, 200])
            }
        });
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&picture)
            .unwrap();
        out
    }

    fn size_of(cover: &Cover) -> (u32, u32) {
        ImageReader::new(Cursor::new(&cover.bytes))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
    }

    #[test]
    fn a_square_cover_is_kept_as_it_is() {
        let bytes = jpeg(544, 544);
        let cover = square(&bytes).unwrap();
        assert_eq!(
            cover,
            Cover {
                bytes,
                mime: MimeType::Jpeg
            }
        );
    }

    #[test]
    fn a_video_frame_becomes_its_middle_square() {
        let cover = square(&jpeg(1280, 720)).unwrap();
        assert_eq!((size_of(&cover), cover.mime), ((720, 720), MimeType::Jpeg));
        // The middle of the frame: both halves are still there.
        let picture = image::load_from_memory(&cover.bytes).unwrap().to_rgb8();
        assert!(picture.get_pixel(10, 360)[0] > 150);
        assert!(picture.get_pixel(710, 360)[2] > 150);
    }

    #[test]
    fn a_large_cover_is_scaled_down() {
        let mut png = Vec::new();
        let picture = DynamicImage::ImageRgb8(RgbImage::new(2000, 2000));
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                picture.as_bytes(),
                2000,
                2000,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        let cover = square(&png).unwrap();
        assert_eq!(
            (size_of(&cover), cover.mime),
            ((COVER_SIZE, COVER_SIZE), MimeType::Jpeg)
        );
        assert!(square(b"not a picture").is_err());
    }

    #[tokio::test]
    async fn the_first_cover_that_loads_is_taken() {
        let closed = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap()
        };
        let (text, _) = range_server(b"<html>not a picture</html>".to_vec()).await;
        let (picture, _) = range_server(jpeg(300, 300)).await;
        let urls = [
            format!("http://{closed}/gone.jpg"),
            format!("http://{text}/page"),
            format!("http://{picture}/cover.jpg"),
        ];
        let http = reqwest::Client::new();
        let cover = fetch(&http, &urls).await.unwrap();
        assert_eq!(size_of(&cover), (300, 300));
        assert_eq!(fetch(&http, &urls[..2]).await, None);
    }
}
