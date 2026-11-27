//! The state of playing an animation: which frame is shown, and whether it
//! plays. It knows nothing of how frames are decoded or when they are due.

use crate::LoopCount;

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

/// Which frame of an animation is shown, whether it plays, and how many
/// times it has been played.
#[derive(Debug, Clone, PartialEq, Eq)]
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
