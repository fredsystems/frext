//! Rasterising vendored SVG icons for kitty images.
//!
//! The kitty graphics protocol's `f=32` format carries raw, straight
//! (not premultiplied) RGBA pixels. [`rasterize`] draws an icon's SVG into
//! a transparent canvas of the requested size and returns exactly those
//! bytes.

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

/// Rasterises `svg` into a `width` x `height` transparent canvas.
///
/// The SVG is scaled, preserving its aspect ratio, to fit a square whose
/// side is `min(width, height)`, and that square is centred in the canvas.
/// The result is straight (not premultiplied) RGBA, row-major, exactly
/// `width * height * 4` bytes long.
///
/// Returns `None` when either side is zero or larger than `u16::MAX`
/// (icons are a couple of terminal cells, so the limit is never reached in
/// practice and keeps the size conversions lossless), when the SVG does
/// not parse, or when the pixmap cannot be created.
#[must_use]
pub fn rasterize(svg: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let canvas_w = f32::from(u16::try_from(width).ok()?);
    let canvas_h = f32::from(u16::try_from(height).ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    let tree = Tree::from_data(svg, &Options::default()).ok()?;
    let mut pixmap = Pixmap::new(width, height)?;

    let side = canvas_w.min(canvas_h);
    let size = tree.size();
    let scale = side / size.width().max(size.height());
    let drawn_w = size.width() * scale;
    let drawn_h = size.height() * scale;
    let transform = Transform::from_scale(scale, scale)
        .post_translate((canvas_w - drawn_w) / 2.0, (canvas_h - drawn_h) / 2.0);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    Some(rgba)
}

#[cfg(test)]
mod tests {
    // Tests unwrap fixtures that are known to be present and valid.
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn rust_icon() -> &'static [u8] {
        frext_core::file_icon::icon_bytes("rust").unwrap()
    }

    fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let index = usize::try_from((y * width + x) * 4).unwrap();
        let bytes = rgba.get(index..index + 4).unwrap();
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    }

    fn near(a: u8, b: u8) -> bool {
        a.abs_diff(b) <= 2
    }

    #[test]
    fn rust_icon_square() {
        let rgba = rasterize(rust_icon(), 20, 20).unwrap();
        assert_eq!(rgba.len(), 1600);
        for (x, y) in [(0, 0), (19, 0), (0, 19), (19, 19)] {
            assert_eq!(pixel(&rgba, 20, x, y)[3], 0, "corner ({x}, {y})");
        }
        let has_peach = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] == 255 && near(p[0], 0xfa) && near(p[1], 0xb3) && near(p[2], 0x87));
        assert!(has_peach, "no opaque #fab387 pixel");
    }

    #[test]
    fn rust_icon_wide_is_centred() {
        let rgba = rasterize(rust_icon(), 40, 20).unwrap();
        assert_eq!(rgba.len(), 40 * 20 * 4);
        let mut middle_opaque = false;
        for y in 0..20 {
            for x in 0..40 {
                let alpha = pixel(&rgba, 40, x, y)[3];
                if !(10..30).contains(&x) {
                    assert_eq!(alpha, 0, "pixel ({x}, {y}) outside the square");
                } else if alpha == 255 {
                    middle_opaque = true;
                }
            }
        }
        assert!(middle_opaque, "no opaque pixel in the centred square");
    }

    #[test]
    fn straight_alpha() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#ff0000" fill-opacity="0.5"/></svg>"##;
        let rgba = rasterize(svg, 4, 4).unwrap();
        assert_eq!(rgba.len(), 64);
        for p in rgba.as_chunks::<4>().0 {
            assert_eq!(&p[..3], &[255, 0, 0]);
            assert!((126..=129).contains(&p[3]), "alpha {}", p[3]);
        }
    }

    #[test]
    fn invalid_input_is_none() {
        assert_eq!(rasterize(b"not an svg \xff\x00", 20, 20), None);
        assert_eq!(rasterize(rust_icon(), 0, 20), None);
        assert_eq!(rasterize(rust_icon(), 20, 0), None);
        assert_eq!(rasterize(rust_icon(), 70_000, 20), None);
    }
}
