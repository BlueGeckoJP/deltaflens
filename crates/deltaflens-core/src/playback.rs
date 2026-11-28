//! The state of playing an animation: which frame is shown, whether it plays,
//! and how fast. It knows nothing of how frames are decoded or when they are
//! due.

use std::{ops::RangeInclusive, time::Duration};

use crate::LoopCount;

/// The speed an animation plays at first, as the file records it.
pub const DEFAULT_PLAYBACK_SPEED: f32 = 1.0;

/// The speeds that stepping the speed down and up goes through.
const PLAYBACK_SPEED_PRESETS: [f32; 6] = [0.25, 0.5, 1.0, 1.5, 2.0, 3.0];

/// The speeds an animation can play at.
const PLAYBACK_SPEED_RANGE: RangeInclusive<f32> = 0.05..=16.0;

/// Reads a speed typed by the user, such as `1.5` or `2x`, with spaces around
/// it. Returns `None` unless it is a number in the range speeds can be, from
/// 0.05 to 16.
#[must_use]
pub fn parse_speed(text: &str) -> Option<f32> {
    let text = text.trim();
    let number = text.strip_suffix(['x', 'X']).unwrap_or(text).trim_end();
    // Not a number and the infinities are out of range too.
    let speed = number.parse().ok()?;
    PLAYBACK_SPEED_RANGE.contains(&speed).then_some(speed)
}

/// Shows a speed as briefly as [`parse_speed`] reads it back the same, without
/// trailing zeros: `1`, `0.25`, `1.5`. Speeds are only set from the presets or
/// from such text, so no rounding is needed.
#[must_use]
pub fn format_speed(speed: f32) -> String {
    speed.to_string()
}

/// How long a frame of `delay` is shown at `speed`: a delay of 0 stays 0, and
/// one too long to hold becomes [`Duration::MAX`].
#[must_use]
pub fn scale_delay(delay: Duration, speed: f32) -> Duration {
    if delay.is_zero() {
        return Duration::ZERO;
    }
    Duration::try_from_secs_f64(delay.as_secs_f64() / f64::from(speed)).unwrap_or(Duration::MAX)
}

/// The fastest preset slower than `speed`. Speeds are compared exactly, as
/// they are never computed.
fn slower_preset(speed: f32) -> Option<f32> {
    PLAYBACK_SPEED_PRESETS
        .into_iter()
        .rev()
        .find(|&preset| preset < speed)
}

/// The slowest preset faster than `speed`.
fn faster_preset(speed: f32) -> Option<f32> {
    PLAYBACK_SPEED_PRESETS
        .into_iter()
        .find(|&preset| preset > speed)
}

/// Whether an animation plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    /// The last play of a finite loop ended, on the last frame.
    Finished,
}

/// What [`PlaybackState::advance`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// Nothing, as the animation does not play.
    Idle,
    /// Moved to the next frame of the same play.
    Next,
    /// Moved back to the first frame for another play.
    Wrapped,
    /// The last play ended, and the last frame stays.
    Finished,
}

/// Which frame of an animation is shown, whether it plays, how many times it
/// has been played, and how fast.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackState {
    /// At least 1.
    frame_count: usize,
    loop_count: LoopCount,
    current_frame: usize,
    /// How many plays ended before the current one.
    completed_loops: u32,
    status: PlaybackStatus,
    /// While the seek bar is dragged, whether to play when it is let go.
    resume_after_scrub: Option<bool>,
    /// The delays of the frames are divided by this. Within
    /// [`PLAYBACK_SPEED_RANGE`].
    speed: f32,
}

impl PlaybackState {
    /// Starts playing from the first frame. A `frame_count` of 0 is taken as
    /// 1, so that there is always a frame to show.
    #[must_use]
    pub fn new(frame_count: usize, loop_count: LoopCount) -> Self {
        Self {
            frame_count: frame_count.max(1),
            loop_count,
            current_frame: 0,
            completed_loops: 0,
            status: PlaybackStatus::Playing,
            resume_after_scrub: None,
            speed: DEFAULT_PLAYBACK_SPEED,
        }
    }

    #[must_use]
    pub fn status(&self) -> PlaybackStatus {
        self.status
    }

    /// The index of the frame shown, from 0.
    #[must_use]
    pub fn current_frame(&self) -> usize {
        self.current_frame
    }

    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.status == PlaybackStatus::Playing
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.status == PlaybackStatus::Finished
    }

    /// Whether the seek bar is being dragged.
    #[must_use]
    pub fn is_scrubbing(&self) -> bool {
        self.resume_after_scrub.is_some()
    }

    /// The frame that [`advance`](Self::advance) moves to, or `None` if it
    /// moves to none: while not playing, and on the last frame of the last
    /// play, after which the animation finishes. So the frame to wait for
    /// before moving on is known.
    #[must_use]
    pub fn next_frame_index(&self) -> Option<usize> {
        if self.status != PlaybackStatus::Playing {
            return None;
        }
        if self.current_frame + 1 < self.frame_count {
            Some(self.current_frame + 1)
        } else {
            self.another_play().then_some(0)
        }
    }

    /// How many more times the animation goes back to the first frame after
    /// the current play, or `None` if it does for ever. Frames can be decoded
    /// ahead up to the end of the last play.
    #[must_use]
    pub fn wraps_left(&self) -> Option<u32> {
        match self.loop_count {
            LoopCount::Infinite => None,
            LoopCount::Finite(plays) => Some(
                plays
                    .get()
                    .saturating_sub(self.completed_loops.saturating_add(1)),
            ),
        }
    }

    /// Whether another play follows the current one.
    fn another_play(&self) -> bool {
        self.wraps_left() != Some(0)
    }

    /// Plays if paused and pauses if playing. A finished animation plays
    /// again from the first frame, with all its loops; then this returns
    /// `true`, as the frames must be decoded from the first again.
    ///
    /// While scrubbing, this only changes whether it plays when the seek bar
    /// is let go.
    pub fn toggle_play_pause(&mut self) -> bool {
        if let Some(resume) = &mut self.resume_after_scrub {
            *resume = !*resume;
            return false;
        }
        match self.status {
            PlaybackStatus::Playing => self.status = PlaybackStatus::Paused,
            PlaybackStatus::Paused => self.status = PlaybackStatus::Playing,
            PlaybackStatus::Finished => {
                self.current_frame = 0;
                self.completed_loops = 0;
                self.status = PlaybackStatus::Playing;
                return true;
            }
        }
        false
    }

    /// Moves to the frame after the current one, once it is due and ready.
    /// After the last frame comes the first if the loop count allows another
    /// play; otherwise the animation finishes on the last frame.
    pub fn advance(&mut self) -> Advance {
        if self.status != PlaybackStatus::Playing {
            return Advance::Idle;
        }
        if self.current_frame + 1 < self.frame_count {
            self.current_frame += 1;
            return Advance::Next;
        }
        if self.another_play() {
            self.completed_loops = self.completed_loops.saturating_add(1);
            self.current_frame = 0;
            Advance::Wrapped
        } else {
            self.status = PlaybackStatus::Finished;
            Advance::Finished
        }
    }

    /// Moves to `frame_index`, or to the last frame if it is beyond it. A
    /// finished animation becomes paused, and playing it then ends the
    /// current play, as the plays done before still count.
    pub fn seek(&mut self, frame_index: usize) {
        self.current_frame = frame_index.min(self.frame_count - 1);
        if self.status == PlaybackStatus::Finished {
            self.status = PlaybackStatus::Paused;
        }
    }

    /// Starts dragging the seek bar, which pauses until
    /// [`end_scrub`](Self::end_scrub).
    pub fn begin_scrub(&mut self) {
        if self.resume_after_scrub.is_none() {
            self.resume_after_scrub = Some(self.is_playing());
            self.status = PlaybackStatus::Paused;
        }
    }

    /// Moves to `frame_index` while dragging the seek bar, as
    /// [`seek`](Self::seek) does.
    pub fn scrub_to(&mut self, frame_index: usize) {
        self.seek(frame_index);
    }

    /// Lets go of the seek bar, playing again if it played before. A finished
    /// animation stays paused.
    pub fn end_scrub(&mut self) {
        if let Some(resume) = self.resume_after_scrub.take() {
            self.status = if resume {
                PlaybackStatus::Playing
            } else {
                PlaybackStatus::Paused
            };
        }
    }

    /// How fast the animation plays, as a multiple of the speed its delays
    /// give: 2 shows each frame for half its delay.
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Plays at `speed` if it is from 0.05 to 16. Returns whether it is, as
    /// otherwise the speed stays.
    pub fn set_speed(&mut self, speed: f32) -> bool {
        let valid = PLAYBACK_SPEED_RANGE.contains(&speed);
        if valid {
            self.speed = speed;
        }
        valid
    }

    /// Plays at the speed the file records.
    pub fn reset_speed(&mut self) {
        self.speed = DEFAULT_PLAYBACK_SPEED;
    }

    /// Plays at the fastest preset slower than the current speed: 0.25, 0.5,
    /// 1, 1.5, 2 or 3. Returns whether there is one.
    pub fn decrease_speed(&mut self) -> bool {
        let Some(speed) = slower_preset(self.speed) else {
            return false;
        };
        self.speed = speed;
        true
    }

    /// Plays at the slowest preset faster than the current speed. Returns
    /// whether there is one.
    pub fn increase_speed(&mut self) -> bool {
        let Some(speed) = faster_preset(self.speed) else {
            return false;
        };
        self.speed = speed;
        true
    }

    /// Whether [`decrease_speed`](Self::decrease_speed) changes the speed.
    #[must_use]
    pub fn can_decrease_speed(&self) -> bool {
        slower_preset(self.speed).is_some()
    }

    /// Whether [`increase_speed`](Self::increase_speed) changes the speed.
    #[must_use]
    pub fn can_increase_speed(&self) -> bool {
        faster_preset(self.speed).is_some()
    }

    /// How long a frame of `delay` is shown at the current speed.
    #[must_use]
    pub fn scaled_delay(&self, delay: Duration) -> Duration {
        scale_delay(delay, self.speed)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;

    fn finite(plays: u32) -> LoopCount {
        LoopCount::Finite(NonZeroU32::new(plays).unwrap())
    }

    /// A state that played `plays` times through 3 frames and finished.
    fn finished(plays: u32) -> PlaybackState {
        let mut state = PlaybackState::new(3, finite(plays));
        while state.advance() != Advance::Finished {}
        state
    }

    #[test]
    fn starts_playing_the_first_frame() {
        let state = PlaybackState::new(3, LoopCount::Infinite);
        assert_eq!(state.status(), PlaybackStatus::Playing);
        assert!(state.is_playing());
        assert!(!state.is_finished());
        assert!(!state.is_scrubbing());
        assert_eq!(state.current_frame(), 0);
        assert_eq!(state.frame_count(), 3);
    }

    #[test]
    fn advances_through_the_frames() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        assert_eq!(state.advance(), Advance::Next);
        assert_eq!(state.current_frame(), 1);
        assert_eq!(state.advance(), Advance::Next);
        assert_eq!(state.current_frame(), 2);
    }

    #[test]
    fn infinite_loop_wraps_forever() {
        let mut state = PlaybackState::new(2, LoopCount::Infinite);
        for _ in 0..100 {
            assert_eq!(state.advance(), Advance::Next);
            assert_eq!(state.advance(), Advance::Wrapped);
            assert_eq!(state.current_frame(), 0);
            assert!(state.is_playing());
        }
    }

    #[test]
    fn finite_loop_stops_on_the_last_frame() {
        let mut state = PlaybackState::new(3, finite(2));
        let steps: Vec<Advance> = (0..5).map(|_| state.advance()).collect();
        assert_eq!(
            steps,
            [
                Advance::Next,
                Advance::Next,
                Advance::Wrapped,
                Advance::Next,
                Advance::Next,
            ]
        );
        assert_eq!(state.advance(), Advance::Finished);
        assert_eq!(state.status(), PlaybackStatus::Finished);
        assert!(state.is_finished());
        assert_eq!(state.current_frame(), 2);
        // Nothing moves any more.
        assert_eq!(state.advance(), Advance::Idle);
        assert_eq!(state.current_frame(), 2);
    }

    #[test]
    fn single_play_finishes_without_wrapping() {
        let mut state = PlaybackState::new(2, finite(1));
        assert_eq!(state.advance(), Advance::Next);
        assert_eq!(state.advance(), Advance::Finished);
        assert_eq!(state.current_frame(), 1);
    }

    #[test]
    fn toggle_pauses_and_plays() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        state.advance();
        assert!(!state.toggle_play_pause());
        assert_eq!(state.status(), PlaybackStatus::Paused);
        assert_eq!(state.advance(), Advance::Idle);
        assert_eq!(state.current_frame(), 1);
        assert!(!state.toggle_play_pause());
        assert_eq!(state.status(), PlaybackStatus::Playing);
        assert_eq!(state.current_frame(), 1);
    }

    #[test]
    fn toggle_restarts_a_finished_animation_with_all_its_loops() {
        let mut state = finished(2);
        assert!(state.toggle_play_pause());
        assert_eq!(state.status(), PlaybackStatus::Playing);
        assert_eq!(state.current_frame(), 0);
        // Both plays again.
        let steps: Vec<Advance> = (0..6).map(|_| state.advance()).collect();
        assert_eq!(
            steps
                .iter()
                .filter(|&&step| step == Advance::Wrapped)
                .count(),
            1
        );
        assert_eq!(steps.last(), Some(&Advance::Finished));
    }

    #[test]
    fn seek_keeps_playing_or_paused() {
        let mut state = PlaybackState::new(5, LoopCount::Infinite);
        state.seek(3);
        assert_eq!(state.current_frame(), 3);
        assert_eq!(state.status(), PlaybackStatus::Playing);

        assert!(!state.toggle_play_pause());
        state.seek(1);
        assert_eq!(state.current_frame(), 1);
        assert_eq!(state.status(), PlaybackStatus::Paused);
    }

    #[test]
    fn seek_from_finished_pauses() {
        let mut state = finished(1);
        state.seek(1);
        assert_eq!(state.current_frame(), 1);
        assert_eq!(state.status(), PlaybackStatus::Paused);
    }

    #[test]
    fn playing_after_seeking_from_finished_ends_the_current_play() {
        let mut state = finished(2);
        state.seek(0);
        assert!(!state.toggle_play_pause());
        assert_eq!(state.advance(), Advance::Next);
        assert_eq!(state.advance(), Advance::Next);
        assert_eq!(state.advance(), Advance::Finished);
    }

    #[test]
    fn seek_clamps_to_the_last_frame() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        state.seek(10);
        assert_eq!(state.current_frame(), 2);
        state.scrub_to(usize::MAX);
        assert_eq!(state.current_frame(), 2);
    }

    #[test]
    fn scrub_pauses_and_resumes_playing() {
        let mut state = PlaybackState::new(5, LoopCount::Infinite);
        state.begin_scrub();
        assert!(state.is_scrubbing());
        assert_eq!(state.status(), PlaybackStatus::Paused);
        assert_eq!(state.advance(), Advance::Idle);
        state.scrub_to(4);
        state.scrub_to(2);
        assert_eq!(state.current_frame(), 2);
        state.end_scrub();
        assert!(!state.is_scrubbing());
        assert_eq!(state.status(), PlaybackStatus::Playing);
        assert_eq!(state.current_frame(), 2);
    }

    #[test]
    fn scrub_stays_paused_if_paused() {
        let mut state = PlaybackState::new(5, LoopCount::Infinite);
        assert!(!state.toggle_play_pause());
        state.begin_scrub();
        state.scrub_to(3);
        state.end_scrub();
        assert_eq!(state.status(), PlaybackStatus::Paused);
        assert_eq!(state.current_frame(), 3);
    }

    #[test]
    fn scrub_from_finished_ends_paused() {
        let mut state = finished(1);
        state.begin_scrub();
        assert_eq!(state.status(), PlaybackStatus::Paused);
        state.scrub_to(0);
        state.end_scrub();
        assert_eq!(state.status(), PlaybackStatus::Paused);
        assert_eq!(state.current_frame(), 0);
    }

    #[test]
    fn scrub_begun_twice_remembers_the_first_status() {
        let mut state = PlaybackState::new(5, LoopCount::Infinite);
        state.begin_scrub();
        state.begin_scrub();
        state.end_scrub();
        assert_eq!(state.status(), PlaybackStatus::Playing);
    }

    #[test]
    fn end_scrub_without_begin_does_nothing() {
        let mut state = PlaybackState::new(5, LoopCount::Infinite);
        assert!(!state.toggle_play_pause());
        state.end_scrub();
        assert_eq!(state.status(), PlaybackStatus::Paused);
    }

    #[test]
    fn toggle_while_scrubbing_changes_what_follows() {
        let mut state = PlaybackState::new(5, LoopCount::Infinite);
        state.begin_scrub();
        assert!(!state.toggle_play_pause());
        assert_eq!(state.status(), PlaybackStatus::Paused);
        state.end_scrub();
        assert_eq!(state.status(), PlaybackStatus::Paused);

        state.begin_scrub();
        assert!(!state.toggle_play_pause());
        state.end_scrub();
        assert_eq!(state.status(), PlaybackStatus::Playing);
    }

    #[test]
    fn next_frame_is_the_one_after_the_current_while_playing() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        assert_eq!(state.next_frame_index(), Some(1));
        state.advance();
        assert_eq!(state.next_frame_index(), Some(2));
    }

    #[test]
    fn no_next_frame_unless_playing() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        assert!(!state.toggle_play_pause());
        assert_eq!(state.next_frame_index(), None);

        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        state.begin_scrub();
        assert_eq!(state.next_frame_index(), None);
        state.end_scrub();
        assert_eq!(state.next_frame_index(), Some(1));

        assert_eq!(finished(2).next_frame_index(), None);
    }

    #[test]
    fn next_frame_after_the_last_is_the_first_while_plays_remain() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        state.seek(2);
        assert_eq!(state.next_frame_index(), Some(0));

        let mut state = PlaybackState::new(3, finite(2));
        state.seek(2);
        assert_eq!(state.next_frame_index(), Some(0));
        assert_eq!(state.advance(), Advance::Wrapped);
        // The last play.
        state.seek(2);
        assert_eq!(state.next_frame_index(), None);
        assert_eq!(state.advance(), Advance::Finished);
    }

    #[test]
    fn single_play_has_no_next_frame_on_the_last() {
        let mut state = PlaybackState::new(2, finite(1));
        assert_eq!(state.next_frame_index(), Some(1));
        state.advance();
        assert_eq!(state.next_frame_index(), None);
    }

    #[test]
    fn no_next_frame_on_the_last_after_seeking_from_finished() {
        let mut state = finished(2);
        state.seek(1);
        assert!(!state.toggle_play_pause());
        assert_eq!(state.next_frame_index(), Some(2));
        state.advance();
        assert_eq!(state.next_frame_index(), None);
    }

    #[test]
    fn next_frame_is_where_advance_goes() {
        for loop_count in [LoopCount::Infinite, finite(1), finite(3)] {
            let mut state = PlaybackState::new(3, loop_count);
            for _ in 0..12 {
                let next = state.next_frame_index();
                let step = state.advance();
                match next {
                    Some(frame) => {
                        assert!(matches!(step, Advance::Next | Advance::Wrapped));
                        assert_eq!(state.current_frame(), frame);
                    }
                    None => assert!(matches!(step, Advance::Finished | Advance::Idle)),
                }
            }
        }
    }

    #[test]
    fn wraps_left_counts_the_plays_after_the_current() {
        assert_eq!(
            PlaybackState::new(3, LoopCount::Infinite).wraps_left(),
            None
        );
        let mut state = PlaybackState::new(2, finite(3));
        assert_eq!(state.wraps_left(), Some(2));
        state.advance();
        assert_eq!(state.advance(), Advance::Wrapped);
        assert_eq!(state.wraps_left(), Some(1));

        let mut state = finished(3);
        assert_eq!(state.wraps_left(), Some(0));
        // The plays done before a seek still count.
        state.seek(0);
        assert_eq!(state.wraps_left(), Some(0));
        state.seek(2);
        assert!(!state.toggle_play_pause());
        assert_eq!(state.wraps_left(), Some(0));

        let mut state = finished(3);
        assert!(state.toggle_play_pause());
        assert_eq!(state.wraps_left(), Some(2));
    }

    #[track_caller]
    fn assert_speed(state: &PlaybackState, expected: f32) {
        let speed = state.speed();
        assert!(
            (speed - expected).abs() < f32::EPSILON,
            "{speed} is not {expected}"
        );
    }

    /// A state that plays at `speed`.
    fn at_speed(speed: f32) -> PlaybackState {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        assert!(state.set_speed(speed), "{speed}");
        state
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn plays_at_the_recorded_speed_at_first() {
        let state = PlaybackState::new(3, LoopCount::Infinite);
        assert_speed(&state, 1.0);
        assert_eq!(state.scaled_delay(ms(100)), ms(100));
    }

    #[test]
    fn set_speed_takes_speeds_in_range() {
        let mut state = PlaybackState::new(3, LoopCount::Infinite);
        for speed in [0.05, 0.125, 1.2, 16.0] {
            assert!(state.set_speed(speed), "{speed}");
            assert_speed(&state, speed);
        }
    }

    #[test]
    fn set_speed_keeps_the_speed_for_others() {
        let mut state = at_speed(2.0);
        for speed in [
            0.0,
            -0.0,
            -1.0,
            0.049,
            16.01,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            assert!(!state.set_speed(speed), "{speed}");
            assert_speed(&state, 2.0);
        }
    }

    #[test]
    fn reset_speed_goes_back_to_the_recorded_speed() {
        let mut state = at_speed(0.3);
        state.reset_speed();
        assert_speed(&state, 1.0);
    }

    #[test]
    fn decrease_steps_down_through_the_presets() {
        let mut state = at_speed(3.0);
        for expected in [2.0, 1.5, 1.0, 0.5, 0.25] {
            assert!(state.can_decrease_speed());
            assert!(state.decrease_speed());
            assert_speed(&state, expected);
        }
        assert!(!state.can_decrease_speed());
        assert!(!state.decrease_speed());
        assert_speed(&state, 0.25);
    }

    #[test]
    fn increase_steps_up_through_the_presets() {
        let mut state = at_speed(0.25);
        for expected in [0.5, 1.0, 1.5, 2.0, 3.0] {
            assert!(state.can_increase_speed());
            assert!(state.increase_speed());
            assert_speed(&state, expected);
        }
        assert!(!state.can_increase_speed());
        assert!(!state.increase_speed());
        assert_speed(&state, 3.0);
    }

    #[test]
    fn steps_from_a_speed_between_presets_to_the_nearest() {
        let mut state = at_speed(1.2);
        assert!(state.decrease_speed());
        assert_speed(&state, 1.0);

        let mut state = at_speed(1.2);
        assert!(state.increase_speed());
        assert_speed(&state, 1.5);
    }

    #[test]
    fn steps_from_beyond_the_presets_toward_them() {
        let mut state = at_speed(0.1);
        assert!(!state.can_decrease_speed());
        assert!(!state.decrease_speed());
        assert_speed(&state, 0.1);
        assert!(state.increase_speed());
        assert_speed(&state, 0.25);

        let mut state = at_speed(16.0);
        assert!(!state.can_increase_speed());
        assert!(!state.increase_speed());
        assert_speed(&state, 16.0);
        assert!(state.decrease_speed());
        assert_speed(&state, 3.0);
    }

    #[test]
    fn speed_stays_through_plays_seeks_and_scrubs() {
        let mut state = PlaybackState::new(3, finite(1));
        assert!(state.set_speed(2.0));
        while state.advance() != Advance::Finished {}
        assert!(state.toggle_play_pause());
        state.seek(2);
        state.begin_scrub();
        state.scrub_to(1);
        state.end_scrub();
        assert_speed(&state, 2.0);
    }

    #[test]
    fn delays_are_divided_by_the_speed() {
        assert_eq!(at_speed(2.0).scaled_delay(ms(100)), ms(50));
        assert_eq!(at_speed(0.5).scaled_delay(ms(100)), ms(200));
        assert_eq!(at_speed(0.25).scaled_delay(ms(100)), ms(400));
        assert_eq!(scale_delay(ms(10), 3.0), Duration::from_nanos(3_333_333));
        assert_eq!(scale_delay(ms(250), 1.0), ms(250));
    }

    #[test]
    fn delay_of_0_stays_0_at_any_speed() {
        for speed in [0.05, 1.0, 16.0] {
            assert_eq!(scale_delay(Duration::ZERO, speed), Duration::ZERO);
        }
    }

    #[test]
    fn delays_too_long_to_hold_saturate() {
        assert_eq!(scale_delay(Duration::MAX, 0.05), Duration::MAX);
        assert_eq!(scale_delay(Duration::MAX, 0.5), Duration::MAX);
        assert_eq!(
            scale_delay(Duration::from_secs(u64::MAX / 2), 0.25),
            Duration::MAX
        );
    }

    #[test]
    fn parses_speeds_with_spaces_and_an_x() {
        for (text, expected) in [
            ("1", 1.0),
            ("1.5", 1.5),
            ("  0.25 ", 0.25),
            ("2x", 2.0),
            ("2X", 2.0),
            ("0.5 x", 0.5),
            (" 3x  ", 3.0),
            (".5", 0.5),
            ("0.05", 0.05),
            ("16", 16.0),
            ("1.2", 1.2),
        ] {
            assert_eq!(parse_speed(text), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn rejects_text_that_is_not_a_speed() {
        for text in [
            "", " ", "x", "abc", "0", "-1", "0.049", "16.01", "NaN", "inf", "-inf", "1.5xx", "x2",
            "1,5", "1.5 2",
        ] {
            assert_eq!(parse_speed(text), None, "{text:?}");
        }
    }

    #[test]
    fn formats_speeds_without_trailing_zeros() {
        for (speed, expected) in [
            (1.0, "1"),
            (0.25, "0.25"),
            (1.5, "1.5"),
            (1.25, "1.25"),
            (0.05, "0.05"),
            (0.125, "0.125"),
            (1.2, "1.2"),
            (3.0, "3"),
            (16.0, "16"),
        ] {
            assert_eq!(format_speed(speed), expected);
        }
    }

    #[test]
    fn formatted_speeds_parse_back_the_same() {
        for speed in PLAYBACK_SPEED_PRESETS
            .into_iter()
            .chain([0.05, 0.125, 1.2, 0.33, 16.0])
        {
            assert_eq!(parse_speed(&format_speed(speed)), Some(speed), "{speed}");
        }
    }

    #[test]
    fn fewer_than_two_frames_do_not_underflow() {
        for frame_count in [0, 1] {
            let mut state = PlaybackState::new(frame_count, finite(2));
            assert_eq!(state.frame_count(), 1);
            assert_eq!(state.advance(), Advance::Wrapped);
            assert_eq!(state.advance(), Advance::Finished);
            assert_eq!(state.current_frame(), 0);
            state.seek(5);
            assert_eq!(state.current_frame(), 0);

            let mut state = PlaybackState::new(frame_count, LoopCount::Infinite);
            assert_eq!(state.advance(), Advance::Wrapped);
            assert_eq!(state.current_frame(), 0);
        }
    }
}
