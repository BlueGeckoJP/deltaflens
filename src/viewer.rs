use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use deltaflens_core::{
    DecodedImage, Direction, ViewTransform, decode, list_images, neighbor_image,
};
use eyre::{Result, eyre};
use gpui::{
    Bounds, Context, CursorStyle, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, RenderImage, ScrollDelta, ScrollEvent, SharedString, Task, Window,
    canvas, div, point, prelude::*, px, rgb, size,
};
use image::{Frame, RgbaImage};

/// How much one wheel notch zooms.
const ZOOM_STEP: f32 = 1.2;
/// gpui on Linux reports one wheel notch as 3 lines.
const LINES_PER_ZOOM_STEP: f32 = 3.0;
/// How many pixels of precise (touchpad) scrolling count as one wheel notch.
const PIXELS_PER_ZOOM_STEP: f32 = 50.0;

/// Shows one image, fitted to the viewer or zoomed and panned by the user.
pub struct Viewer {
    path: PathBuf,
    /// The image of `path`, or of the previous path while `path` is loading.
    image: Option<Arc<RenderImage>>,
    error: Option<SharedString>,
    transform: ViewTransform,
    /// The bounds and scale factor of the last frame, used by input handlers.
    viewport: Option<(Bounds<Pixels>, f32)>,
    /// The last mouse position while panning.
    drag_position: Option<gpui::Point<Pixels>>,
    /// Dropping it cancels the load, so that only the latest path is shown.
    load_task: Option<Task<()>>,
    /// The images in the folder of `path`, reused while moving within it.
    folder: Option<Folder>,
    /// The folder being listed, if any.
    folder_task: Option<(PathBuf, Task<()>)>,
}

struct Folder {
    dir: PathBuf,
    images: Vec<PathBuf>,
}

impl Viewer {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release_in(window, |this, window, cx| {
            if let Some(image) = this.image.take() {
                cx.drop_image(image, Some(window));
            }
        })
        .detach();

        let mut this = Self {
            path: PathBuf::new(),
            image: None,
            error: None,
            transform: ViewTransform::default(),
            viewport: None,
            drag_position: None,
            load_task: None,
            folder: None,
            folder_task: None,
        };
        this.load(path, window, cx);
        this
    }

    /// Decodes `path` in the background and shows it once done. The current
    /// image stays visible until then.
    pub fn load(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.path.clone_from(&path);
        self.list_folder(cx);
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { decode(&path).and_then(to_render_image) })
                .await;
            // Fails only if the viewer or the window is gone.
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_load(result, window, cx);
            });
        }));
    }

    /// Lists the folder of `path` in the background, unless it is already
    /// listed or being listed.
    fn list_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.path.parent() else {
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
                let _ = this.update(cx, |this, _| {
                    this.folder_task = None;
                    match result {
                        Ok(images) => this.folder = Some(Folder { dir, images }),
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
        let folder = self.folder.as_ref()?;
        if Some(folder.dir.as_path()) != self.path.parent() {
            return None;
        }
        neighbor_image(&folder.images, &self.path, direction).map(Path::to_path_buf)
    }

    fn finish_load(
        &mut self,
        result: Result<Arc<RenderImage>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(image) = self.image.take() {
            cx.drop_image(image, Some(window));
        }
        match result {
            Ok(image) => {
                self.image = Some(image);
                self.error = None;
            }
            Err(error) => {
                tracing::warn!("{error:#}");
                self.error = Some(format!("{error:#}").into());
            }
        }
        self.transform.reset();
        cx.notify();
    }

    pub fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.transform.reset();
        cx.notify();
    }

    /// The image and viewport sizes in logical pixels, if both are known.
    fn sizes(&self) -> Option<(deltaflens_core::Size, deltaflens_core::Size)> {
        let image = self.image.as_ref()?;
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
        let Some(image) = &self.image else {
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
            tracing::error!("failed to paint {}: {error:#}", self.path.display());
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

/// Wraps the pixels without copying them. Both use BGRA, despite the name
/// `RgbaImage`.
fn to_render_image(image: DecodedImage) -> Result<Arc<RenderImage>> {
    let (width, height) = (image.width(), image.height());
    let buffer = RgbaImage::from_raw(width, height, image.into_pixels())
        .ok_or_else(|| eyre!("pixel buffer does not match {width}x{height}"))?;
    Ok(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
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
