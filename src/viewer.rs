use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use deltaflens_core::{Direction, ViewTransform, list_images, neighbor_image, prefetch_images};
use gpui::{
    Bounds, Context, CursorStyle, DispatchPhase, Entity, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, RenderImage, ScrollDelta, ScrollEvent, SharedString,
    Task, Window, canvas, div, point, prelude::*, px, rgb, size,
};

use crate::images::{ImageKey, Images, ImagesEvent, Request};

/// How much one wheel notch zooms.
const ZOOM_STEP: f32 = 1.2;
/// gpui on Linux reports one wheel notch as 3 lines.
const LINES_PER_ZOOM_STEP: f32 = 3.0;
/// How many pixels of precise (touchpad) scrolling count as one wheel notch.
const PIXELS_PER_ZOOM_STEP: f32 = 50.0;
/// How many images on each side of the current one to decode ahead of time.
const PREFETCH_COUNT: usize = 2;

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
    image: Option<(ImageKey, Arc<RenderImage>)>,
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
    pub fn new(path: PathBuf, images: Entity<Images>, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        cx.on_release(move |this, cx| {
            this.images.update(cx, |images, cx| images.forget(id, cx));
        })
        .detach();
        cx.subscribe(&images, |this, _, event, cx| {
            this.on_images_event(event, cx);
        })
        .detach();

        let mut this = Self {
            key: ImageKey {
                path: PathBuf::new(),
                modified: None,
            },
            direction: Direction::Next,
            images,
            image: None,
            error: None,
            visible: false,
            transform: ViewTransform::default(),
            placed: None,
            viewport: None,
            drag_position: None,
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
                shown: self.image.as_ref().map(|(key, _)| key.clone()),
                current: Some(self.key.clone()),
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
        let (ImagesEvent::Loaded(key) | ImagesEvent::Failed(key)) = event;
        if self.visible && *key == self.key {
            self.show_decoded(cx);
            self.request(cx);
        }
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
        let task = cx.spawn({
            let dir = dir.clone();
            async move |this, cx| {
                let result = cx
                    .background_spawn({
                        let dir = dir.clone();
                        async move { list_images(&dir) }
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

    fn show(&mut self, image: Arc<RenderImage>, cx: &mut Context<Self>) {
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
        let (_, image) = self.image.as_ref()?;
        let (bounds, scale_factor) = self.viewport?;
        Some((image_size(image, scale_factor), to_core_size(bounds.size)))
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

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let Some((_, image)) = &self.image else {
            return;
        };
        let rect = self.transform.image_rect(
            image_size(image, window.scale_factor()),
            to_core_size(bounds.size),
        );
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
            tracing::error!("failed to paint {}: {error:#}", self.key.path.display());
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
                            view.update(cx, |this, _| {
                                this.viewport = Some((bounds, window.scale_factor()));
                            });
                        }
                    },
                    move |bounds, (), window, cx| {
                        view.read(cx).paint(bounds, window);
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
                        .text_color(rgb(0xd0_d0_d0))
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

/// The size of the image in logical pixels, so that scale 1.0 shows one image
/// pixel per device pixel.
#[allow(clippy::cast_precision_loss)]
fn image_size(image: &RenderImage, scale_factor: f32) -> deltaflens_core::Size {
    let size = image.size(0);
    deltaflens_core::Size {
        width: size.width.0 as f32 / scale_factor,
        height: size.height.0 as f32 / scale_factor,
    }
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
