//! Plays an animation in a viewer. A thread decodes its frames a few ahead of
//! the one shown, so that the memory taken stays the same however long it
//! is, and a timer shows each frame once the one before has been shown for its
//! delay at the speed it plays at.
//!
//! Every seek starts a new generation. The thread drops the frames it decodes
//! for an older one, so that only the latest seek is decoded while the seek
//! bar is dragged, and the frame shown stays until the one sought is decoded.

use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

use deltaflens_core::{AnimationInfo, FrameSource, PlaybackState, open_animation};
use eyre::Result;
use futures::{StreamExt as _, channel::mpsc};
use gpui::{App, Context, EventEmitter, RenderImage, Task};

use crate::{
    images::MAX_TEXTURE_SIZE,
    render_image::{release_textures, to_render_image},
};

/// How long a frame that is due at once still waits for the next. gpui's
/// timer is ready at once for no wait, and its UI thread on Linux runs the
/// tasks spawned while it runs others before it handles input or draws. So
/// frames of no delay shown one after another would keep it from both, while
/// a timer this long goes through gpui's timer thread. The delays themselves
/// stay as they are.
const MIN_WAIT: Duration = Duration::from_millis(1);

/// How long to wait before showing the next frame, when the shown one is due
/// to be shown for `delay` and has been for `elapsed`.
fn wait_for(delay: Duration, elapsed: Duration) -> Duration {
    let wait = delay.saturating_sub(elapsed);
    if wait.is_zero() { MIN_WAIT } else { wait }
}

/// At most this many frames are decoded ahead of the one shown.
const MAX_FRAMES_AHEAD: usize = 4;
/// At least this many, so that one is decoded while the next waits to be
/// shown.
const MIN_FRAMES_AHEAD: usize = 2;
/// The frames decoded ahead take at most this much memory, unless that is
/// less than [`MIN_FRAMES_AHEAD`] frames.
const MAX_BYTES_AHEAD: u64 = 64 * 1024 * 1024;

/// How many frames of `width` × `height` pixels to decode ahead.
fn frames_ahead(width: u32, height: u32) -> usize {
    let frame_bytes = u64::from(width) * u64::from(height) * 4;
    usize::try_from(MAX_BYTES_AHEAD / frame_bytes.max(1))
        .unwrap_or(usize::MAX)
        .clamp(MIN_FRAMES_AHEAD, MAX_FRAMES_AHEAD)
}

/// Where the thumb of a seek bar is for `frame`, from 0 at the start of the
/// bar for the first frame to 1 at the end for the last.
#[expect(clippy::cast_precision_loss)]
pub fn seek_fraction(frame: usize, frame_count: usize) -> f32 {
    match frame_count.checked_sub(1) {
        Some(last) if last > 0 => frame.min(last) as f32 / last as f32,
        _ => 0.0,
    }
}

/// The frame whose thumb is nearest to `x` on a seek bar that starts at
/// `start` and is `length` long, as [`seek_fraction`] places them. Before and
/// after the bar are the first and the last frames.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn frame_at(x: f32, start: f32, length: f32, frame_count: usize) -> usize {
    let last = frame_count.saturating_sub(1);
    if length <= 0.0 {
        return 0;
    }
    let fraction = ((x - start) / length).clamp(0.0, 1.0);
    // Not negative, and at most `last` but for rounding.
    ((fraction * last as f32).round() as usize).min(last)
}

/// A decoded frame, ready to draw.
struct ReadyFrame {
    /// The place of the frame in the animation, from 0.
    index: usize,
    image: Arc<RenderImage>,
    /// How long it is shown.
    delay: Duration,
}

/// What the thread tells the session.
enum Message {
    /// The image is an animation. The thread waits for the first seek.
    Opened(AnimationInfo),
    /// The image is not an animation that can be played, as it has a single
    /// frame, is larger than a texture or cannot be opened. The thread has
    /// ended.
    Still,
    /// The next frame for the seek of `generation`, the first being the frame
    /// sought.
    Frame { generation: u64, frame: ReadyFrame },
    /// The next frame for the seek of `generation` failed to decode. The
    /// thread waits for another seek.
    Failed { generation: u64 },
}

/// Where the thread decodes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seek {
    /// The first frame to decode.
    frame: usize,
    /// How many times to go on from the first frame after the last, or
    /// `None` for ever.
    wraps: Option<u32>,
}

/// What the session asks of the thread, and how far ahead the thread is.
#[derive(Debug, Default)]
struct Control {
    /// The generation of the latest seek.
    generation: u64,
    /// The latest seek, until the thread takes it.
    seek: Option<Seek>,
    /// The frames sent for `generation` that the session has not shown yet.
    ahead: usize,
    stopped: bool,
}

/// The control of a thread, shared with it.
#[derive(Debug, Default)]
struct Shared {
    control: Mutex<Control>,
    /// Wakes the thread when `control` changes.
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Control> {
        // Nothing is left half changed under the lock, so a panic while it
        // was held changes nothing.
        self.control.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the frames of `generation` from `seek`. The frames sent for the
    /// generations before no longer count.
    fn seek(&self, generation: u64, seek: Seek) {
        let mut control = self.lock();
        control.generation = generation;
        control.seek = Some(seek);
        control.ahead = 0;
        self.changed.notify_all();
    }

    /// Makes room for another frame, after one sent for the current
    /// generation is shown or dropped.
    fn take_frame(&self) {
        let mut control = self.lock();
        control.ahead = control.ahead.saturating_sub(1);
        self.changed.notify_all();
    }

    /// Ends the thread once it is done with the step it is on.
    fn stop(&self) {
        self.lock().stopped = true;
        self.changed.notify_all();
    }
}

/// Opens the animation at `path` and decodes the frames that the session of
/// `shared` asks for, sending them with `sender`, until it stops or is gone.
///
/// The animation is opened here rather than by the session, as a
/// [`FrameSource`] cannot move between threads, and opening it reads the
/// file.
fn run(path: &Path, shared: &Shared, sender: &mpsc::UnboundedSender<Message>) {
    let source = open_animation(path).unwrap_or_else(|error| {
        tracing::warn!("{error:#}");
        None
    });
    // A frame larger than a texture would not be drawn, so the first frame is
    // shown in tiles instead, as a still image.
    let Some(source) = source.filter(|source| {
        let info = source.info();
        info.width <= MAX_TEXTURE_SIZE && info.height <= MAX_TEXTURE_SIZE
    }) else {
        let _ = sender.unbounded_send(Message::Still);
        return;
    };
    let info = source.info();
    if sender.unbounded_send(Message::Opened(info)).is_err() {
        return;
    }
    let decoder = Decoder {
        source,
        max_ahead: frames_ahead(info.width, info.height),
        generation: 0,
        position: 0,
        target: None,
        wraps: Some(0),
        idle: true,
    };
    decoder.run(path, shared, sender);
}

/// Decodes the frames of a thread.
struct Decoder {
    source: Box<dyn FrameSource>,
    /// How many frames to send ahead of the one shown.
    max_ahead: usize,
    /// The generation of the seek taken last.
    generation: u64,
    /// The frame `source` gives next.
    position: usize,
    /// The frame to move to before decoding.
    target: Option<usize>,
    /// How many more times to go on from the first frame after the last, or
    /// `None` for ever.
    wraps: Option<u32>,
    /// Whether there is nothing to decode until the next seek: before the
    /// first, after the last frame of the last play, and after a failure.
    idle: bool,
}

impl Decoder {
    fn run(mut self, path: &Path, shared: &Shared, sender: &mpsc::UnboundedSender<Message>) {
        while self.wait(shared) {
            // Each step is done without the lock, so that the session is not
            // kept waiting, and is short, so that a seek is taken soon.
            let step = match self.target {
                Some(target) if target != self.position => self.move_toward(target).map(|()| None),
                _ => {
                    self.target = None;
                    self.decode()
                }
            };
            let message = match step {
                Ok(None) => continue,
                Ok(Some(frame)) => {
                    let mut control = shared.lock();
                    // Sought past while it was decoded.
                    if control.generation != self.generation {
                        continue;
                    }
                    control.ahead += 1;
                    Message::Frame {
                        generation: self.generation,
                        frame,
                    }
                }
                Err(error) => {
                    tracing::warn!("{}: {error:#}", path.display());
                    self.idle = true;
                    self.target = None;
                    Message::Failed {
                        generation: self.generation,
                    }
                }
            };
            if sender.unbounded_send(message).is_err() {
                return;
            }
        }
    }

    /// Waits until there is something to do, taking the latest seek if there
    /// is one. Returns `false` once the session stops.
    fn wait(&mut self, shared: &Shared) -> bool {
        let mut control = shared.lock();
        loop {
            if control.stopped {
                return false;
            }
            if let Some(seek) = control.seek.take() {
                self.generation = control.generation;
                self.target = Some(seek.frame);
                self.wraps = seek.wraps;
                self.idle = false;
                return true;
            }
            if !self.idle && (self.target.is_some() || control.ahead < self.max_ahead) {
                return true;
            }
            control = shared
                .changed
                .wait(control)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Moves a frame toward `target`, or back to the first frame if it is
    /// behind. Moving forward decodes the frames passed in some formats, so it
    /// is done a frame at a time.
    fn move_toward(&mut self, target: usize) -> Result<()> {
        let next = if target < self.position {
            0
        } else {
            self.position + 1
        };
        self.source.seek(next)?;
        self.position = next;
        Ok(())
    }

    /// Decodes the next frame. After the last, it goes back to the first if
    /// another play follows, or else becomes idle.
    fn decode(&mut self) -> Result<Option<ReadyFrame>> {
        let Some(frame) = self.source.next_frame()? else {
            match self.wraps {
                Some(0) => self.idle = true,
                wraps => {
                    self.wraps = wraps.map(|wraps| wraps - 1);
                    self.target = Some(0);
                }
            }
            return Ok(None);
        };
        self.position = frame.index + 1;
        Ok(Some(ReadyFrame {
            index: frame.index,
            delay: frame.delay,
            // Made here, so that the UI thread does not copy the pixels.
            image: to_render_image(frame.image)?,
        }))
    }
}

/// Emitted when the image turns out not to be an animation to play, after
/// which the session does nothing.
pub struct NotAnimated;

/// Where an animation was left, to play it on from there.
#[derive(Debug, Clone)]
pub struct Resume {
    info: AnimationInfo,
    state: PlaybackState,
}

impl Resume {
    /// The speed it played at.
    pub fn speed(&self) -> f32 {
        self.state.speed()
    }
}

/// Plays an animation: decodes its frames in a thread of its own, shows each
/// when it is due, and frees their textures once they are not shown. Dropping
/// the session ends the thread and frees the textures.
pub struct Playback {
    shared: Arc<Shared>,
    /// Known once the thread opens the animation, or from where it was left.
    info: Option<AnimationInfo>,
    state: Option<PlaybackState>,
    /// Of the latest seek.
    generation: u64,
    /// Whether the frame of the latest seek has yet to come. It is shown as
    /// soon as it does.
    seeking: bool,
    /// The frame drawn.
    shown: Option<ReadyFrame>,
    /// The frames decoded after the one shown, for the latest seek.
    ahead: VecDeque<ReadyFrame>,
    /// Whether the next frame is due but not decoded yet. It is shown as soon
    /// as it is.
    late: bool,
    /// Whether the thread failed to decode a frame for the latest seek, and
    /// waits for another.
    failed: bool,
    /// Shows the next frame once the shown one has been shown for its delay,
    /// with when it began to wait: when the frame was shown, or played on
    /// after a pause.
    timer: Option<(Instant, Task<()>)>,
    /// Receives what the thread sends. Dropping it ends the thread.
    _messages: Task<()>,
}

impl EventEmitter<NotAnimated> for Playback {}

impl Playback {
    /// Starts playing the image at `path` if it is an animation, from where
    /// it was left if `resume` tells.
    pub fn new(path: &Path, resume: Option<Resume>, cx: &mut Context<Self>) -> Self {
        let shared = Arc::new(Shared::default());
        let (sender, mut receiver) = mpsc::unbounded();
        let spawned = thread::Builder::new().name("playback".into()).spawn({
            let shared = shared.clone();
            let sender = sender.clone();
            let path = path.to_path_buf();
            move || run(&path, &shared, &sender)
        });
        if let Err(error) = spawned {
            tracing::warn!("failed to play {}: {error}", path.display());
            let _ = sender.unbounded_send(Message::Still);
        }
        drop(sender);
        let messages = cx.spawn(async move |this, cx| {
            while let Some(message) = receiver.next().await {
                if this
                    .update(cx, |this, cx| this.receive(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        cx.on_release(Self::stop).detach();
        let (info, state) = resume.map(|resume| (resume.info, resume.state)).unzip();
        Self {
            shared,
            info,
            state,
            generation: 0,
            seeking: false,
            shown: None,
            ahead: VecDeque::new(),
            late: false,
            failed: false,
            timer: None,
            _messages: messages,
        }
    }

    /// The state of playing, once the image is known to be an animation.
    pub fn state(&self) -> Option<&PlaybackState> {
        self.state.as_ref()
    }

    /// The frame to draw, once one is decoded.
    pub fn image(&self) -> Option<&Arc<RenderImage>> {
        self.shown.as_ref().map(|frame| &frame.image)
    }

    /// Where the animation is, to play it on from there in another session.
    /// A drag of the seek bar ends there, as its release will not come.
    pub fn resume_point(&self) -> Option<Resume> {
        let mut state = self.state.clone()?;
        state.end_scrub();
        Some(Resume {
            info: self.info?,
            state,
        })
    }

    /// Plays or pauses. A finished animation plays again from the first
    /// frame.
    pub fn toggle_play_pause(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.state else {
            return;
        };
        if state.toggle_play_pause() {
            self.request(cx);
        }
        self.on_play_changed(cx);
    }

    /// Starts dragging the seek bar, which pauses until
    /// [`end_scrub`](Self::end_scrub).
    pub fn begin_scrub(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.state else {
            return;
        };
        state.begin_scrub();
        self.on_play_changed(cx);
    }

    /// Shows `frame` once it is decoded, while the seek bar is dragged. The
    /// frame shown stays until then.
    pub fn scrub_to(&mut self, frame: usize, cx: &mut Context<Self>) {
        let Some(state) = &mut self.state else {
            return;
        };
        let before = state.current_frame();
        state.scrub_to(frame);
        // The same frame again, as the mouse moves over it, is decoded once.
        if state.current_frame() != before {
            self.request(cx);
        }
        cx.notify();
    }

    /// Lets go of the seek bar, which plays again if it played before.
    pub fn end_scrub(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.state else {
            return;
        };
        state.end_scrub();
        self.on_play_changed(cx);
    }

    /// Plays at `speed` if it is from 0.05 to 16. Returns whether it does.
    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) -> bool {
        let set = self
            .state
            .as_mut()
            .is_some_and(|state| state.set_speed(speed));
        if set {
            self.on_speed_changed(cx);
        }
        set
    }

    /// Plays at the fastest preset slower than the current speed, if there
    /// is one.
    pub fn decrease_speed(&mut self, cx: &mut Context<Self>) {
        if self
            .state
            .as_mut()
            .is_some_and(PlaybackState::decrease_speed)
        {
            self.on_speed_changed(cx);
        }
    }

    /// Plays at the slowest preset faster than the current speed, if there
    /// is one.
    pub fn increase_speed(&mut self, cx: &mut Context<Self>) {
        if self
            .state
            .as_mut()
            .is_some_and(PlaybackState::increase_speed)
        {
            self.on_speed_changed(cx);
        }
    }

    /// Plays at the speed the file records.
    pub fn reset_speed(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.state {
            state.reset_speed();
            self.on_speed_changed(cx);
        }
    }

    /// Waits for the shown frame at the new speed, less the time it has
    /// waited already, so that going faster does not wait out a slow wait.
    fn on_speed_changed(&mut self, cx: &mut Context<Self>) {
        // Dropping the task stops it.
        if let Some((since, _)) = self.timer.take() {
            self.start_timer(since, cx);
        }
        cx.notify();
    }

    /// Starts or stops the timer as the state now plays or not.
    fn on_play_changed(&mut self, cx: &mut Context<Self>) {
        let playing = self.state.as_ref().is_some_and(PlaybackState::is_playing);
        // Playing again after a failure tries the frames from the one shown
        // again, once those decoded before the failure are shown.
        if playing && self.failed && self.ahead.is_empty() && !self.seeking {
            self.request(cx);
        }
        if playing && self.late {
            self.show_next(cx);
        } else {
            self.update_timer(cx);
        }
        cx.notify();
    }

    /// Asks the thread for the frames from the current frame of the state,
    /// dropping those decoded for an earlier seek.
    fn request(&mut self, cx: &mut App) {
        let Some(state) = &self.state else {
            return;
        };
        let seek = Seek {
            frame: state.current_frame(),
            wraps: state.wraps_left(),
        };
        self.generation += 1;
        self.seeking = true;
        self.late = false;
        self.failed = false;
        self.timer = None;
        release_textures(self.ahead.drain(..).map(|frame| frame.image).collect(), cx);
        self.shared.seek(self.generation, seek);
    }

    /// Runs the timer of the shown frame while it plays, unless it runs
    /// already, and stops it otherwise.
    fn update_timer(&mut self, cx: &mut Context<Self>) {
        let due = match (&self.state, &self.shown) {
            (Some(state), Some(shown)) => {
                state.is_playing()
                    && !self.seeking
                    && !self.late
                    && shown.index == state.current_frame()
            }
            _ => false,
        };
        if !due {
            self.timer = None;
            return;
        }
        if self.timer.is_none() {
            self.start_timer(Instant::now(), cx);
        }
    }

    /// Shows the next frame once the shown one has been shown for its delay
    /// at the current speed, counted from `since`. Even a frame due at once
    /// is moved on from in a later turn of the UI thread, after it has
    /// handled input.
    fn start_timer(&mut self, since: Instant, cx: &mut Context<Self>) {
        let (Some(state), Some(shown)) = (&self.state, &self.shown) else {
            return;
        };
        let wait = wait_for(state.scaled_delay(shown.delay), since.elapsed());
        let generation = self.generation;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.timer = None;
                    this.show_next(cx);
                }
            });
        });
        self.timer = Some((since, task));
    }

    /// Moves on from the shown frame, whose delay is over: to the next frame
    /// if it is decoded, or else as soon as it is. After the last frame of
    /// the last play, the animation finishes.
    fn show_next(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &mut self.state else {
            return;
        };
        if !state.is_playing() {
            return;
        }
        let Some(next) = state.next_frame_index() else {
            self.late = false;
            state.advance();
            cx.notify();
            return;
        };
        // Frames are decoded in the order they are shown, so none other is
        // expected here; dropping them keeps it from waiting for ever.
        while self.ahead.front().is_some_and(|frame| frame.index != next) {
            if let Some(frame) = self.ahead.pop_front() {
                self.shared.take_frame();
                release_textures(vec![frame.image], cx);
            }
        }
        let Some(frame) = self.ahead.pop_front() else {
            if self.failed {
                // No more frames will come.
                self.late = false;
                state.toggle_play_pause();
                cx.notify();
            } else {
                self.late = true;
            }
            return;
        };
        self.late = false;
        state.advance();
        self.shared.take_frame();
        self.show(frame, cx);
        self.update_timer(cx);
        cx.notify();
    }

    /// Draws `frame` instead of the frame drawn before, whose texture is
    /// freed.
    fn show(&mut self, frame: ReadyFrame, cx: &mut App) {
        if let Some(old) = self.shown.replace(frame) {
            release_textures(vec![old.image], cx);
        }
    }

    fn receive(&mut self, message: Message, cx: &mut Context<Self>) {
        match message {
            Message::Opened(info) => {
                // From the first frame, unless it goes on from where it was
                // left in the same animation.
                if self.info != Some(info) || self.state.is_none() {
                    self.state = Some(PlaybackState::new(info.frame_count, info.loop_count));
                }
                self.info = Some(info);
                self.request(cx);
                cx.notify();
            }
            Message::Still => {
                self.info = None;
                self.state = None;
                cx.emit(NotAnimated);
                cx.notify();
            }
            Message::Frame { generation, frame } => {
                if generation != self.generation {
                    release_textures(vec![frame.image], cx);
                } else if self.seeking {
                    self.seeking = false;
                    self.shared.take_frame();
                    self.show(frame, cx);
                    self.update_timer(cx);
                    cx.notify();
                } else {
                    self.ahead.push_back(frame);
                    if self.late {
                        self.show_next(cx);
                    }
                }
            }
            Message::Failed { generation } => {
                if generation != self.generation {
                    return;
                }
                self.failed = true;
                // The frame sought or the next one will not come, so it stops
                // on the frame shown.
                if self.seeking || self.late {
                    self.seeking = false;
                    self.late = false;
                    if let Some(state) = &mut self.state
                        && state.is_playing()
                    {
                        state.toggle_play_pause();
                    }
                    self.update_timer(cx);
                    cx.notify();
                }
            }
        }
    }

    /// Ends the thread and frees the textures of the frames.
    fn stop(&mut self, cx: &mut App) {
        self.shared.stop();
        self.timer = None;
        let images = self
            .shown
            .take()
            .into_iter()
            .chain(self.ahead.drain(..))
            .map(|frame| frame.image)
            .collect();
        release_textures(images, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Instant};

    use futures::channel::mpsc::{TryRecvError, UnboundedReceiver};

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates/deltaflens-core/tests/fixtures")
            .join(name)
    }

    /// A thread decoding the animation at `name`.
    struct Worker {
        shared: Arc<Shared>,
        receiver: UnboundedReceiver<Message>,
        thread: thread::JoinHandle<()>,
    }

    impl Worker {
        fn start(name: &str) -> Self {
            let shared = Arc::new(Shared::default());
            let (sender, receiver) = mpsc::unbounded();
            let thread = thread::spawn({
                let shared = shared.clone();
                let path = fixture(name);
                move || run(&path, &shared, &sender)
            });
            Self {
                shared,
                receiver,
                thread,
            }
        }

        /// Waits for the next message, for at most a few seconds.
        fn next(&mut self) -> Message {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match self.receiver.try_recv() {
                    Ok(message) => return message,
                    Err(TryRecvError::Closed) => panic!("the thread ended"),
                    Err(TryRecvError::Empty) => {
                        assert!(Instant::now() < deadline, "no message came");
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }

        /// The index of the next frame, which must be for `generation`.
        fn next_frame(&mut self, generation: u64) -> usize {
            match self.next() {
                Message::Frame {
                    generation: actual,
                    frame,
                } => {
                    assert_eq!(actual, generation);
                    frame.index
                }
                _ => panic!("not a frame"),
            }
        }

        /// Asserts that nothing comes for a while.
        fn assert_quiet(&mut self) {
            thread::sleep(Duration::from_millis(100));
            assert!(matches!(self.receiver.try_recv(), Err(TryRecvError::Empty)));
        }

        fn opened(&mut self) -> AnimationInfo {
            match self.next() {
                Message::Opened(info) => info,
                _ => panic!("not opened"),
            }
        }

        fn stop(self) {
            self.shared.stop();
            self.thread.join().unwrap();
        }
    }

    fn seek(frame: usize, wraps: Option<u32>) -> Seek {
        Seek { frame, wraps }
    }

    #[test]
    fn waits_for_what_is_left_of_the_delay() {
        let ms = Duration::from_millis;
        assert_eq!(wait_for(ms(400), Duration::ZERO), ms(400));
        assert_eq!(wait_for(ms(400), ms(150)), ms(250));
    }

    #[test]
    fn frame_due_at_once_still_waits_a_little() {
        let ms = Duration::from_millis;
        assert_eq!(wait_for(Duration::ZERO, Duration::ZERO), MIN_WAIT);
        // Sped up after waiting longer than the new delay.
        assert_eq!(wait_for(ms(100), ms(300)), MIN_WAIT);
        assert_eq!(wait_for(ms(100), ms(100)), MIN_WAIT);
    }

    #[test]
    fn frames_ahead_fit_in_64_mib() {
        assert_eq!(frames_ahead(320, 240), 4);
        assert_eq!(frames_ahead(1920, 1080), 4);
        // 2560 x 1440 takes 14 MiB.
        assert_eq!(frames_ahead(2560, 1440), 4);
        // 33 MB, so only one fits, but two are kept.
        assert_eq!(frames_ahead(3840, 2160), 2);
        assert_eq!(frames_ahead(8192, 8192), 2);
        assert_eq!(frames_ahead(0, 0), 4);
    }

    #[test]
    fn seek_bar_ends_are_the_first_and_last_frames() {
        assert_eq!(frame_at(10.0, 10.0, 100.0, 5), 0);
        assert_eq!(frame_at(110.0, 10.0, 100.0, 5), 4);
        assert!(seek_fraction(0, 5).abs() < f32::EPSILON);
        assert!((seek_fraction(4, 5) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn seek_bar_gives_the_nearest_frame() {
        // Frames at 0, 25, 50, 75 and 100.
        assert_eq!(frame_at(22.0, 0.0, 100.0, 5), 1);
        assert_eq!(frame_at(37.0, 0.0, 100.0, 5), 1);
        assert_eq!(frame_at(38.0, 0.0, 100.0, 5), 2);
        for frame in 0..60 {
            let x = 5.0 + seek_fraction(frame, 60) * 200.0;
            assert_eq!(frame_at(x, 5.0, 200.0, 60), frame);
        }
    }

    #[test]
    fn seek_bar_clamps_outside_positions() {
        assert_eq!(frame_at(-50.0, 0.0, 100.0, 5), 0);
        assert_eq!(frame_at(500.0, 0.0, 100.0, 5), 4);
        assert_eq!(frame_at(f32::NAN, 0.0, 100.0, 5), 0);
        assert!((seek_fraction(10, 5) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn seek_bar_without_room_or_frames_gives_the_first() {
        assert_eq!(frame_at(50.0, 0.0, 0.0, 5), 0);
        assert_eq!(frame_at(50.0, 0.0, -10.0, 5), 0);
        assert_eq!(frame_at(50.0, 0.0, 100.0, 1), 0);
        assert_eq!(frame_at(50.0, 0.0, 100.0, 0), 0);
        assert!(seek_fraction(0, 1).abs() < f32::EPSILON);
        assert!(seek_fraction(0, 0).abs() < f32::EPSILON);
    }

    #[test]
    fn still_images_end_the_thread() {
        for name in ["red-3x2.png", "red-3x2.gif", "missing.gif"] {
            let mut worker = Worker::start(name);
            assert!(matches!(worker.next(), Message::Still), "{name}");
            worker.thread.join().unwrap();
        }
    }

    #[test]
    fn decodes_nothing_before_the_first_seek() {
        let mut worker = Worker::start("anim-rgb.gif");
        let info = worker.opened();
        assert_eq!((info.width, info.height, info.frame_count), (4, 2, 3));
        worker.assert_quiet();
        worker.stop();
    }

    #[test]
    fn decodes_in_order_up_to_the_end_of_the_last_play() {
        for name in ["anim-rgb.gif", "anim-rgb.jxl"] {
            let mut worker = Worker::start(name);
            worker.opened();
            worker.shared.seek(1, seek(0, Some(1)));
            let frames: Vec<_> = (0..4).map(|_| worker.next_frame(1)).collect();
            assert_eq!(frames, [0, 1, 2, 0], "{name}");
            // Four frames ahead.
            worker.assert_quiet();
            for _ in 0..4 {
                worker.shared.take_frame();
            }
            let frames: Vec<_> = (0..2).map(|_| worker.next_frame(1)).collect();
            assert_eq!(frames, [1, 2], "{name}");
            // The last play ends there.
            worker.assert_quiet();
            worker.stop();
        }
    }

    #[test]
    fn plays_for_ever_without_a_wrap_count() {
        let mut worker = Worker::start("anim-rgb.webp");
        worker.opened();
        worker.shared.seek(1, seek(1, None));
        let mut frames = Vec::new();
        for _ in 0..10 {
            frames.push(worker.next_frame(1));
            worker.shared.take_frame();
        }
        assert_eq!(frames, [1, 2, 0, 1, 2, 0, 1, 2, 0, 1]);
        worker.stop();
    }

    #[test]
    fn first_frame_after_a_seek_is_the_one_sought() {
        let mut worker = Worker::start("anim-rgb.apng");
        worker.opened();
        worker.shared.seek(1, seek(0, None));
        assert_eq!(worker.next_frame(1), 0);
        // Backward from where the thread is, then forward.
        for (generation, frame) in [(2, 0), (3, 2), (4, 1)] {
            worker.shared.seek(generation, seek(frame, Some(0)));
            let first = loop {
                match worker.next() {
                    Message::Frame {
                        generation: actual,
                        frame,
                    } if actual == generation => break frame.index,
                    // Decoded for the seek before.
                    Message::Frame {
                        generation: actual, ..
                    } => assert!(actual < generation),
                    _ => panic!("not a frame"),
                }
            };
            assert_eq!(first, frame);
        }
        worker.stop();
    }

    #[test]
    fn stop_ends_a_waiting_thread() {
        let mut worker = Worker::start("anim-rgb.gif");
        worker.opened();
        worker.shared.seek(1, seek(0, None));
        for _ in 0..4 {
            worker.next_frame(1);
        }
        // Waiting for room.
        worker.assert_quiet();
        worker.stop();
    }

    #[test]
    fn thread_ends_when_the_session_is_gone() {
        let Worker {
            shared,
            receiver,
            thread,
        } = Worker::start("anim-rgb.gif");
        drop(receiver);
        shared.seek(1, seek(0, None));
        thread.join().unwrap();
    }
}
