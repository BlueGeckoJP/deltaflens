//! Images larger than a GPU texture can hold, drawn as tiles that each fit in
//! one.

use crate::{DecodedImage, Rect, decode::fit_within};

/// A part of an image, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Cuts a `width` x `height` image into a grid of tiles at most `max_tile`
/// pixels on each side, row by row from the top left.
///
/// The tiles of a row or column are about the same size, so that none of them
/// is a thin strip.
#[must_use]
pub fn tile_rects(width: u32, height: u32, max_tile: u32) -> Vec<TileRect> {
    let columns = split(width, max_tile);
    let rows = split(height, max_tile);
    rows.iter()
        .flat_map(|&(y, height)| {
            columns.iter().map(move |&(x, width)| TileRect {
                x,
                y,
                width,
                height,
            })
        })
        .collect()
}

/// Splits `length` into the fewest parts of at most `max` that differ by at
/// most one, as (start, length) pairs.
fn split(length: u32, max: u32) -> Vec<(u32, u32)> {
    let count = length.div_ceil(max.max(1)).max(1);
    let (base, extra) = (length / count, length % count);
    let mut start = 0;
    (0..count)
        .map(|index| {
            let part = base + u32::from(index < extra);
            let range = (start, part);
            start += part;
            range
        })
        .collect()
}

/// The indices of the tiles that overlap `view`, given in image pixels.
#[must_use]
pub fn visible_tiles(tiles: &[TileRect], view: Rect) -> Vec<usize> {
    let (left, top) = (view.origin.x, view.origin.y);
    let (right, bottom) = (left + view.size.width, top + view.size.height);
    tiles
        .iter()
        .enumerate()
        .filter(|(_, tile)| {
            // Images are at most a few hundred thousand pixels wide, which
            // f32 holds well enough to find what is on screen.
            #[expect(clippy::cast_precision_loss)]
            let (x0, y0, x1, y1) = (
                tile.x as f32,
                tile.y as f32,
                (tile.x + tile.width) as f32,
                (tile.y + tile.height) as f32,
            );
            x0 < right && left < x1 && y0 < bottom && top < y1
        })
        .map(|(index, _)| index)
        .collect()
}

/// Copies the pixels of each tile out of `image`.
#[must_use]
pub fn cut_tiles(image: &DecodedImage, tiles: &[TileRect]) -> Vec<DecodedImage> {
    let stride = image.width() as usize * 4;
    tiles
        .iter()
        .map(|tile| {
            let row = tile.width as usize * 4;
            let mut pixels = Vec::with_capacity(row * tile.height as usize);
            for y in tile.y..tile.y + tile.height {
                let start = y as usize * stride + tile.x as usize * 4;
                pixels.extend_from_slice(&image.pixels()[start..start + row]);
            }
            DecodedImage::new(tile.width, tile.height, pixels)
        })
        .collect()
}

/// The size of the whole image shrunk to fit in a `max_texture` square,
/// drawn while the tiles would be shown smaller than their size.
#[must_use]
pub fn overview_size(width: u32, height: u32, max_texture: u32) -> (u32, u32) {
    fit_within(width, height, max_texture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, Size};

    fn rect(x: u32, y: u32, width: u32, height: u32) -> TileRect {
        TileRect {
            x,
            y,
            width,
            height,
        }
    }

    fn view(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    #[test]
    fn image_that_fits_is_one_tile() {
        assert_eq!(tile_rects(100, 50, 2048), [rect(0, 0, 100, 50)]);
        assert_eq!(tile_rects(2048, 2048, 2048), [rect(0, 0, 2048, 2048)]);
    }

    #[test]
    fn tiles_cover_the_image_row_by_row() {
        assert_eq!(
            tile_rects(5000, 3000, 2048),
            [
                rect(0, 0, 1667, 1500),
                rect(1667, 0, 1667, 1500),
                rect(3334, 0, 1666, 1500),
                rect(0, 1500, 1667, 1500),
                rect(1667, 1500, 1667, 1500),
                rect(3334, 1500, 1666, 1500),
            ]
        );
    }

    #[test]
    fn tiles_do_not_leave_thin_strips() {
        // 2049 split by 2048 is not 2048 + 1.
        assert_eq!(
            tile_rects(2049, 10, 2048),
            [rect(0, 0, 1025, 10), rect(1025, 0, 1024, 10)]
        );
    }

    #[test]
    fn tiles_of_a_large_image_fit_and_add_up() {
        let tiles = tile_rects(20_000, 10_000, 2048);
        assert_eq!(tiles.len(), 10 * 5);
        assert!(tiles.iter().all(|t| t.width <= 2048 && t.height <= 2048));
        let area: u64 = tiles
            .iter()
            .map(|t| u64::from(t.width) * u64::from(t.height))
            .sum();
        assert_eq!(area, 20_000 * 10_000);
        let last = tiles.last().unwrap();
        assert_eq!(
            (last.x + last.width, last.y + last.height),
            (20_000, 10_000)
        );
    }

    #[test]
    fn visible_tiles_are_those_overlapping_the_view() {
        let tiles = tile_rects(4000, 4000, 2000);
        // Inside the top-left tile.
        assert_eq!(visible_tiles(&tiles, view(10., 10., 100., 100.)), [0]);
        // Across the middle, all four.
        assert_eq!(
            visible_tiles(&tiles, view(1900., 1900., 200., 200.)),
            [0, 1, 2, 3]
        );
        // The right column.
        assert_eq!(visible_tiles(&tiles, view(2500., 0., 1000., 4000.)), [1, 3]);
    }

    #[test]
    fn tile_touching_the_view_edge_is_not_visible() {
        let tiles = tile_rects(4000, 2000, 2000);
        assert_eq!(visible_tiles(&tiles, view(0., 0., 2000., 2000.)), [0]);
    }

    #[test]
    fn view_outside_the_image_shows_no_tile() {
        let tiles = tile_rects(4000, 2000, 2000);
        assert_eq!(
            visible_tiles(&tiles, view(-500., 0., 400., 100.)),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn cut_tiles_copies_each_part() {
        // A 4 x 2 image whose pixel at (x, y) is [x, y, 0, 255].
        let pixels = (0..2u8)
            .flat_map(|y| (0..4u8).flat_map(move |x| [x, y, 0, 255]))
            .collect();
        let image = DecodedImage::new(4, 2, pixels);
        let tiles = cut_tiles(&image, &tile_rects(4, 2, 2));
        assert_eq!(tiles.len(), 2);
        assert_eq!((tiles[1].width(), tiles[1].height()), (2, 2));
        assert_eq!(
            tiles[1].pixels(),
            [2, 0, 0, 255, 3, 0, 0, 255, 2, 1, 0, 255, 3, 1, 0, 255]
        );
    }

    #[test]
    fn overview_fits_the_texture() {
        assert_eq!(overview_size(20_000, 10_000, 8192), (8192, 4096));
        assert_eq!(overview_size(5000, 3000, 8192), (5000, 3000));
    }
}
