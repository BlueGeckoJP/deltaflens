use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use deltaflens_core::{
    DEFAULT_PLAYBACK_SPEED, Direction, PlaybackState, Rect, ViewTransform, downscaled_size,
    format_speed, list_images, neighbor_image, parse_speed, prefetch_images, visible_tiles,
};
use gpui::{
    App, Bounds, ClickEvent, Context, CursorStyle, DispatchPhase, Entity, FontFeatures,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, RenderImage,
    Rgba, ScrollDelta, ScrollEvent, SharedString, Subscription, Task, Window, canvas, div, fill,
    point, prelude::*, px, size,
};

use crate::{
    file_actions,
    images::{ImageKey, Images, ImagesEvent, Picture, Request},
    menu::{ContextMenu, Menu},
    playback::{NotAnimated, Playback, Resume, frame_at, seek_fraction},
    settings::SettingsStore,
    text_input::{TextInput, TextInputEvent},
    theme::{Theme, theme},
    workspace_view::{NextImage, PreviousImage, ResetView, SplitDown, SplitRight, TogglePlayback},
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

/// The size of the play and pause button of an animation.
const PLAY_BUTTON_SIZE: Pixels = px(22.);
/// The size of the square that holds the play and pause icons.
const PLAY_ICON_SIZE: Pixels = px(12.);
/// A play triangle on a 12 × 12 grid.
const PLAY_TRIANGLE: [(f32, f32); 3] = [(3., 2.), (10., 6.), (3., 10.)];
/// The two bars of a pause icon on a 12 × 12 grid, as left, top, right and
/// bottom.
const PAUSE_BARS: [[f32; 4]; 2] = [[3., 2., 5., 10.], [7., 2., 9., 10.]];
/// The width of the seek bar, which gets narrower in a narrow viewer, down to
/// the smaller width.
const SEEK_BAR_WIDTH: Pixels = px(200.);
const MIN_SEEK_BAR_WIDTH: Pixels = px(48.);
const SEEK_BAR_HEIGHT: Pixels = px(16.);
const SEEK_TRACK_HEIGHT: Pixels = px(4.);
/// The diameter of the thumb of the seek bar, which stays within the bar.
const SEEK_THUMB_SIZE: Pixels = px(10.);
/// About the width of a digit of the frame numbers, to keep their room the
/// same as they change.
const DIGIT_WIDTH: f32 = 7.;
/// The width of the field of the playback speed, enough for `0.125`.
const SPEED_INPUT_WIDTH: Pixels = px(56.);
/// The size of the buttons that step the playback speed.
const SPEED_BUTTON_SIZE: Pixels = px(20.);

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
    /// The active left-button interaction, if any.
    drag: Option<Drag>,
    /// Whether moving between images includes hidden files.
    show_hidden: bool,
    /// The images in the folder of the current image, reused while moving
    /// within it.
    folder: Option<Folder>,
    /// The folder being listed, if any.
    folder_task: Option<(PathBuf, Task<()>)>,
    settings: Entity<SettingsStore>,
    context_menu: Entity<ContextMenu>,
    /// The error of the last action from the context menu.
    notice: Option<SharedString>,
    /// The animation of the current image, played while the tab is visible.
    animation: Option<Animation>,
    /// Where the animation of an image was left when the tab was hidden, to
    /// go on from there when it is shown again.
    resume: Option<(ImageKey, Resume)>,
    /// The image last found not to be an animation, so that it is not opened
    /// as one again each time it is shown.
    still: Option<ImageKey>,
    /// The bounds of the seek bar in the last frame, to tell the frame under
    /// the mouse.
    seek_bar: Option<Bounds<Pixels>>,
    /// Shows the playback speed and changes it when one is entered.
    speed_input: Entity<TextInput>,
    /// The speed `speed_input` was last set to show, to show it again once it
    /// changes.
    shown_speed: f32,
}

/// An active left-button interaction in the viewer.
enum Drag {
    /// Panning the image, with the last mouse position.
    Pan(gpui::Point<Pixels>),
    /// Dragging the seek bar of an animation.
    Scrub,
    /// A left press on the animation controls, ignored by the viewer so it
    /// does not pan while still bubbling up to take keyboard focus back from
    /// a text field.
    Controls,
}

/// The animation of an image, played by a session of its own.
struct Animation {
    key: ImageKey,
    playback: Entity<Playback>,
    _subscriptions: [Subscription; 2],
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
        context_menu: Entity<ContextMenu>,
        window: &mut Window,
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
        let speed_input =
            cx.new(|cx| TextInput::new(format_speed(DEFAULT_PLAYBACK_SPEED).into(), window, cx));
        cx.subscribe_in(&speed_input, window, |this, _, event, window, cx| {
            this.on_speed_input(event, window, cx);
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
            drag: None,
            show_hidden,
            folder: None,
            folder_task: None,
            settings: settings.clone(),
            context_menu,
            notice: None,
            animation: None,
            resume: None,
            still: None,
            seek_bar: None,
            speed_input,
            shown_speed: DEFAULT_PLAYBACK_SPEED,
        };
        this.load(path, Direction::Next, cx);
        this
    }

    /// Shows `path`, reached by moving in `direction`. Unless it is decoded
    /// already, the current image stays visible until it is.
    pub fn load(&mut self, path: PathBuf, direction: Direction, cx: &mut Context<Self>) {
        let key = ImageKey::new(path);
        if key != self.key {
            // Ends the session of the image before, and its thread.
            self.animation = None;
            self.resume = None;
            self.clear_scrub();
        }
        self.key = key;
        self.direction = direction;
        self.notice = None;
        self.wanted_downscaled = None;
        self.downscale_task = None;
        self.list_folder(cx);
        if self.visible {
            self.show_decoded(cx);
            self.request(cx);
        }
    }

    /// Shows or hides the tab. A hidden viewer lets go of its image, and
    /// decodes it again when shown unless it is still in memory. An animation
    /// stops, and goes on from the same frame when shown again.
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
            self.suspend_animation(cx);
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
        let wanted = self.downscaled_key(cx);
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
    /// shown at, or `None` if the image itself is to be drawn. The frames of
    /// an animation are always drawn as they are, scaled down by the GPU, as
    /// it would take as long to make smaller versions of them.
    fn downscaled_key(&self, cx: &App) -> Option<ImageKey> {
        if self.playback_state(cx).is_some() {
            return None;
        }
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
        // An image larger than a texture is shown in tiles, still.
        if image.tiles.is_empty() {
            self.play(cx);
        }
        self.image = Some((self.key.clone(), image));
        self.error = None;
        cx.notify();
    }

    /// Plays the current image if it is an animation, from where it was left
    /// if the tab was hidden. Until its first frame is decoded, the image
    /// itself is drawn.
    fn play(&mut self, cx: &mut Context<Self>) {
        let playing = self
            .animation
            .as_ref()
            .is_some_and(|animation| animation.key == self.key);
        if playing || self.still.as_ref() == Some(&self.key) {
            return;
        }
        let resume = self
            .resume
            .take()
            .filter(|(key, _)| *key == self.key)
            .map(|(_, resume)| resume);
        let key = self.key.clone();
        let playback = cx.new(|cx| Playback::new(&key.path, resume, cx));
        let subscriptions = [
            cx.observe(&playback, |this, playback, cx| {
                this.on_playback_changed(&playback, cx);
            }),
            cx.subscribe(&playback, |this, playback, _: &NotAnimated, cx| {
                this.on_not_animated(&playback, cx);
            }),
        ];
        self.animation = Some(Animation {
            key,
            playback,
            _subscriptions: subscriptions,
        });
    }

    fn on_playback_changed(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        // Smaller versions are not used for an animation.
        let animated = playback.read(cx).state().is_some();
        if animated
            && (self.downscaled.is_some()
                || self.wanted_downscaled.is_some()
                || self.downscale_task.is_some())
        {
            self.want_downscaled(None, cx);
        }
        // Only once it changes, as this comes at every frame and would drop
        // what is being typed.
        if (self.speed(cx) - self.shown_speed).abs() > f32::EPSILON {
            self.show_speed(cx);
        }
        cx.notify();
    }

    fn on_not_animated(&mut self, playback: &Entity<Playback>, cx: &mut Context<Self>) {
        let current = self
            .animation
            .as_ref()
            .is_some_and(|animation| animation.playback == *playback);
        if let Some(animation) = self.animation.take_if(|_| current) {
            self.still = Some(animation.key);
            self.clear_scrub();
            cx.notify();
        }
    }

    /// Stops playing, keeping where the animation is to go on from there.
    /// Dropping the session ends its thread and frees its textures.
    fn suspend_animation(&mut self, cx: &mut Context<Self>) {
        self.clear_scrub();
        if let Some(animation) = self.animation.take() {
            self.resume = animation
                .playback
                .read(cx)
                .resume_point()
                .map(|resume| (animation.key, resume));
        }
    }

    fn clear_scrub(&mut self) {
        if matches!(self.drag, Some(Drag::Scrub)) {
            self.drag = None;
        }
    }

    /// The state of playing the current image, once it is known to be an
    /// animation.
    fn playback_state<'a>(&self, cx: &'a App) -> Option<&'a PlaybackState> {
        self.animation.as_ref()?.playback.read(cx).state()
    }

    /// Plays or pauses the animation, if the image is one.
    pub fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if let Some(animation) = &self.animation {
            animation.playback.update(cx, Playback::toggle_play_pause);
        }
    }

    /// Plays the animation, if the image is one, at the fastest preset speed
    /// slower than its speed.
    pub fn decrease_speed(&mut self, cx: &mut Context<Self>) {
        if let Some(animation) = &self.animation {
            animation.playback.update(cx, Playback::decrease_speed);
        }
    }

    /// Plays the animation, if the image is one, at the slowest preset speed
    /// faster than its speed.
    pub fn increase_speed(&mut self, cx: &mut Context<Self>) {
        if let Some(animation) = &self.animation {
            animation.playback.update(cx, Playback::increase_speed);
        }
    }

    /// Plays the animation, if the image is one, at the speed the file
    /// records.
    pub fn reset_speed(&mut self, cx: &mut Context<Self>) {
        if let Some(animation) = &self.animation {
            animation.playback.update(cx, Playback::reset_speed);
        }
    }

    /// The playback speed of the current image: of its session, or where it
    /// was left while the tab is hidden, or else the speed a new one starts
    /// at.
    fn speed(&self, cx: &App) -> f32 {
        let resumed = || {
            self.resume
                .as_ref()
                .filter(|(key, _)| *key == self.key)
                .map(|(_, resume)| resume.speed())
        };
        self.playback_state(cx)
            .map(PlaybackState::speed)
            .or_else(resumed)
            .unwrap_or(DEFAULT_PLAYBACK_SPEED)
    }

    /// Shows the playback speed in its field, dropping what was typed there.
    fn show_speed(&mut self, cx: &mut Context<Self>) {
        self.shown_speed = self.speed(cx);
        let text = format_speed(self.shown_speed).into();
        self.speed_input
            .update(cx, |input, cx| input.reset_text(text, cx));
    }

    /// Plays at the speed entered, if it is one, and gives the keys back to
    /// the viewer. What was typed and not entered is dropped.
    fn on_speed_input(
        &mut self,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let TextInputEvent::Submitted(text) = event {
            if let (Some(speed), Some(animation)) = (parse_speed(text), &self.animation) {
                animation
                    .playback
                    .update(cx, |playback, cx| playback.set_speed(speed, cx));
            }
            // The pane takes the focus back.
            window.blur(cx);
        }
        self.show_speed(cx);
    }

    /// The frame under `x` on the seek bar.
    fn frame_under(&self, x: Pixels, cx: &App) -> Option<usize> {
        let bounds = self.seek_bar?;
        let frame_count = self.playback_state(cx)?.frame_count();
        // The thumb stays within the bar.
        let start = bounds.left() + SEEK_THUMB_SIZE * 0.5;
        let length = bounds.size.width - SEEK_THUMB_SIZE;
        Some(frame_at(x.into(), start.into(), length.into(), frame_count))
    }

    /// Starts dragging the seek bar, which moves to the frame under the
    /// mouse at once.
    fn on_seek_bar_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(frame) = self.frame_under(event.position.x, cx) else {
            return;
        };
        let Some(animation) = &self.animation else {
            return;
        };
        animation.playback.update(cx, |playback, cx| {
            playback.begin_scrub(cx);
            playback.scrub_to(frame, cx);
        });
        self.drag = Some(Drag::Scrub);
        cx.notify();
    }

    fn on_scrub(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !matches!(self.drag, Some(Drag::Scrub)) {
            return;
        }
        let Some(frame) = self.frame_under(event.position.x, cx) else {
            return;
        };
        if let Some(animation) = &self.animation {
            animation
                .playback
                .update(cx, |playback, cx| playback.scrub_to(frame, cx));
        }
    }

    fn on_scrub_end(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.drag, Some(Drag::Scrub)) {
            return;
        }
        self.drag = None;
        if let Some(animation) = &self.animation {
            animation.playback.update(cx, Playback::end_scrub);
        }
        cx.notify();
    }

    fn show_error(&mut self, error: SharedString, cx: &mut Context<Self>) {
        self.image = None;
        self.downscaled = None;
        self.animation = None;
        self.clear_scrub();
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
        match self.drag {
            Some(Drag::Controls) => {
                self.drag = None;
                return;
            }
            Some(Drag::Scrub) => return,
            Some(Drag::Pan(_)) | None => {}
        }
        self.drag = Some(Drag::Pan(event.position));
        cx.notify();
    }

    /// Opens the menu of the image at the mouse. Its actions apply to the
    /// active tab of the focused pane, which pressing the pane made this one.
    fn show_menu(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.key.path.clone();
        let this = cx.entity().downgrade();
        let open_command = self.settings.read(cx).settings().open_command.clone();
        let playing = self.playback_state(cx).map(PlaybackState::is_playing);
        let menu = Menu::new(event.position)
            .when_some(playing, |menu, playing| {
                let label = if playing { "Pause" } else { "Play" };
                menu.action(label, TogglePlayback, true, window).separator()
            })
            .action(
                "Previous image",
                PreviousImage,
                self.neighbor(Direction::Previous).is_some(),
                window,
            )
            .action(
                "Next image",
                NextImage,
                self.neighbor(Direction::Next).is_some(),
                window,
            )
            .action("Reset view", ResetView, true, window)
            .separator()
            .item("Copy image", {
                let this = this.clone();
                let path = path.clone();
                move |_, cx| {
                    let _ = this.update(cx, |_, cx| Self::copy_image(&path, cx));
                }
            })
            .item("Copy path", {
                let path = path.clone();
                move |_, cx| file_actions::copy_path(&path, cx)
            })
            .item(
                file_actions::open_with_label(&open_command),
                move |_, cx| {
                    let _ = this.update(cx, |this, cx| {
                        let result = file_actions::open_externally(&path, &open_command);
                        this.notice = file_actions::notice(result);
                        cx.notify();
                    });
                },
            )
            .separator()
            .action("Split right", SplitRight, true, window)
            .action("Split down", SplitDown, true, window);
        self.context_menu
            .update(cx, |context_menu, cx| context_menu.show(menu, cx));
    }

    /// Copies the image to the clipboard in the background.
    fn copy_image(path: &Path, cx: &mut Context<Self>) {
        let task = file_actions::copy_image(path, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            // Fails only if the viewer is gone.
            let _ = this.update(cx, |this, cx| {
                this.notice = file_actions::notice(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn on_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(Drag::Pan(last)) = self.drag.as_mut() else {
            return;
        };
        let delta = event.position - *last;
        *last = event.position;
        let Some((image, viewport)) = self.sizes() else {
            return;
        };
        self.transform.pan_by(to_core_point(delta), image, viewport);
        cx.notify();
    }

    fn on_drag_end(&mut self, cx: &mut Context<Self>) {
        if matches!(self.drag, Some(Drag::Pan(_))) {
            self.drag = None;
            cx.notify();
        }
    }

    /// Draws the frame of the animation being played, or else the smallest
    /// version of the image that has at least as many pixels as the screen
    /// shows, so that it is not enlarged: the downscaled one, the image
    /// itself, or the tiles of a large image that are on screen.
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, picture)) = &self.image else {
            return;
        };
        let scale_factor = window.scale_factor();
        let rect = self.transform.image_rect(
            image_size(picture.size, scale_factor),
            to_core_size(bounds.size),
        );
        let frame = self
            .animation
            .as_ref()
            .filter(|animation| animation.key == *key)
            .and_then(|animation| animation.playback.read(cx).image().cloned());
        let large_enough =
            |image: &RenderImage| covers(render_size(image, scale_factor), rect.size);
        let image = match (&frame, &self.downscaled) {
            (Some(frame), _) => Some(frame),
            (None, Some((downscaled_key, downscaled)))
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

impl Viewer {
    /// The bar over the bottom of an animation that plays and pauses it,
    /// seeks in it, tells the frame shown and changes its speed. A press on it
    /// does not reach the image, but still focuses the pane.
    fn render_controls(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.error.is_some() {
            return None;
        }
        let state = self.playback_state(cx)?;
        let (playing, frame, frame_count) = (
            state.is_playing(),
            state.current_frame(),
            state.frame_count(),
        );
        if frame_count < 2 {
            return None;
        }
        let theme = theme(cx);
        let view = cx.entity();
        let hover = theme.element_hover;
        let icon_color = theme.text_strong;
        let play_button = div()
            .id("play-pause")
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(PLAY_BUTTON_SIZE)
            .rounded_sm()
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_playback(cx)))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| {
                        paint_play_icon(playing, icon_color, bounds, window);
                    },
                )
                .flex_shrink_0()
                .size(PLAY_ICON_SIZE),
            );
        let fraction = seek_fraction(frame, frame_count);
        let seek_bar = div()
            .w(SEEK_BAR_WIDTH)
            .min_w(MIN_SEEK_BAR_WIDTH)
            .h(SEEK_BAR_HEIGHT)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_seek_bar_down))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        view.update(cx, |this, _| this.seek_bar = Some(bounds));
                    },
                    move |bounds, (), window, _| {
                        paint_seek_bar(bounds, fraction, &theme, window);
                    },
                )
                .size_full(),
            );
        let speed = self.render_speed(&theme, cx);
        let bar = div()
            .flex()
            .items_center()
            .gap_2()
            .max_w_full()
            .pl_1()
            .pr_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.menu_border)
            .bg(theme.menu)
            .shadow_md()
            .text_xs()
            .text_color(theme.text_muted)
            // Keep presses and the wheel from panning, zooming or opening
            // the menu of the image. Left presses still bubble up (with pan
            // suppressed on the viewer) so that the pane takes keyboard focus
            // back from a text field.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if !matches!(this.drag, Some(Drag::Scrub)) {
                        this.drag = Some(Drag::Controls);
                    }
                }),
            )
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(play_button)
            .child(seek_bar)
            .child(frame_counter(frame, frame_count))
            .child(speed);
        // Centered by a box across the viewer that has no listeners, so that
        // the image can be panned beside the bar.
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_3()
                .px_3()
                .flex()
                .justify_center()
                .child(bar),
        )
    }

    /// The part of the controls that changes the speed: buttons that step it
    /// down and up through the presets, around a field that shows it and
    /// takes one typed in. A press on the field focuses it rather than the
    /// pane.
    fn render_speed(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (slower, faster) = self.playback_state(cx).map_or((false, false), |state| {
            (state.can_decrease_speed(), state.can_increase_speed())
        });
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .pl_2()
            .border_l_1()
            .border_color(theme.menu_border)
            .child(speed_button(
                "speed-down",
                "\u{2212}",
                slower,
                theme,
                cx.listener(|this, _: &ClickEvent, _, cx| this.decrease_speed(cx)),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    // The field fills the width it is given.
                    .child(div().w(SPEED_INPUT_WIDTH).child(self.speed_input.clone()))
                    .child("x"),
            )
            .child(speed_button(
                "speed-up",
                "+",
                faster,
                theme,
                cx.listener(|this, _: &ClickEvent, _, cx| this.increase_speed(cx)),
            ))
    }
}

impl Render for Viewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let dragging = matches!(self.drag, Some(Drag::Pan(_)));
        let scrubbing = matches!(self.drag, Some(Drag::Scrub));
        let controls = self.render_controls(cx);
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .when(dragging, |this| this.cursor(CursorStyle::ClosedHand))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::show_menu))
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
                            follow_drag(&view, window, Viewer::on_drag, Viewer::on_drag_end);
                        }
                        if scrubbing {
                            follow_drag(&view, window, Viewer::on_scrub, Viewer::on_scrub_end);
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
            .children(controls)
            .when_some(self.notice.clone(), |this, notice| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .child(file_actions::render_notice(notice, cx)),
                )
            })
    }
}

/// Follows the mouse anywhere in the window until the left button is
/// released, so that panning or dragging the seek bar goes on when the cursor
/// leaves the viewer.
fn follow_drag(
    view: &Entity<Viewer>,
    window: &mut Window,
    on_move: fn(&mut Viewer, &MouseMoveEvent, &mut Context<Viewer>),
    on_end: fn(&mut Viewer, &mut Context<Viewer>),
) {
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |this, cx| on_move(this, event, cx));
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                view.update(cx, on_end);
            }
        }
    });
}

/// The number of the frame shown and of all frames, in room wide enough for
/// the largest numbers, so that the bar keeps its width as they change.
fn frame_counter(frame: usize, frame_count: usize) -> impl IntoElement {
    #[expect(clippy::cast_precision_loss)]
    let digits = frame_count.to_string().len() as f32;
    div()
        .flex_shrink_0()
        .min_w(px(DIGIT_WIDTH * (digits * 2. + 3.)))
        .text_right()
        .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
        .child(format!("{} / {frame_count}", frame + 1))
}

/// A button of the controls of an animation that steps its speed, drawn
/// dimmed and doing nothing unless `enabled`. A press on it does not pan the
/// image, as the controls take it.
fn speed_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let hover = theme.element_hover;
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(SPEED_BUTTON_SIZE)
        .rounded_sm()
        .text_sm()
        .when(enabled, |this| {
            this.text_color(theme.text_strong)
                .hover(move |style| style.bg(hover))
                .on_click(on_click)
        })
        .when(!enabled, |this| this.text_color(theme.text_disabled))
        .child(label)
}

/// Draws a pause icon if `playing`, or else a play icon, in the 12 × 12
/// `bounds`.
fn paint_play_icon(playing: bool, color: Rgba, bounds: Bounds<Pixels>, window: &mut Window) {
    let at = |(x, y): (f32, f32)| bounds.origin + point(px(x), px(y));
    if playing {
        for [left, top, right, bottom] in PAUSE_BARS {
            let bar = Bounds::from_corners(at((left, top)), at((right, bottom)));
            window.paint_quad(fill(bar, color));
        }
        return;
    }
    let [first, rest @ ..] = PLAY_TRIANGLE;
    let mut builder = PathBuilder::fill();
    builder.move_to(at(first));
    for corner in rest {
        builder.line_to(at(corner));
    }
    builder.close();
    match builder.build() {
        Ok(path) => window.paint_path(path, color),
        Err(error) => tracing::error!("failed to draw an icon: {error}"),
    }
}

/// Draws a seek bar over `bounds`, with its thumb `fraction` of the way
/// along. The thumb stays within the bounds.
fn paint_seek_bar(bounds: Bounds<Pixels>, fraction: f32, theme: &Theme, window: &mut Window) {
    let radius = SEEK_THUMB_SIZE * 0.5;
    let center_y = bounds.center().y;
    let start = bounds.left() + radius;
    let length = (bounds.size.width - SEEK_THUMB_SIZE).max(Pixels::ZERO);
    let track = Bounds::new(
        point(start, center_y - SEEK_TRACK_HEIGHT * 0.5),
        size(length, SEEK_TRACK_HEIGHT),
    );
    let rounding = SEEK_TRACK_HEIGHT * 0.5;
    window.paint_quad(fill(track, theme.menu_border).corner_radii(rounding));
    let thumb_x = start + length * fraction;
    let played = Bounds::new(track.origin, size(thumb_x - start, SEEK_TRACK_HEIGHT));
    window.paint_quad(fill(played, theme.accent).corner_radii(rounding));
    let thumb = Bounds::centered_at(
        point(thumb_x, center_y),
        size(SEEK_THUMB_SIZE, SEEK_THUMB_SIZE),
    );
    window.paint_quad(fill(thumb, theme.accent).corner_radii(radius));
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
