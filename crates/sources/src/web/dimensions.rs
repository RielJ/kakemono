//! Image size from the first bytes of a file, so a page's candidates can be checked without
//! downloading whole images. Only the formats kakemono sets as wallpapers count: JPEG, PNG and
//! WebP. The header parsing is `imagesize`'s; this module only narrows the formats.

use imagesize::ImageType;

/// (width, height) of a JPEG, PNG or WebP from its first bytes. None for other formats or too few bytes.
///
/// A JPEG's size sits in its SOF segment, after any EXIF/ICC segments, so a JPEG needs the whole
/// prefix up to that segment (the caller reads up to `PROBE_MAX_BYTES`).
pub(crate) fn dimensions(prefix: &[u8]) -> Option<(u32, u32)> {
    let kind = imagesize::image_type(prefix).ok()?;
    if !matches!(kind, ImageType::Jpeg | ImageType::Png | ImageType::Webp) {
        return None;
    }
    let size = imagesize::blob_size(prefix).ok()?;
    let width = u32::try_from(size.width).ok()?;
    let height = u32::try_from(size.height).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG signature plus an IHDR chunk (CRC not checked by the parser).
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }

    /// One JPEG marker segment: `FF marker`, big-endian length (including itself), payload.
    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let len = u16::try_from(payload.len() + 2).unwrap();
        let mut bytes = vec![0xFF, marker];
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    /// SOI, the given segments, then a baseline SOF0 for `width` x `height`.
    fn jpeg(width: u16, height: u16, before_sof: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        for seg in before_sof {
            bytes.extend_from_slice(seg);
        }
        let mut sof = vec![8];
        sof.extend_from_slice(&height.to_be_bytes());
        sof.extend_from_slice(&width.to_be_bytes());
        sof.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        bytes.extend_from_slice(&segment(0xC0, &sof));
        bytes
    }

    fn jfif_app0() -> Vec<u8> {
        segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0")
    }

    #[test]
    fn dimensions_should_read_png_ihdr() {
        assert_eq!(dimensions(&png(3840, 2160)), Some((3840, 2160)));
    }

    #[test]
    fn dimensions_should_read_baseline_jpeg_sof0() {
        assert_eq!(
            dimensions(&jpeg(1920, 1080, &[jfif_app0()])),
            Some((1920, 1080))
        );
    }

    #[test]
    fn dimensions_should_read_jpeg_after_100_kib_of_app_segments() {
        // One segment holds at most 65533 bytes, so ~100 KiB of EXIF/ICC takes two.
        let mut exif = b"Exif\0\0".to_vec();
        exif.resize(65_533, 0xAB);
        let icc = vec![0xCD; 40_000];
        let bytes = jpeg(
            5000,
            3000,
            &[jfif_app0(), segment(0xE1, &exif), segment(0xE2, &icc)],
        );
        assert!(bytes.len() > 100 * 1024);
        assert_eq!(dimensions(&bytes), Some((5000, 3000)));
    }

    #[test]
    fn dimensions_should_return_none_for_jpeg_cut_before_sof() {
        let mut exif = b"Exif\0\0".to_vec();
        exif.resize(65_533, 0xAB);
        let bytes = jpeg(5000, 3000, &[segment(0xE1, &exif)]);
        assert_eq!(dimensions(&bytes[..32 * 1024]), None);
    }

    #[test]
    fn dimensions_should_read_webp_vp8x() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&22u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBPVP8X");
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&(2560u32 - 1).to_le_bytes()[..3]);
        bytes.extend_from_slice(&(1440u32 - 1).to_le_bytes()[..3]);
        assert_eq!(dimensions(&bytes), Some((2560, 1440)));
    }

    #[test]
    fn dimensions_should_return_none_for_truncated_png() {
        assert_eq!(dimensions(&png(3840, 2160)[..18]), None);
    }

    #[test]
    fn dimensions_should_return_none_for_empty_input() {
        assert_eq!(dimensions(&[]), None);
    }

    #[test]
    fn dimensions_should_return_none_for_gif() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&3840u16.to_le_bytes());
        gif.extend_from_slice(&2160u16.to_le_bytes());
        gif.extend_from_slice(&[0; 8]);
        assert_eq!(dimensions(&gif), None);
    }

    #[test]
    fn dimensions_should_return_none_for_html() {
        assert_eq!(
            dimensions(b"<!DOCTYPE html><html><head><title>x</title></head></html>"),
            None
        );
    }
}
