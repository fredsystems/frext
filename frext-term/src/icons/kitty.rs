//! kitty-image icons.
//!
//! [`KittyIcons`] draws each row's file-type SVG as a kitty image. The
//! first time a stem is needed its SVG is rasterised at
//! `2 * cell width` x `cell height` pixels (about square, since cells
//! are roughly 1:2) and uploaded once with the next image id; every
//! later row with that stem reuses the id. The tree draws the
//! unicode placeholders; the queued upload bytes are drained with
//! [`IconProvider::take_uploads`] and written before the frame that
//! shows them, and [`IconProvider::release`] deletes every image on
//! exit. A stem that cannot be drawn as an image (no SVG, a failed
//! rasterisation, or no ids left) falls back to its Nerd glyph.

use std::collections::HashMap;

use frext_core::file_icon::icon_bytes;

use super::{IconCell, IconKind, IconProvider, nerd, raster};
use crate::term::caps::{CellPx, Multiplexer};
use crate::term::kitty::{self, ImageId};

/// Rasterises an SVG to straight RGBA at the given width and height in
/// pixels, or `None` when it cannot.
pub(crate) type Rasterizer = fn(&[u8], u32, u32) -> Option<Vec<u8>>;

/// What happened to a stem's upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Upload {
    /// Uploaded as this image.
    Image(ImageId),
    /// Could not be uploaded; the Nerd glyph is used instead.
    Failed,
}

/// Why a stem could not be uploaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// There is no SVG for the stem.
    NoBytes,
    /// The SVG could not be rasterised.
    RasterFailed,
    /// Every image id has been used.
    IdsExhausted,
}

impl Failure {
    const fn reason(self) -> &'static str {
        match self {
            Self::NoBytes => "no SVG for the stem",
            Self::RasterFailed => "rasterising the SVG failed",
            Self::IdsExhausted => "no image ids left",
        }
    }
}

/// The kitty-image provider: each stem's SVG uploaded once and shown
/// through unicode placeholders, with the Nerd glyph as a fallback.
#[derive(Debug)]
pub struct KittyIcons {
    cell_px: CellPx,
    multiplexer: Multiplexer,
    uploads: HashMap<&'static str, Upload>,
    next_id: Option<ImageId>,
    queued: Vec<u8>,
    rasterize: Rasterizer,
}

impl KittyIcons {
    /// A provider for cells of `cell_px` pixels whose uploads are
    /// wrapped for `multiplexer`.
    #[must_use]
    pub fn new(cell_px: CellPx, multiplexer: Multiplexer) -> Self {
        Self::with_rasterizer(cell_px, multiplexer, raster::rasterize)
    }

    /// As [`KittyIcons::new`], rasterising with `rasterize`.
    #[must_use]
    pub(crate) fn with_rasterizer(
        cell_px: CellPx,
        multiplexer: Multiplexer,
        rasterize: Rasterizer,
    ) -> Self {
        Self {
            cell_px,
            multiplexer,
            uploads: HashMap::new(),
            next_id: ImageId::new(1),
            queued: Vec::new(),
            rasterize,
        }
    }

    /// Rasterises `stem` and queues its upload, returning its id.
    fn upload(&mut self, stem: &str) -> Result<ImageId, Failure> {
        let width = 2 * u32::from(self.cell_px.width);
        let height = u32::from(self.cell_px.height);
        let svg = icon_bytes(stem).ok_or(Failure::NoBytes)?;
        let rgba = (self.rasterize)(svg, width, height).ok_or(Failure::RasterFailed)?;
        let id = self.next_id.ok_or(Failure::IdsExhausted)?;
        self.queued
            .extend(kitty::upload(id, width, height, &rgba, self.multiplexer));
        self.next_id = id.next();
        Ok(id)
    }
}

impl IconProvider for KittyIcons {
    fn icon(&mut self, name: &str, kind: IconKind) -> IconCell {
        let stem = nerd::stem_for(name, kind);
        let upload = match self.uploads.get(stem) {
            Some(&upload) => upload,
            None => {
                let upload = match self.upload(stem) {
                    Ok(id) => Upload::Image(id),
                    Err(failure) => {
                        log::warn!(
                            "kitty icon for {stem:?} unavailable ({}); using the Nerd glyph",
                            failure.reason()
                        );
                        Upload::Failed
                    }
                };
                self.uploads.insert(stem, upload);
                upload
            }
        };
        match upload {
            Upload::Image(id) => IconCell::Image { id },
            Upload::Failed => nerd::nerd_icon(stem, kind),
        }
    }

    fn take_uploads(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.queued)
    }

    fn release(&mut self) -> Vec<u8> {
        let mut ids: Vec<ImageId> = self
            .uploads
            .values()
            .filter_map(|upload| match upload {
                Upload::Image(id) => Some(*id),
                Upload::Failed => None,
            })
            .collect();
        ids.sort_by_key(|id| id.get());
        let out = ids
            .into_iter()
            .flat_map(|id| kitty::delete(id, self.multiplexer))
            .collect();
        self.uploads.clear();
        self.queued.clear();
        out
    }
}

#[cfg(test)]
mod tests {
    // Tests may unwrap: a failure here is a test failure, not a crash.
    #![allow(clippy::unwrap_used)]

    use frext_core::editor::FolderState;

    use super::*;

    const CELL: CellPx = CellPx {
        width: 10,
        height: 20,
    };

    fn id(n: u32) -> ImageId {
        ImageId::new(n).unwrap()
    }

    fn image(n: u32) -> IconCell {
        IconCell::Image { id: id(n) }
    }

    fn count(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|w| *w == needle)
            .count()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        count(haystack, needle) > 0
    }

    fn never(_svg: &[u8], _width: u32, _height: u32) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn first_icon_queues_one_upload() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::None);
        assert_eq!(icons.icon("main.rs", IconKind::File), image(1));
        let out = icons.take_uploads();
        assert_eq!(count(&out, b"\x1b_G"), 1);
        assert!(contains(&out, b"i=1,"));
        assert!(contains(&out, b"s=20,"));
        assert!(contains(&out, b"v=20,"));
        assert!(contains(&out, b"c=2,r=1"));
    }

    #[test]
    fn same_stem_reuses_the_id_and_queues_nothing() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::None);
        icons.icon("main.rs", IconKind::File);
        icons.take_uploads();
        assert_eq!(icons.icon("main.rs", IconKind::File), image(1));
        assert_eq!(icons.icon("lib.rs", IconKind::File), image(1));
        assert!(icons.take_uploads().is_empty());
    }

    #[test]
    fn a_different_stem_gets_the_next_id() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::None);
        icons.icon("main.rs", IconKind::File);
        icons.take_uploads();
        assert_eq!(icons.icon("Cargo.toml", IconKind::File), image(2));
        let out = icons.take_uploads();
        assert!(contains(&out, b"i=2,"));
        assert!(icons.take_uploads().is_empty());
    }

    #[test]
    fn uploads_are_wrapped_under_tmux() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::Tmux);
        icons.icon("main.rs", IconKind::File);
        assert!(icons.take_uploads().starts_with(b"\x1bPtmux;"));
    }

    #[test]
    fn failed_raster_falls_back_to_nerd_and_never_queues() {
        let mut icons = KittyIcons::with_rasterizer(CELL, Multiplexer::None, never);
        let nerd = nerd::nerd_icon("rust", IconKind::File);
        assert_eq!(icons.icon("main.rs", IconKind::File), nerd);
        assert!(icons.take_uploads().is_empty());
        assert_eq!(icons.icon("main.rs", IconKind::File), nerd);
        assert!(icons.take_uploads().is_empty());
    }

    #[test]
    fn release_deletes_in_id_order_then_forgets() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::None);
        icons.icon("main.rs", IconKind::File);
        icons.icon("Cargo.toml", IconKind::File);
        let mut expected = kitty::delete(id(1), Multiplexer::None);
        expected.extend(kitty::delete(id(2), Multiplexer::None));
        assert_eq!(icons.release(), expected);
        assert!(icons.take_uploads().is_empty());
        assert!(icons.release().is_empty());
        assert_eq!(icons.icon("main.rs", IconKind::File), image(3));
        assert!(contains(&icons.take_uploads(), b"i=3,"));
    }

    #[test]
    fn exhausted_ids_fall_back_to_nerd() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::None);
        icons.next_id = ImageId::new(0xFF_FFFF);
        assert_eq!(icons.icon("main.rs", IconKind::File), image(0xFF_FFFF));
        icons.take_uploads();
        assert_eq!(
            icons.icon("Cargo.toml", IconKind::File),
            nerd::nerd_icon(nerd::stem_for("Cargo.toml", IconKind::File), IconKind::File)
        );
        assert!(icons.take_uploads().is_empty());
    }

    #[test]
    fn folders_upload_under_their_own_stem() {
        let mut icons = KittyIcons::new(CELL, Multiplexer::None);
        icons.icon("main.rs", IconKind::File);
        icons.take_uploads();
        let open = IconKind::Dir(FolderState::Expanded);
        assert_eq!(icons.icon("src", open), image(2));
        assert_eq!(count(&icons.take_uploads(), b"\x1b_G"), 1);
        assert_eq!(icons.icon("src", open), image(2));
        assert!(icons.take_uploads().is_empty());
    }
}
