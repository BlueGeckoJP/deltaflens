use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use deltaflens_core::{
    Direction, Rect, ViewTransform, downscaled_size, list_images, neighbor_image, prefetch_images,
    visible_tiles,
};
use gpui::{
    Bounds, Context, CursorStyle, DispatchPhase, Entity, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, RenderImage, ScrollDelta, ScrollEvent, SharedString,
    Task, Window, canvas, div, point, prelude::*, px, size,
};

use crate::{
    images::{ImageKey, Images, ImagesEvent, Picture, Request},
    settings::SettingsStore,
    theme::theme,
};

/// How much one wheel notch zooms.
const ZOOM_STEP: f32 = 1.2;
/// gpui on Linux reports one wheel notch as 3 lines.
const LINES_PER_ZOOM_STEP: f32 = 3.0;
/// How many pixels of precise (touchpad) scrolling count as one wheel notch.
const PIXELS_PER_ZOOM_STEP: f32 = 50.0;
/// How many images on each side of the current one to decode ahead of time.
const PREFETCH_COUNT: usize = 2;
/// How long the scale must stay the same before a downscaled image is made
/// for it, so that zooming and resizing do not make one at every step.
const DOWNSCALE_DELAY: Duration = Duration::from_millis(200);

/// Shows one image, fitted to the viewer or zoomed and panned by the user.
pub struct Viewer {
    /// The image to show.
    key: ImageKey,
    /// Which way the user last moved in the folder, to decode the images that
    /// way first.
    direction: Direction,
    images: Entity<Images>,
    /// The image of `key`, or of the previous key while `key` is loading.
    /// `None` while the tab is hidden, so that only [`Images`] holds it.
    image: Option<(ImageKey, Arc<Picture>)>,
    /// A smaller version of `image`, drawn instead of it while it is shown at
    /// most that large.
    downscaled: Option<(ImageKey, Arc<Picture>)>,
    /// The tiles of the image of the key drawn in the last frame, told to
    /// [`Images`] when they change, which frees the textures of tiles no viewer
    /// draws.
    drawn_tiles: Option<(ImageKey, Vec<usize>)>,
    /// The smaller version asked for, for the current scale.
    wanted_downscaled: Option<ImageKey>,
    /// Asks for the smaller version for a new scale once it stays the same.
    /// Dropping it cancels that.
    downscale_task: Option<(Option<ImageKey>, Task<()>)>,
    error: Option<SharedString>,
    /// Whether the tab is on screen. A hidden viewer keeps only its path.
    visible: bool,
    transform: ViewTransform,
    /// The image `transform` was set for, kept while the tab is hidden so
    /// that the zoom stays when it is shown again.
    placed: Option<ImageKey>,
    /// The bounds and scale factor of the last frame, used by input handlers.
    viewport: Option<(Bounds<Pixels>, f32)>,
    /// The last mouse position while panning.
    drag_position: Option<gpui::Point<Pixels>>,
    /// Whether moving between images includes hidden files.
    show_hidden: bool,
    /// The images in the folder of the current image, reused while moving
    /// within it.
    folder: Option<Folder>,
    /// The folder being listed, if any.
    folder_task: Option<(PathBuf, Task<()>)>,
}

struct Folder {
    dir: PathBuf,
    images: Vec<PathBuf>,
}

impl Viewer {
    pub fn new(
        path: PathBuf,
        images: Entity<Images>,
        settings: &Entity<SettingsStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        // List the folder again when hidden files are shown or hidden.
        cx.observe(settings, |this, settings, cx| {
            let show_hidden = settings.read(cx).settings().show_hidden_files;
            if show_hidden != this.show_hidden {
                this.show_hidden = show_hidden;
                this.folder = None;
                this.folder_task = None;
                this.list_folder(cx);
            }
        })
        .detach();
        let show_hidden = settings.read(cx).settings().show_hidden_files;
        cx.on_release(move |this, cx| {
            this.images.update(cx, |images, cx| images.forget(id, cx));
        })
        .detach();
        cx.subscribe(&images, |this, _, event, cx| {
            this.on_images_event(event, cx);
        })
        .detach();

        let mut this = Self {
            key: ImageKey::new(PathBuf::new()),
            direction: Direction::Next,
            images,
            image: None,
            downscaled: None,
            drawn_tiles: None,
            wanted_downscaled: None,
            downscale_task: None,
            error: None,
            visible: false,
            transform: ViewTransform::default(),
            placed: None,
            viewport: None,
            drag_position: None,
            show_hidden,
            folder: None,
            folder_task: None,
        };
        this.load(path, Direction::Next, cx);
        this
    }

    /// Shows `path`, reached by moving in `direction`. Unless it is decoded
    /// already, the current image stays visible until it is.
    pub fn load(&mut self, path: PathBuf, direction: Direction, cx: &mut Context<Self>) {
        self.key = ImageKey::new(path);
        self.direction = direction;
        self.wanted_downscaled = None;
        self.downscale_task = None;
        self.list_folder(cx);
        if self.visible {
            self.show_decoded(cx);
            self.request(cx);
        }
    }

    /// Shows or hides the tab. A hidden viewer lets go of its image, and
    /// decodes it again when shown unless it is still in memory.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.show_decoded(cx);
        } else {
            self.image = None;
            self.downscaled = None;
            self.wanted_downscaled = None;
            self.downscale_task = None;
        }
        self.request(cx);
        cx.notify();
    }

    /// Shows the current image or why it cannot be shown, if it is known
    /// already.
    fn show_decoded(&mut self, cx: &mut Context<Self>) {
        let key = self.key.clone();
        let (image, error) = self
            .images
            .update(cx, |images, _| (images.get(&key), images.error(&key)));
        if let Some(image) = image {
            self.show(image, cx);
        } else if let Some(error) = error {
            self.show_error(error, cx);
        }
    }

    /// Asks for the current image and the images around it in the folder, or
    /// for nothing while the tab is hidden.
    fn request(&self, cx: &mut Context<Self>) {
        let request = if self.visible {
            Request {
                shown: [&self.image, &self.downscaled]
                    .into_iter()
                    .flatten()
                    .map(|(key, _)| key.clone())
                    .collect(),
                current: [Some(self.key.clone()), self.wanted_downscaled.clone()]
                    .into_iter()
                    .flatten()
                    .collect(),
                prefetch: self.folder_images().map_or_default(|images| {
                    prefetch_images(images, &self.key.path, self.direction, PREFETCH_COUNT)
                        .into_iter()
                        .map(|path| ImageKey::new(path.to_path_buf()))
                        .collect()
                }),
            }
        } else {
            Request::default()
        };
        let id = cx.entity_id();
        self.images
            .update(cx, |images, cx| images.request(id, request, cx));
    }

    fn on_images_event(&mut self, event: &ImagesEvent, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        match event {
            ImagesEvent::Loaded(key) | ImagesEvent::Failed(key) if *key == self.key => {
                self.show_decoded(cx);
                self.request(cx);
            }
            ImagesEvent::Loaded(key) if Some(key) == self.wanted_downscaled.as_ref() => {
                let image = self.images.update(cx, |images, _| images.get(key));
                if let Some(image) = image {
                    self.downscaled = Some((key.clone(), image));
                    self.request(cx);
                    cx.notify();
                }
            }
            // Without the smaller version, the image itself is drawn.
            ImagesEvent::Loaded(_) | ImagesEvent::Failed(_) => {}
        }
    }

    /// Notes the bounds of the viewer, and asks for a smaller version of the
    /// image for the scale it is shown at, once that stays the same for
    /// [`DOWNSCALE_DELAY`]. One already made is used at once.
    fn on_layout(&mut self, bounds: Bounds<Pixels>, scale_factor: f32, cx: &mut Context<Self>) {
        self.viewport = Some((bounds, scale_factor));
        let wanted = self.downscaled_key();
        if wanted == self.wanted_downscaled {
            // Back to the scale before a change that did not last.
            self.downscale_task = None;
            return;
        }
        let waiting = self
            .downscale_task
            .as_ref()
            .is_some_and(|(key, _)| *key == wanted);
        if waiting {
            return;
        }
        let made = wanted
            .as_ref()
            .is_some_and(|key| self.images.read(cx).contains(key));
        if wanted.is_none() || made {
            self.want_downscaled(wanted, cx);
            return;
        }
        let task = cx.spawn({
            let wanted = wanted.clone();
            async move |this, cx| {
                cx.background_executor().timer(DOWNSCALE_DELAY).await;
                let _ = this.update(cx, |this, cx| this.want_downscaled(wanted, cx));
            }
        });
        self.downscale_task = Some((wanted, task));
    }

    /// Asks for the smaller version of `wanted`, or for none. One made
    /// already, such as by another viewer while waiting, is used at once, as
    /// no event tells of it.
    fn want_downscaled(&mut self, wanted: Option<ImageKey>, cx: &mut Context<Self>) {
        self.downscale_task = None;
        let made = wanted
            .as_ref()
            .and_then(|key| self.images.update(cx, |images, _| images.get(key)));
        if wanted.is_none() || made.is_some() {
            self.downscaled = wanted.clone().zip(made);
            cx.notify();
        }
        self.wanted_downscaled = wanted;
        self.request(cx);
    }

    /// The key of the smaller version of the current image for the scale it is
    /// shown at, or `None` if the image itself is to be drawn.
    fn downscaled_key(&self) -> Option<ImageKey> {
        let (key, picture) = self.image.as_ref().filter(|(key, _)| *key == self.key)?;
        let (image_size, viewport) = self.sizes()?;
        // The scale of logical sizes is also that of device pixels shown to
        // image pixels.
        let scale = self.transform.scale(image_size, viewport);
        let (width, height) = picture.size;
        let size = downscaled_size(width, height, scale)?;
        // A tiled image is downscaled from its overview, which is no use when
        // shown at least as large.
        let overview = picture.image.size(0);
        let smaller = i64::from(size.0) < i64::from(overview.width.0);
        smaller.then(|| key.downscaled(size))
    }

    /// Lists the folder of the current image in the background, unless it is
    /// already listed or being listed.
    fn list_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.key.path.parent() else {
            return;
        };
        let listed = self.folder.as_ref().is_some_and(|folder| folder.dir == dir);
        let listing = self
            .folder_task
            .as_ref()
            .is_some_and(|(listing_dir, _)| listing_dir == dir);
        if listed || listing {
            return;
        }
        let dir = dir.to_path_buf();
        let show_hidden = self.show_hidden;
        let task = cx.spawn({
            let dir = dir.clone();
            async move |this, cx| {
                let result = cx
                    .background_spawn({
                        let dir = dir.clone();
                        async move { list_images(&dir, show_hidden) }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.folder_task = None;
                    match result {
                        Ok(images) => {
                            this.folder = Some(Folder { dir, images });
                            this.request(cx);
                        }
                        Err(error) => tracing::warn!("{error:#}"),
                    }
                });
            }
        });
        self.folder_task = Some((dir, task));
    }

    /// The image next to the current one in its folder, once the folder is
    /// listed.
    pub fn neighbor(&self, direction: Direction) -> Option<PathBuf> {
        neighbor_image(self.folder_images()?, &self.key.path, direction).map(Path::to_path_buf)
    }

    /// The images in the folder of the current image, once it is listed.
    fn folder_images(&self) -> Option<&[PathBuf]> {
        self.folder
            .as_ref()
            .filter(|folder| Some(folder.dir.as_path()) == self.key.path.parent())
            .map(|folder| folder.images.as_slice())
    }

    fn show(&mut self, image: Arc<Picture>, cx: &mut Context<Self>) {
        if self
            .downscaled
            .as_ref()
            .is_some_and(|(key, _)| key.original() != self.key)
        {
            self.downscaled = None;
        }
        if self.placed.as_ref() != Some(&self.key) {
            self.transform.reset();
            self.placed = Some(self.key.clone());
        }
        self.image = Some((self.key.clone(), image));
        self.error = None;
        cx.notify();
    }

    fn show_error(&mut self, error: SharedString, cx: &mut Context<Self>) {
        self.image = None;
        self.downscaled = None;
        self.error = Some(error);
        self.transform.reset();
        self.placed = None;
        cx.notify();
    }

    pub fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.transform.reset();
        cx.notify();
    }

    /// The image and viewport sizes in logical pixels, if both are known.
    fn sizes(&self) -> Option<(deltaflens_core::Size, deltaflens_core::Size)> {
        let (_, picture) = self.image.as_ref()?;
        let (bounds, scale_factor) = self.viewport?;
        Some((
            image_size(picture.size, scale_factor),
            to_core_size(bounds.size),
        ))
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let steps = match event.delta {
            ScrollDelta::Lines(lines) => lines.y / LINES_PER_ZOOM_STEP,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / PIXELS_PER_ZOOM_STEP,
        };
        let (Some((image, viewport)), Some((bounds, _))) = (self.sizes(), self.viewport) else {
            return;
        };
        if steps == 0.0 {
            return;
        }
        let anchor = to_core_point(event.position - bounds.origin);
        self.transform
            .zoom_at(ZOOM_STEP.powf(steps), anchor, image, viewport);
        cx.notify();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag_position = Some(event.position);
        cx.notify();
    }

    fn on_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(last) = self.drag_position.replace(event.position) else {
            return;
        };
        let Some((image, viewport)) = self.sizes() else {
            return;
        };
        self.transform
            .pan_by(to_core_point(event.position - last), image, viewport);
        cx.notify();
    }

    fn on_drag_end(&mut self, cx: &mut Context<Self>) {
        self.drag_position = None;
        cx.notify();
    }

    /// Draws the smallest version of the image that has at least as many
    /// pixels as the screen shows, so that it is not enlarged: the downscaled
    /// one, the image itself, or the tiles of a large image that are on
    /// screen.
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, picture)) = &self.image else {
            return;
        };
        let scale_factor = window.scale_factor();
        let rect = self.transform.image_rect(
            image_size(picture.size, scale_factor),
            to_core_size(bounds.size),
        );
        let large_enough =
            |image: &RenderImage| covers(render_size(image, scale_factor), rect.size);
        let image = match &self.downscaled {
            Some((downscaled_key, downscaled))
                if downscaled_key.original() == *key && large_enough(&downscaled.image) =>
            {
                Some(&downscaled.image)
            }
            _ => {
                (picture.tiles.is_empty() || large_enough(&picture.image)).then_some(&picture.image)
            }
        };
        let tiles = match image {
            Some(image) => {
                paint_image(window, bounds, rect, image, &self.key);
                Vec::new()
            }
            None => paint_tiles(window, bounds, rect, picture, &self.key),
        };
        let drawn = Some((key.clone(), tiles));
        if self.drawn_tiles != drawn {
            self.drawn_tiles.clone_from(&drawn);
            if let Some((key, tiles)) = drawn {
                let id = cx.entity_id();
                self.images.update(cx, |images, cx| {
                    images.set_drawn_tiles(id, key, tiles, cx);
                });
            }
        }
    }
}

impl Render for Viewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let dragging = self.drag_position.is_some();
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .when(dragging, |this| this.cursor(CursorStyle::ClosedHand))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .child(
                canvas(
                    {
                        let view = view.clone();
                        move |bounds, window, cx| {
                            view.update(cx, |this, cx| {
                                this.on_layout(bounds, window.scale_factor(), cx);
                            });
                        }
                    },
                    move |bounds, (), window, cx| {
                        view.update(cx, |this, cx| this.paint(bounds, window, cx));
                        if dragging {
                            listen_to_drag(&view, window);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_color(theme(cx).text)
                        .child(error),
                )
            })
    }
}

/// Follows the mouse anywhere in the window until the button is released, so
/// that panning goes on when the cursor leaves the viewer.
fn listen_to_drag(view: &gpui::Entity<Viewer>, window: &mut Window) {
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |this, cx| this.on_drag(event, cx));
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                view.update(cx, Viewer::on_drag_end);
            }
        }
    });
}

/// Draws `image` over `rect`, relative to the viewer `bounds`.
fn paint_image(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    rect: Rect,
    image: &Arc<RenderImage>,
    key: &ImageKey,
) {
    let image_bounds = Bounds::new(
        bounds.origin + point(px(rect.origin.x), px(rect.origin.y)),
        size(px(rect.size.width), px(rect.size.height)),
    );
    if let Err(error) = window.paint_image(
        bounds,
        image_bounds,
        gpui::Corners::default(),
        image.clone(),
        0,
        false,
    ) {
        tracing::error!("failed to paint {}: {error:#}", key.path.display());
    }
}

/// Draws the tiles of `picture` on screen, the image being over `rect`, and
/// returns their indices.
#[allow(clippy::cast_precision_loss)]
fn paint_tiles(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    rect: Rect,
    picture: &Picture,
    key: &ImageKey,
) -> Vec<usize> {
    // Logical pixels per image pixel.
    let scale = rect.size.width / picture.size.0 as f32;
    let viewport = to_core_size(bounds.size);
    let view = Rect {
        origin: deltaflens_core::Point {
            x: -rect.origin.x / scale,
            y: -rect.origin.y / scale,
        },
        size: deltaflens_core::Size {
            width: viewport.width / scale,
            height: viewport.height / scale,
        },
    };
    let rects: Vec<_> = picture.tiles.iter().map(|(rect, _)| *rect).collect();
    let visible = visible_tiles(&rects, view);
    for &index in &visible {
        let (tile, image) = &picture.tiles[index];
        let tile_rect = Rect {
            origin: deltaflens_core::Point {
                x: rect.origin.x + tile.x as f32 * scale,
                y: rect.origin.y + tile.y as f32 * scale,
            },
            size: deltaflens_core::Size {
                width: tile.width as f32 * scale,
                height: tile.height as f32 * scale,
            },
        };
        paint_image(window, bounds, tile_rect, image, key);
    }
    visible
}

/// The size of an image of `size` pixels in logical pixels, so that scale 1.0
/// shows one image pixel per device pixel.
#[allow(clippy::cast_precision_loss)]
fn image_size(size: (u32, u32), scale_factor: f32) -> deltaflens_core::Size {
    deltaflens_core::Size {
        width: size.0 as f32 / scale_factor,
        height: size.1 as f32 / scale_factor,
    }
}

/// The size of `image` in logical pixels, as [`image_size`].
#[allow(clippy::cast_precision_loss)]
fn render_size(image: &RenderImage, scale_factor: f32) -> deltaflens_core::Size {
    let size = image.size(0);
    deltaflens_core::Size {
        width: size.width.0 as f32 / scale_factor,
        height: size.height.0 as f32 / scale_factor,
    }
}

/// Whether an image of `size` has at least as many pixels as `shown`, both in
/// logical pixels, so that drawing it there does not enlarge it. Both sides
/// count, as rounding can leave one side of a thin image far too short.
fn covers(size: deltaflens_core::Size, shown: deltaflens_core::Size) -> bool {
    size.width + 0.5 >= shown.width && size.height + 0.5 >= shown.height
}

fn to_core_size(size: gpui::Size<Pixels>) -> deltaflens_core::Size {
    deltaflens_core::Size {
        width: size.width.into(),
        height: size.height.into(),
    }
}

fn to_core_point(point: gpui::Point<Pixels>) -> deltaflens_core::Point {
    deltaflens_core::Point {
        x: point.x.into(),
        y: point.y.into(),
    }
}

#[cfg(test)]
mod tests {
    use deltaflens_core::Size;

    use super::*;

    fn size(width: f32, height: f32) -> Size {
        Size { width, height }
    }

    #[test]
    fn image_as_large_as_shown_covers_it() {
        assert!(covers(size(960., 540.), size(960., 540.)));
        // Rounding the size of a downscaled image.
        assert!(covers(size(960., 540.), size(960.4, 540.3)));
        assert!(!covers(size(960., 540.), size(1000., 562.5)));
    }

    #[test]
    fn thin_image_needs_both_sides() {
        // The 1 x 8192 overview of a 1 x 10000 image, shown at its size.
        assert!(!covers(size(1., 8192.), size(1., 10_000.)));
    }
}
