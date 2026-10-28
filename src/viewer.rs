use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use deltaflens_core::{Direction, ViewTransform, list_images, neighbor_image, prefetch_images};
use gpui::{
    App, Bounds, Context, CursorStyle, DispatchPhase, Entity, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, RenderImage, ScrollDelta, ScrollEvent, SharedString,
    Task, Window, canvas, div, point, prelude::*, px, rgb, size,
};

use crate::images::{ImageKey, Images, ImagesEvent};

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
    image: Option<(ImageKey, Arc<RenderImage>)>,
    error: Option<SharedString>,
    transform: ViewTransform,
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
    pub fn new(
        path: PathBuf,
        images: Entity<Images>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        cx.on_release_in(window, move |this, window, cx| {
            this.images.update(cx, |images, cx| images.forget(id, cx));
            this.release_image(window, cx);
        })
        .detach();
        cx.subscribe_in(&images, window, |this, _, event, window, cx| {
            this.on_images_event(event, window, cx);
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
            transform: ViewTransform::default(),
            viewport: None,
            drag_position: None,
            folder: None,
            folder_task: None,
        };
        this.load(path, Direction::Next, window, cx);
        this
    }

    /// Shows `path`, reached by moving in `direction`. Unless it is decoded
    /// already, the current image stays visible until it is.
    pub fn load(
        &mut self,
        path: PathBuf,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.key = ImageKey::new(path);
        self.direction = direction;
        self.list_folder(cx);
        let key = self.key.clone();
        let (image, error) = self
            .images
            .update(cx, |images, _| (images.get(&key), images.error(&key)));
        if let Some(image) = image {
            self.show(image, window, cx);
        } else if let Some(error) = error {
            self.show_error(error, window, cx);
        }
        self.request(cx);
    }

    /// Asks for the current image and the images around it in the folder.
    fn request(&self, cx: &mut Context<Self>) {
        let prefetch = self.folder_images().map_or_default(|images| {
            prefetch_images(images, &self.key.path, self.direction, PREFETCH_COUNT)
                .into_iter()
                .map(|path| ImageKey::new(path.to_path_buf()))
                .collect()
        });
        let id = cx.entity_id();
        let current = self.key.clone();
        self.images.update(cx, |images, cx| {
            images.request(id, Some(current), prefetch, cx);
        });
    }

    fn on_images_event(
        &mut self,
        event: &ImagesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ImagesEvent::Loaded(key) if *key == self.key => {
                let image = self.images.update(cx, |images, _| images.get(key));
                if let Some(image) = image {
                    self.show(image, window, cx);
                }
            }
            ImagesEvent::Failed(key) if *key == self.key => {
                let error = self.images.read(cx).error(key);
                if let Some(error) = error {
                    self.show_error(error, window, cx);
                }
            }
            ImagesEvent::Loaded(_) | ImagesEvent::Failed(_) => {}
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

    fn show(&mut self, image: Arc<RenderImage>, window: &mut Window, cx: &mut Context<Self>) {
        self.release_image(window, cx);
        self.image = Some((self.key.clone(), image));
        self.error = None;
        self.transform.reset();
        cx.notify();
    }

    fn show_error(&mut self, error: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.release_image(window, cx);
        self.error = Some(error);
        self.transform.reset();
        cx.notify();
    }

    /// Stops showing the image. Its texture is freed unless the image is still
    /// kept in memory to be shown again.
    fn release_image(&mut self, window: &mut Window, cx: &mut App) {
        if let Some((key, image)) = self.image.take()
            && !self.images.read(cx).contains(&key)
        {
            cx.drop_image(image, Some(window));
        }
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
