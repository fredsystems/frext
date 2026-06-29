//! The application window icon, embedded at compile time.
//!
//! The bytes are the 256x256 entry of the hicolor icon tree under
//! `assets/icons/hicolor`, the same artwork the installed `.desktop` entry
//! points at. Embedding it lets the running window carry its icon even when
//! launched outside a desktop environment that would resolve the `.desktop`
//! entry.

/// The raw PNG bytes of the window icon (256x256).
const ICON_PNG: &[u8] = include_bytes!("../../assets/icons/hicolor/256x256/apps/frext.png");

/// Decode the embedded window icon into an [`egui::IconData`].
///
/// Returns `None` (and logs) if the embedded PNG cannot be decoded, so a
/// decode failure leaves the window iconless rather than crashing the editor.
#[must_use]
pub fn window_icon() -> Option<egui::IconData> {
    match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => Some(icon),
        Err(err) => {
            log::error!("failed to decode embedded window icon: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    // `expect` is acceptable in test code: a panic on an unexpected `None`
    // is exactly the failure signal we want from a test.
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn embedded_icon_decodes_to_a_square_rgba_image() {
        let icon = window_icon().expect("embedded icon must decode");

        // The asset is the 256x256 hicolor entry.
        assert_eq!(icon.width, 256);
        assert_eq!(icon.height, 256);

        // RGBA: four bytes per pixel, and non-empty.
        assert_eq!(
            icon.rgba.len(),
            (icon.width as usize) * (icon.height as usize) * 4
        );
        assert!(!icon.rgba.is_empty());
    }
}
