//! Avatar pictures: decode what a player uploads, make a 256 by 256 square of it and encode that
//! again, so that nothing of the original file (metadata, odd chunks, other formats hiding in
//! it) is kept.
//!
//! The decoders are `image`'s PNG and JPEG decoders, written in Rust. A small file can claim a
//! huge picture, so the dimensions and the memory the decoder may use are limited before it
//! allocates anything.

use std::io::Cursor;

use anyplay_api::AVATAR_SIZE;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder as _, ImageError, ImageFormat, ImageReader, Limits};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;

use crate::error::AppError;

/// The widest and tallest picture that is decoded, in pixels.
pub const MAX_SIDE: u32 = 4096;
/// The most memory a decoder may ask for, in bytes.
pub const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
/// How many pictures one node decodes at a time; the others wait. Decoding is the one costly
/// thing a player can make the cloud do.
const DECODERS: usize = 4;

static SLOTS: Semaphore = Semaphore::const_new(DECODERS);

/// A finished avatar.
#[derive(Debug, PartialEq, Eq)]
pub struct Avatar {
    /// The SHA-256 of `png`, as 64 lowercase hex digits.
    pub hash: String,
    pub png: Vec<u8>,
}

/// Why a picture was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Neither a PNG nor a JPEG, or not the type the request declared.
    Type,
    /// More pixels or memory than the limits allow.
    Size,
    /// A PNG or JPEG that cannot be decoded.
    Broken,
}

impl From<Refused> for AppError {
    fn from(refused: Refused) -> Self {
        match refused {
            Refused::Type => AppError::bad_request("Send a PNG or JPEG picture."),
            Refused::Size => AppError::too_large(format!(
                "The picture may have at most {MAX_SIDE} pixels on a side."
            )),
            Refused::Broken => AppError::bad_request("The picture cannot be read."),
        }
    }
}

/// The format `bytes` really is, from its first bytes: `Png` or `Jpeg`, nothing else.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => Some(format),
        _ => None,
    }
}

/// Makes the avatar of an uploaded picture. `declared` is the format the request's content type
/// named; the bytes must be that format. This is CPU work; [`make`] runs it off the async threads.
pub fn process(bytes: &[u8], declared: ImageFormat) -> Result<Avatar, Refused> {
    if sniff(bytes) != Some(declared) {
        return Err(Refused::Type);
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), declared);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(refusal)?;
    // The decoder's own memory limit does not see the whole picture, so the size of the decoded
    // picture is checked here, before any of it is read.
    if decoder.total_bytes() > MAX_DECODE_BYTES {
        return Err(Refused::Size);
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = DynamicImage::from_decoder(decoder).map_err(refusal)?;
    picture.apply_orientation(orientation);

    let (width, height) = (picture.width(), picture.height());
    let side = width.min(height);
    if side == 0 {
        return Err(Refused::Broken);
    }
    let square = picture.crop_imm((width - side) / 2, (height - side) / 2, side, side);
    let small = square.resize_exact(AVATAR_SIZE, AVATAR_SIZE, FilterType::Lanczos3);
    // Pixels only: the encoder writes the header, the data and the end, nothing else.
    let small = if small.color().has_alpha() {
        DynamicImage::ImageRgba8(small.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(small.to_rgb8())
    };
    let mut png = Vec::new();
    small
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|_| Refused::Broken)?;
    let hash = data_hex(&Sha256::digest(&png));
    Ok(Avatar { hash, png })
}

fn refusal(error: ImageError) -> Refused {
    match error {
        ImageError::Limits(_) => Refused::Size,
        ImageError::Unsupported(_) => Refused::Type,
        _ => Refused::Broken,
    }
}

fn data_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(64), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    })
}

/// [`process`] on a blocking thread, a few at a time on each node.
pub async fn make(bytes: Vec<u8>, declared: ImageFormat) -> Result<Avatar, AppError> {
    let _slot = SLOTS.acquire().await.map_err(|_| AppError::internal())?;
    tokio::task::spawn_blocking(move || process(&bytes, declared))
        .await
        .map_err(|_| AppError::internal())?
        .map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder as _, RgbImage};

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let picture = RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut bytes)
            .write_image(
                picture.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_picture_becomes_a_square_png_of_the_set_size() {
        let avatar = process(&jpeg(300, 120), ImageFormat::Jpeg).unwrap();
        assert_eq!(avatar.hash.len(), 64);
        assert_eq!(sniff(&avatar.png), Some(ImageFormat::Png));
        let decoded = image::load_from_memory(&avatar.png).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (AVATAR_SIZE, AVATAR_SIZE)
        );
        // The same input gives the same hash.
        assert_eq!(avatar, process(&jpeg(300, 120), ImageFormat::Jpeg).unwrap());
    }

    #[test]
    fn the_declared_type_must_be_the_real_one() {
        assert_eq!(
            process(&jpeg(8, 8), ImageFormat::Png).unwrap_err(),
            Refused::Type
        );
        assert_eq!(
            process(b"GIF89a....", ImageFormat::Png).unwrap_err(),
            Refused::Type
        );
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(data_hex(&[0, 15, 255]), "000fff");
    }
}
