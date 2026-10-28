//! Placement of an image in a viewport: fitting, zooming and panning.
//!
//! All values are in logical pixels.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

/// The largest scale the user can zoom in to.
const MAX_SCALE: f32 = 32.0;

/// How an image is placed in a viewport: fitted to it, or zoomed and panned
/// by the user.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewTransform {
    /// `None` while the image is fitted to the viewport, so that it follows
    /// changes of the viewport size.
    scale: Option<f32>,
    /// Offset of the image center from the viewport center.
    pan: Point,
}

impl ViewTransform {
    /// Returns the scale at which the whole image fits in the viewport. Small
    /// images are not enlarged.
    #[must_use]
    pub fn fit_scale(image: Size, viewport: Size) -> f32 {
        if image.width <= 0.0 || image.height <= 0.0 {
            return 1.0;
        }
        (viewport.width / image.width)
            .min(viewport.height / image.height)
            .clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn scale(&self, image: Size, viewport: Size) -> f32 {
        self.scale
            .unwrap_or_else(|| Self::fit_scale(image, viewport))
    }

    /// Returns where the image is drawn, relative to the top-left corner of
    /// the viewport.
    #[must_use]
    pub fn image_rect(&self, image: Size, viewport: Size) -> Rect {
        let size = scaled(image, self.scale(image, viewport));
        let pan = clamp_pan(self.pan, size, viewport);
        Rect {
            origin: Point {
                x: (viewport.width - size.width) / 2.0 + pan.x,
                y: (viewport.height - size.height) / 2.0 + pan.y,
            },
            size,
        }
    }

    /// Multiplies the scale by `factor`, keeping the image point under
    /// `anchor` (relative to the top-left corner of the viewport) in place.
    ///
    /// The image cannot be made smaller than when fitted; reaching that size
    /// fits it to the viewport again.
    pub fn zoom_at(&mut self, factor: f32, anchor: Point, image: Size, viewport: Size) {
        let fit = Self::fit_scale(image, viewport);
        let old = self.scale(image, viewport);
        let new = (old * factor).clamp(fit, MAX_SCALE.max(fit));
        if new <= fit {
            *self = Self::default();
            return;
        }

        let pan = clamp_pan(self.pan, scaled(image, old), viewport);
        let anchor = Point {
            x: anchor.x - viewport.width / 2.0,
            y: anchor.y - viewport.height / 2.0,
        };
        let ratio = new / old;
        let pan = Point {
            x: anchor.x - (anchor.x - pan.x) * ratio,
            y: anchor.y - (anchor.y - pan.y) * ratio,
        };
        self.scale = Some(new);
        self.pan = clamp_pan(pan, scaled(image, new), viewport);
    }

    /// Moves the image by `delta`, as far as it stays covering the viewport
    /// along each axis it overflows.
    pub fn pan_by(&mut self, delta: Point, image: Size, viewport: Size) {
        let size = scaled(image, self.scale(image, viewport));
        let pan = clamp_pan(self.pan, size, viewport);
        let pan = Point {
            x: pan.x + delta.x,
            y: pan.y + delta.y,
        };
        self.pan = clamp_pan(pan, size, viewport);
    }

    /// Fits the image to the viewport again.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn scaled(size: Size, scale: f32) -> Size {
    Size {
        width: size.width * scale,
        height: size.height * scale,
    }
}

/// Limits `pan` so that the image edges do not move inside the viewport. Along
/// an axis where the image is smaller than the viewport, it stays centered.
fn clamp_pan(pan: Point, image: Size, viewport: Size) -> Point {
    let limit_x = ((image.width - viewport.width) / 2.0).max(0.0);
    let limit_y = ((image.height - viewport.height) / 2.0).max(0.0);
    Point {
        x: pan.x.clamp(-limit_x, limit_x),
        y: pan.y.clamp(-limit_y, limit_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: f32, height: f32) -> Size {
        Size { width, height }
    }

    fn point(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            origin: point(x, y),
            size: size(width, height),
        }
    }

    const IMAGE_4K: Size = Size {
        width: 3840.0,
        height: 2160.0,
    };
    const VIEWPORT: Size = Size {
        width: 960.0,
        height: 720.0,
    };

    #[test]
    fn fit_shrinks_large_image_to_viewport() {
        assert_eq!(ViewTransform::fit_scale(IMAGE_4K, VIEWPORT), 0.25);
        assert_eq!(
            ViewTransform::default().image_rect(IMAGE_4K, VIEWPORT),
            rect(0.0, 90.0, 960.0, 540.0)
        );
    }

    #[test]
    fn fit_follows_narrower_axis() {
        let tall = size(1000.0, 4000.0);
        assert_eq!(ViewTransform::fit_scale(tall, VIEWPORT), 0.18);
    }

    #[test]
    fn fit_does_not_enlarge_small_image() {
        let small = size(100.0, 50.0);
        assert_eq!(ViewTransform::fit_scale(small, VIEWPORT), 1.0);
        assert_eq!(
            ViewTransform::default().image_rect(small, VIEWPORT),
            rect(430.0, 335.0, 100.0, 50.0)
        );
    }

    #[test]
    fn fit_of_empty_image_does_not_divide_by_zero() {
        assert_eq!(ViewTransform::fit_scale(size(0.0, 0.0), VIEWPORT), 1.0);
    }

    #[test]
    fn fitted_image_follows_viewport_size() {
        let transform = ViewTransform::default();
        let viewport = size(1920.0, 1080.0);
        assert_eq!(
            transform.image_rect(IMAGE_4K, viewport),
            rect(0.0, 0.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn zoom_at_center_keeps_image_centered() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(2.0, point(480.0, 360.0), IMAGE_4K, VIEWPORT);
        assert_eq!(transform.scale(IMAGE_4K, VIEWPORT), 0.5);
        assert_eq!(
            transform.image_rect(IMAGE_4K, VIEWPORT),
            rect(-480.0, -180.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn zoom_keeps_point_under_anchor_in_place() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(2.0, point(480.0, 360.0), IMAGE_4K, VIEWPORT);
        // At scale 0.5, image pixel (960, 360) is at the viewport origin.
        let anchor = point(0.0, 0.0);
        transform.zoom_at(2.0, anchor, IMAGE_4K, VIEWPORT);
        let rect = transform.image_rect(IMAGE_4K, VIEWPORT);
        assert_eq!(rect.size, size(3840.0, 2160.0));
        // At scale 1.0, the same pixel must still be there.
        assert_eq!(rect.origin, point(-960.0, -360.0));
    }

    #[test]
    fn zoom_near_edge_does_not_reveal_outside_of_image() {
        let mut transform = ViewTransform::default();
        // The left edge of the fitted image is at x = 0, so zooming there
        // keeps it there.
        transform.zoom_at(2.0, point(0.0, 360.0), IMAGE_4K, VIEWPORT);
        assert_eq!(transform.image_rect(IMAGE_4K, VIEWPORT).origin.x, 0.0);
    }

    #[test]
    fn zoom_out_below_fit_returns_to_fit() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(2.0, point(100.0, 100.0), IMAGE_4K, VIEWPORT);
        transform.zoom_at(0.1, point(100.0, 100.0), IMAGE_4K, VIEWPORT);
        assert_eq!(transform, ViewTransform::default());
    }

    #[test]
    fn zoom_is_limited() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(1000.0, point(480.0, 360.0), IMAGE_4K, VIEWPORT);
        assert_eq!(transform.scale(IMAGE_4K, VIEWPORT), MAX_SCALE);
    }

    #[test]
    fn zoomed_image_keeps_scale_when_viewport_changes() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(2.0, point(480.0, 360.0), IMAGE_4K, VIEWPORT);
        let viewport = size(1920.0, 1080.0);
        assert_eq!(transform.scale(IMAGE_4K, viewport), 0.5);
        assert_eq!(
            transform.image_rect(IMAGE_4K, viewport),
            rect(0.0, 0.0, 1920.0, 1080.0)
        );
    }

    #[test]
    fn pan_moves_zoomed_image() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(2.0, point(480.0, 360.0), IMAGE_4K, VIEWPORT);
        transform.pan_by(point(100.0, -50.0), IMAGE_4K, VIEWPORT);
        assert_eq!(
            transform.image_rect(IMAGE_4K, VIEWPORT).origin,
            point(-380.0, -230.0)
        );
    }

    #[test]
    fn pan_stops_at_image_edges() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(2.0, point(480.0, 360.0), IMAGE_4K, VIEWPORT);
        transform.pan_by(point(10_000.0, -10_000.0), IMAGE_4K, VIEWPORT);
        assert_eq!(
            transform.image_rect(IMAGE_4K, VIEWPORT).origin,
            point(0.0, -360.0)
        );
        // Moving back takes effect right away, without first undoing the
        // excess.
        transform.pan_by(point(-10.0, 0.0), IMAGE_4K, VIEWPORT);
        assert_eq!(transform.image_rect(IMAGE_4K, VIEWPORT).origin.x, -10.0);
    }

    #[test]
    fn pan_does_nothing_on_fitted_image() {
        let mut transform = ViewTransform::default();
        transform.pan_by(point(100.0, 100.0), IMAGE_4K, VIEWPORT);
        assert_eq!(
            transform.image_rect(IMAGE_4K, VIEWPORT),
            rect(0.0, 90.0, 960.0, 540.0)
        );
    }

    #[test]
    fn reset_fits_again() {
        let mut transform = ViewTransform::default();
        transform.zoom_at(3.0, point(10.0, 20.0), IMAGE_4K, VIEWPORT);
        transform.pan_by(point(100.0, 100.0), IMAGE_4K, VIEWPORT);
        transform.reset();
        assert_eq!(transform, ViewTransform::default());
    }
}
