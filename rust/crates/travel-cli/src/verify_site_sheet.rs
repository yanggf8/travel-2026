//! Layer 4 of `travel verify-site` (docs/plans/2026-10-30-verify-site.md,
//! phase 3): `--contact-sheet <dir>` writes one thumbnail-grid PNG per plan,
//! every cell labelled with its map key, so an agent reviews a plan's maps in
//! one image. Advisory only — it never changes a PASS/WARN/FAIL verdict.
//!
//! Drawn with snapshot-maps' own raster, bitmap font, ink and PNG encoder; the
//! thumbnails are the live bytes layer 2 already fetched.

use crate::snapshot_maps::{self as maps, Raster};

/// Thumbnails per row.
pub(crate) const COLS: usize = 3;
/// Each live map is shown at 1/SHRINK of its size.
const SHRINK: u32 = 2;
/// Gap around and between cells.
const GAP: i32 = 10;
/// Label strip under each thumbnail: one line of the scale-1 font plus padding.
const LABEL_PAD: i32 = 4;
/// Plan title at the top, in the map title's scale.
const TITLE_SCALE: i32 = maps::TITLE_SCALE;

/// One cell: a map key, the state it was found in, and its live PNG when it is
/// a real map.
pub(crate) struct SheetCell {
    pub(crate) key: String,
    /// Empty for a real uploaded map; otherwise what the cell holds instead
    /// (SKIPPED, PLACEHOLDER, MISSING, …).
    pub(crate) note: String,
    pub(crate) png: Option<Vec<u8>>,
}

/// Pixel geometry of one sheet.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) thumb_w: i32,
    pub(crate) thumb_h: i32,
    pub(crate) label_h: i32,
    pub(crate) header_h: i32,
    /// Top-left of each cell's thumbnail, row-major.
    pub(crate) cells: Vec<(i32, i32)>,
}

/// Grid geometry for `n` cells. Pure.
pub(crate) fn layout(n: usize) -> Layout {
    let thumb_w = (maps::WIDTH / SHRINK) as i32;
    let thumb_h = (maps::HEIGHT / SHRINK) as i32;
    let label_h = maps::GLYPH_HEIGHT + 2 * LABEL_PAD;
    let header_h = GAP + maps::GLYPH_HEIGHT * TITLE_SCALE + GAP;
    let cols = n.clamp(1, COLS) as i32;
    let rows = n.div_ceil(COLS) as i32;
    let cells = (0..n as i32)
        .map(|i| {
            let (c, r) = (i % COLS as i32, i / COLS as i32);
            (GAP + c * (thumb_w + GAP), header_h + r * (thumb_h + label_h + GAP))
        })
        .collect();
    Layout {
        width: (GAP + cols * (thumb_w + GAP)) as u32,
        height: (header_h + rows * (thumb_h + label_h + GAP)) as u32,
        thumb_w,
        thumb_h,
        label_h,
        header_h,
        cells,
    }
}

/// The label under a cell, in the font's character set (upper case).
pub(crate) fn cell_label(cell: &SheetCell) -> String {
    if cell.note.is_empty() {
        cell.key.to_ascii_uppercase()
    } else {
        format!("{} {}", cell.key, cell.note).to_ascii_uppercase()
    }
}

/// Render one plan's sheet to PNG bytes.
pub(crate) fn render(plan_id: &str, cells: &[SheetCell]) -> Result<Vec<u8>, String> {
    let l = layout(cells.len());
    let mut r = Raster::new(l.width as usize, l.height as usize);
    r.fill_rect(0, 0, l.width as i32, l.height as i32, [255, 255, 255]);
    r.text(GAP, GAP, &plan_id.to_ascii_uppercase(), maps::TITLE_INK, TITLE_SCALE);
    for (cell, &(x, y)) in cells.iter().zip(&l.cells) {
        // Frame, then the thumbnail (or the bare grid color where there is none).
        r.fill_rect(x - 1, y - 1, l.thumb_w + 2, l.thumb_h + 2, maps::GRID_LINE);
        r.fill_rect(x, y, l.thumb_w, l.thumb_h, maps::GRID_BG);
        if let Some(rgba) = cell.png.as_deref().and_then(maps::decode_rgba_png) {
            blit_shrunk(&mut r, &rgba, x, y);
        }
        r.text(x, y + l.thumb_h + LABEL_PAD, &cell_label(cell), maps::TITLE_INK, 1);
    }
    maps::encode_rgba_png(&r.pixels, l.width, l.height)
}

/// Box-average a WIDTH×HEIGHT RGBA map down by SHRINK into the raster at (x, y).
fn blit_shrunk(r: &mut Raster, rgba: &[u8], x: i32, y: i32) {
    let (w, s) = (maps::WIDTH as usize, SHRINK as usize);
    for ty in 0..maps::HEIGHT as usize / s {
        for tx in 0..w / s {
            let mut sum = [0u32; 3];
            for dy in 0..s {
                for dx in 0..s {
                    let i = ((ty * s + dy) * w + tx * s + dx) * 4;
                    for (c, v) in sum.iter_mut().zip(&rgba[i..i + 3]) {
                        *c += u32::from(*v);
                    }
                }
            }
            let n = (s * s) as u32;
            r.put(x + tx as i32, y + ty as i32, sum.map(|c| (c / n) as u8));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_maps::test_render::{self, Bg};

    fn overlaps(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> bool {
        a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
    }

    #[test]
    fn grid_layout_wraps_at_cols_and_keeps_cells_apart() {
        let l = layout(7);
        assert_eq!((l.thumb_w, l.thumb_h), ((maps::WIDTH / 2) as i32, (maps::HEIGHT / 2) as i32));
        assert_eq!(l.cells.len(), 7);
        // Three per row, rows below the header.
        assert_eq!(l.cells[0].1, l.cells[2].1);
        assert!(l.cells[3].1 > l.cells[2].1 && l.cells[3].0 == l.cells[0].0);
        assert_eq!(l.cells[6], (l.cells[0].0, l.cells[0].1 + 2 * (l.cells[3].1 - l.cells[0].1)));
        assert!(l.cells.iter().all(|&(_, y)| y >= l.header_h));
        // Each cell (thumb + label strip) fits on the sheet and overlaps no other.
        let rects: Vec<_> = l
            .cells
            .iter()
            .map(|&(x, y)| (x, y, x + l.thumb_w, y + l.thumb_h + l.label_h))
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(a.0 >= 0 && a.1 >= 0 && a.2 <= l.width as i32 && a.3 <= l.height as i32, "{a:?} in {l:?}");
            for b in &rects[i + 1..] {
                assert!(!overlaps(*a, *b), "{a:?} vs {b:?}");
            }
        }
        // Fewer cells than a row: the sheet is only as wide as it needs.
        assert!(layout(1).width < layout(3).width);
        assert_eq!(layout(3).width, layout(9).width);
        assert_eq!(layout(0).cells.len(), 0);
    }

    #[test]
    fn sheet_draws_thumbnails_and_labels() {
        let textured = test_render::textured_basemap();
        let cells = [
            SheetCell {
                key: "plan.png".into(),
                note: String::new(),
                png: Some(test_render::day_map(Bg::Basemap(&textured))),
            },
            SheetCell { key: "day-1.png".into(), note: "skipped".into(), png: None },
        ];
        assert_eq!(cell_label(&cells[1]), "DAY-1.PNG SKIPPED");
        let png = render("kyoto-nov-2026", &cells).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        let l = layout(2);
        assert_eq!((info.width, info.height), (l.width, l.height));
        let px = |x: i32, y: i32| {
            let i = ((y as u32 * info.width + x as u32) * 4) as usize;
            [buf[i], buf[i + 1], buf[i + 2]]
        };
        // Label ink sits in each label strip.
        for &(x, y) in &l.cells {
            let ink = (y + l.thumb_h..y + l.thumb_h + l.label_h)
                .flat_map(|yy| (x..x + l.thumb_w).map(move |xx| (xx, yy)))
                .filter(|&(xx, yy)| px(xx, yy) == maps::TITLE_INK)
                .count();
            assert!(ink > 20, "label ink {ink} at {x},{y}");
        }
        // The real map's thumbnail is not the empty grid fill; the skipped one is.
        let (x0, y0) = l.cells[0];
        let (x1, y1) = l.cells[1];
        let non_bg = |x: i32, y: i32| {
            (y..y + l.thumb_h)
                .flat_map(|yy| (x..x + l.thumb_w).map(move |xx| (xx, yy)))
                .filter(|&(xx, yy)| px(xx, yy) != maps::GRID_BG)
                .count()
        };
        assert!(non_bg(x0, y0) > 1000);
        assert_eq!(non_bg(x1, y1), 0);
    }
}
