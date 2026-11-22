//! Fixed-length stereo ring buffer (SPEC: overwrite oldest when full).

use crate::params::{buffer_capacity_frames, BufferDuration, NUM_CHANNELS};

/// Inclusive-exclusive selection in "logical" timeline frames:
/// `0` is the oldest retained sample, `len()` is one past the newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}

impl Selection {
    pub fn new(start: usize, end: usize) -> Option<Self> {
        if start < end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    pub fn len(self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

/// Preallocated stereo ring buffer. Audio-thread writes must not allocate.
#[derive(Debug)]
pub struct RingBuffer {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Allocated storage length.
    storage: usize,
    /// Active ring length (`<= storage`). Changing this is RT-safe if `<= storage`.
    capacity: usize,
    write_pos: usize,
    /// Number of valid frames currently stored (`<= capacity`).
    filled: usize,
    /// Frames written since the timeline was last reset (absolute indexing for playback).
    written_total: u64,
    /// Absolute frame index of the next sample to output, when a playhead is cued.
    play_abs: Option<u64>,
    /// Absolute frame index one past the last sample the current playback run may read.
    play_end_abs: u64,
    /// Start of the loop, or None for one-shot playback.
    loop_start_abs: Option<u64>,
    /// Frames the captured audio has been rotated under the playhead (audition scrubbing).
    shift: i64,
    /// Whether playback is currently advancing.
    playing: bool,
    paused: bool,
    sample_rate: f32,
    recent_peak_l: f32,
    recent_peak_r: f32,
}

impl RingBuffer {
    pub fn new(sample_rate: f32, duration: BufferDuration) -> Self {
        let capacity = buffer_capacity_frames(sample_rate, duration);
        Self::with_capacity(sample_rate, capacity)
    }

    /// Allocate for the maximum v1 duration at `sample_rate`, start at `duration`.
    pub fn with_max_duration(sample_rate: f32, duration: BufferDuration) -> Self {
        let storage = buffer_capacity_frames(
            sample_rate,
            BufferDuration::new(crate::BUFFER_DURATION_MAX_SECS),
        );
        let active = buffer_capacity_frames(sample_rate, duration).min(storage);
        let mut rb = Self::with_capacity(sample_rate, storage);
        rb.capacity = active.max(1);
        rb
    }

    /// Construct with an explicit frame capacity (tests / offline harness).
    pub fn with_capacity(sample_rate: f32, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            left: vec![0.0; capacity],
            right: vec![0.0; capacity],
            storage: capacity,
            capacity,
            write_pos: 0,
            filled: 0,
            written_total: 0,
            play_abs: None,
            play_end_abs: 0,
            loop_start_abs: None,
            shift: 0,
            playing: false,
            paused: false,
            sample_rate: sample_rate.max(1.0),
            recent_peak_l: 0.0,
            recent_peak_r: 0.0,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn capacity_frames(&self) -> usize {
        self.capacity
    }

    pub fn filled_frames(&self) -> usize {
        self.filled
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Change active duration without reallocating when storage is large enough.
    ///
    /// Preserves existing audio without heap allocations:
    /// - If the buffer expands, existing audio is kept intact.
    /// - If the buffer shrinks below current filled frames, only the oldest audio
    ///   is dropped, keeping the most recent `new_capacity` frames.
    ///
    /// RT-safe if the new capacity fits in `storage`.
    pub fn set_duration_rt_safe(&mut self, duration: BufferDuration) -> bool {
        let needed = buffer_capacity_frames(self.sample_rate, duration);
        if needed > self.storage {
            return false;
        }
        let new_cap = needed.max(1);
        if new_cap == self.capacity {
            return true;
        }

        // Fold any audition rotation into the audio before the storage layout changes.
        self.bake_shift();

        // Linearize active filled slice so index 0 is oldest and filled-1 is newest
        if self.filled == self.capacity && self.write_pos > 0 && self.write_pos < self.capacity {
            self.left[..self.capacity].rotate_left(self.write_pos);
            self.right[..self.capacity].rotate_left(self.write_pos);
            self.write_pos = 0;
        }

        if new_cap < self.filled {
            // Buffer shrunk: drop oldest excess frames, keeping the most recent new_cap frames
            let excess = self.filled - new_cap;
            self.left.copy_within(excess..self.filled, 0);
            self.right.copy_within(excess..self.filled, 0);
            self.filled = new_cap;
            self.write_pos = 0;
        } else {
            // Buffer expanded: keep existing audio, write_pos continues at filled
            self.write_pos = if self.filled == new_cap {
                0
            } else {
                self.filled
            };
            // Zero out unused area in new capacity
            for i in self.filled..new_cap {
                self.left[i] = 0.0;
                self.right[i] = 0.0;
            }
        }

        self.capacity = new_cap;
        // The buffer was linearized above, so absolute playback indexing restarts here.
        self.written_total = self.filled as u64;
        self.play_abs = None;
        self.play_end_abs = 0;
        self.loop_start_abs = None;
        self.playing = false;
        true
    }

    /// Reallocate storage (NOT RT-safe). Used on sample-rate changes.
    pub fn reinit(&mut self, sample_rate: f32, duration: BufferDuration) {
        *self = Self::with_max_duration(sample_rate, duration);
    }

    /// Clear buffer contents without reallocating.
    pub fn clear(&mut self) {
        // Only clear active region; storage tail can keep junk.
        for i in 0..self.capacity {
            self.left[i] = 0.0;
            self.right[i] = 0.0;
        }
        self.write_pos = 0;
        self.filled = 0;
        self.written_total = 0;
        self.play_abs = None;
        self.play_end_abs = 0;
        self.loop_start_abs = None;
        self.shift = 0;
        self.playing = false;
        self.recent_peak_l = 0.0;
        self.recent_peak_r = 0.0;
    }

    /// Read recent peak level and decay it slightly for smooth visual metering.
    pub fn read_and_decay_peak(&mut self) -> (f32, f32) {
        let p = (self.recent_peak_l, self.recent_peak_r);
        self.recent_peak_l *= 0.88;
        self.recent_peak_r *= 0.88;
        p
    }

    /// Write one stereo frame. No-op when paused or while playing back. RT-safe.
    ///
    /// Recording is held still during playback: a rolling buffer would otherwise overwrite the
    /// region being auditioned at the same rate it is consumed, and a monitored output would be
    /// captured straight back into the buffer.
    #[inline]
    pub fn write_frame(&mut self, left: f32, right: f32) {
        if self.paused || self.playing || self.capacity == 0 {
            return;
        }
        let i = self.write_pos;
        let l = sanitize(left);
        let r = sanitize(right);
        self.left[i] = l;
        self.right[i] = r;

        let abs_l = l.abs();
        let abs_r = r.abs();
        if abs_l > self.recent_peak_l {
            self.recent_peak_l = abs_l;
        }
        if abs_r > self.recent_peak_r {
            self.recent_peak_r = abs_r;
        }

        self.write_pos = (i + 1) % self.capacity;
        if self.filled < self.capacity {
            self.filled += 1;
        }
        self.written_total += 1;
    }

    /// Write interleaved stereo samples `[L,R,L,R,...]`. RT-safe if `samples` is given.
    pub fn write_interleaved(&mut self, samples: &[f32]) {
        let mut i = 0;
        while i + 1 < samples.len() {
            self.write_frame(samples[i], samples[i + 1]);
            i += 2;
        }
    }

    /// Logical length available for selection / export.
    pub fn len(&self) -> usize {
        self.filled
    }

    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Absolute index of the oldest retained frame.
    #[inline]
    fn oldest_abs(&self) -> u64 {
        self.written_total.saturating_sub(self.filled as u64)
    }

    /// Whether playback is currently advancing.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether playback is a running selection loop, as opposed to a one-shot pass.
    pub fn is_looping(&self) -> bool {
        self.playing && self.loop_start_abs.is_some()
    }

    /// Cue the playhead at normalized `start` and play through to normalized `end`
    /// (`0.0` = oldest retained frame, `1.0` = newest), returning whether playback started.
    ///
    /// This is how a selection is auditioned: the run stops at `end` instead of the live edge,
    /// and both bounds are resolved against the frames that exist right now.
    pub fn play_range(&mut self, start: f32, end: f32) -> bool {
        if self.filled == 0 {
            return false;
        }
        self.bake_shift();
        let oldest = self.oldest_abs();
        let filled = self.filled as u64;
        let start_idx =
            ((start.clamp(0.0, 1.0) * self.filled as f32).floor() as u64).min(filled - 1);
        let end_idx = ((end.clamp(0.0, 1.0) * self.filled as f32).ceil() as u64).min(filled);
        if end_idx <= start_idx {
            self.playing = false;
            self.play_abs = None;
            return false;
        }
        self.play_abs = Some(oldest + start_idx);
        self.play_end_abs = oldest + end_idx;
        self.loop_start_abs = None;
        self.playing = true;
        true
    }

    /// Loop a selection, starting from `from` when it falls within the selection.
    pub fn loop_range(&mut self, start: f32, end: f32, from: f32) -> bool {
        if !self.play_range(start, end) {
            return false;
        }
        let loop_start = self.play_abs.expect("successful playback has a playhead");
        self.loop_start_abs = Some(loop_start);
        let cue = self.oldest_abs() + ((from.clamp(0.0, 1.0) * self.filled as f32).floor() as u64);
        if (loop_start..self.play_end_abs).contains(&cue) {
            self.play_abs = Some(cue);
        }
        true
    }

    /// Resize the loop that is currently running, without stopping or re-cueing it.
    ///
    /// `start` and `end` are normalized buffer positions resolved like [`Self::play_range`]. The
    /// playhead keeps its place while it stays inside the new window; if the window moved past
    /// it, it is cued at the new loop start so playback continues through the new range. Returns
    /// `false` when no loop is running or the range is empty.
    pub fn resize_loop(&mut self, start: f32, end: f32) -> bool {
        if !self.playing || self.filled == 0 || self.loop_start_abs.is_none() {
            return false;
        }
        let oldest = self.oldest_abs();
        let filled = self.filled as u64;
        let start_idx =
            ((start.clamp(0.0, 1.0) * self.filled as f32).floor() as u64).min(filled - 1);
        let end_idx = ((end.clamp(0.0, 1.0) * self.filled as f32).ceil() as u64).min(filled);
        if end_idx <= start_idx {
            return false;
        }
        let new_start = oldest + start_idx;
        let new_end = oldest + end_idx;
        self.loop_start_abs = Some(new_start);
        self.play_end_abs = new_end;
        if self
            .play_abs
            .is_none_or(|pos| !(new_start..new_end).contains(&pos))
        {
            self.play_abs = Some(new_start);
        }
        true
    }

    /// Frames the captured audio has been rotated under the playhead (positive = moved later).
    pub fn shift_frames(&self) -> i64 {
        self.shift
    }

    /// Rotate the captured audio under the playhead by `delta` frames while a loop plays.
    ///
    /// The loop window and playhead stay where they are: the audio moves underneath them, so the
    /// loop auditions whatever material was scrubbed into it. Recording is frozen during playback,
    /// so this is a pure edit and is folded into the buffers when playback stops (see
    /// [`Self::bake_shift`]). The rotation wraps inside the filled audio. Returns the applied delta.
    pub fn shift_content(&mut self, delta: i64) -> i64 {
        if !self.is_looping() || self.filled == 0 || delta == 0 {
            return 0;
        }
        self.shift += delta;
        delta
    }

    /// Fold the audition rotation into the stored audio so a scrub survives after playback stops.
    ///
    /// The rotation is O(filled). Callers that share this buffer with the audio thread (the GUI)
    /// must not hold the ring lock across this call; take the buffer out of its lock first.
    pub fn bake_scrub(&mut self) {
        self.bake_shift();
    }

    /// Fold the audition rotation into the stored audio so a scrub survives after playback stops.
    fn bake_shift(&mut self) {
        let filled = self.filled;
        if filled == 0 || self.shift == 0 {
            self.shift = 0;
            return;
        }
        let s = self.shift.rem_euclid(filled as i64) as usize;
        if s > 0 {
            // Only the active region is meaningful; storage past `filled` may hold stale audio.
            self.left[..filled].rotate_right(s);
            self.right[..filled].rotate_right(s);
        }
        self.shift = 0;
    }

    /// Wrap a logical offset (possibly negative) into the filled audio.
    #[inline]
    fn wrapped_logical(&self, logical: i64) -> usize {
        debug_assert!(self.filled > 0);
        (logical.rem_euclid(self.filled as i64) as usize).min(self.filled - 1)
    }

    /// Stop advancing but keep the playhead cued where it stopped.
    ///
    /// Audio scrubbed under the loop is folded into the buffer, so the move survives the stop.
    pub fn pause_playback(&mut self) {
        self.bake_shift();
        self.playing = false;
    }

    /// Playhead position as a normalized buffer position, if one is cued.
    pub fn playhead_fraction(&self) -> Option<f32> {
        if self.filled == 0 {
            return None;
        }
        let pos = self.play_abs?;
        let oldest = self.oldest_abs();
        if pos < oldest || pos >= self.written_total {
            return None;
        }
        Some(((pos - oldest) as f32 / self.filled as f32).clamp(0.0, 1.0))
    }

    /// Pop the next stereo frame for output. RT-safe.
    ///
    /// Playback stops or loops at the end of the requested range; recording is frozen until
    /// playback ends or is paused.
    #[inline]
    pub fn next_playback_frame(&mut self) -> Option<(f32, f32)> {
        if !self.playing {
            return None;
        }
        let Some(pos) = self.play_abs else {
            self.playing = false;
            return None;
        };
        if pos >= self.play_end_abs || pos >= self.written_total || pos < self.oldest_abs() {
            self.playing = false;
            self.play_abs = None;
            // Only one-shot playback stops here. Scrubbing is confined to loops, so there is no
            // rotation to bake on the audio thread; rotating O(filled) samples in `process` would
            // be an RT violation. Clear defensively rather than ever doing that work here.
            debug_assert_eq!(self.shift, 0, "one-shot playback cannot be scrubbed");
            self.shift = 0;
            return None;
        }
        let logical = pos - self.oldest_abs();
        let i = self.physical_index(self.wrapped_logical(logical as i64 - self.shift));
        let next = pos + 1;
        self.play_abs = Some(if next >= self.play_end_abs {
            self.loop_start_abs.unwrap_or(next)
        } else {
            next
        });
        Some((self.left[i], self.right[i]))
    }

    /// Map logical index (`0` = oldest) to physical storage index.
    #[inline]
    fn physical_index(&self, logical: usize) -> usize {
        debug_assert!(logical < self.filled);
        if self.filled < self.capacity {
            logical
        } else {
            (self.write_pos + logical) % self.capacity
        }
    }

    /// Read one frame at a logical index. Returns `None` if out of range.
    pub fn read_logical(&self, logical: usize) -> Option<(f32, f32)> {
        if logical >= self.filled {
            return None;
        }
        let i = self.physical_index(self.wrapped_logical(logical as i64 - self.shift));
        Some((self.left[i], self.right[i]))
    }

    /// Copy a logical selection into interleaved stereo `f32` samples.
    pub fn copy_selection_interleaved(&self, selection: Selection) -> Option<Vec<f32>> {
        if selection.end > self.filled || selection.start >= selection.end {
            return None;
        }
        let n = selection.len();
        let mut out = Vec::with_capacity(n * NUM_CHANNELS);
        for logical in selection.start..selection.end {
            let (l, r) = self.read_logical(logical)?;
            out.push(l);
            out.push(r);
        }
        Some(out)
    }

    /// Downsample peaks for UI: returns max abs per bucket for L/R as `(l, r)`.
    pub fn waveform_peaks(&self, buckets: usize) -> Vec<(f32, f32)> {
        self.waveform_peaks_range(buckets, 0.0, 1.0)
    }

    /// Downsample peaks for UI within a normalized view range `(view_start..=view_end)`.
    pub fn waveform_peaks_range(
        &self,
        buckets: usize,
        view_start: f32,
        view_end: f32,
    ) -> Vec<(f32, f32)> {
        if buckets == 0 || self.filled == 0 {
            return Vec::new();
        }
        let v_start = view_start.clamp(0.0, 1.0).min(view_end);
        let v_end = view_end.clamp(0.0, 1.0).max(v_start);

        let total_filled = self.filled as f32;
        let start_frame = (v_start * total_filled).floor() as usize;
        let end_frame = ((v_end * total_filled).ceil() as usize)
            .min(self.filled)
            .max(start_frame + 1);
        let span = end_frame.saturating_sub(start_frame).max(1);

        let mut peaks = Vec::with_capacity(buckets);
        for b in 0..buckets {
            let start = start_frame + b * span / buckets;
            let end = (start_frame + (b + 1) * span / buckets).max(start + 1);
            let mut max_l = 0.0f32;
            let mut max_r = 0.0f32;
            for logical in start..end.min(self.filled) {
                if let Some((l, r)) = self.read_logical(logical) {
                    max_l = max_l.max(l.abs());
                    max_r = max_r.max(r.abs());
                }
            }
            peaks.push((max_l, max_r));
        }
        peaks
    }
}

#[inline]
fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrite_oldest_when_full() {
        let mut rb = RingBuffer::with_capacity(48_000.0, 4);
        for i in 0..6 {
            let v = i as f32;
            rb.write_frame(v, -v);
        }
        // Logical oldest should be 2,3,4,5 (0 and 1 overwritten).
        assert_eq!(rb.filled_frames(), 4);
        assert_eq!(rb.read_logical(0), Some((2.0, -2.0)));
        assert_eq!(rb.read_logical(1), Some((3.0, -3.0)));
        assert_eq!(rb.read_logical(2), Some((4.0, -4.0)));
        assert_eq!(rb.read_logical(3), Some((5.0, -5.0)));
    }

    #[test]
    fn pause_freezes_contents() {
        let mut rb = RingBuffer::with_capacity(48_000.0, 8);
        rb.write_frame(1.0, 1.0);
        rb.write_frame(2.0, 2.0);
        rb.set_paused(true);
        rb.write_frame(99.0, 99.0);
        assert_eq!(rb.filled_frames(), 2);
        assert_eq!(rb.read_logical(0), Some((1.0, 1.0)));
        assert_eq!(rb.read_logical(1), Some((2.0, 2.0)));
    }

    #[test]
    fn selection_copy_matches_logical_order() {
        let mut rb = RingBuffer::with_capacity(44_100.0, 4);
        for i in 0..4 {
            rb.write_frame(i as f32, (i * 10) as f32);
        }
        let sel = Selection::new(1, 3).unwrap();
        let samples = rb.copy_selection_interleaved(sel).unwrap();
        assert_eq!(samples, vec![1.0, 10.0, 2.0, 20.0]);
    }

    #[test]
    fn nan_inputs_become_zero() {
        let mut rb = RingBuffer::with_capacity(48_000.0, 2);
        rb.write_frame(f32::NAN, f32::INFINITY);
        assert_eq!(rb.read_logical(0), Some((0.0, 0.0)));
    }

    #[test]
    fn set_duration_preserves_audio_when_expanding_and_shrinking() {
        // Storage for 60 seconds at 100 Hz = 6000 frames
        let sr = 100.0;
        let mut rb = RingBuffer::with_max_duration(sr, BufferDuration::new(30.0));
        let cap_30 = rb.capacity_frames(); // 3000 frames

        // Write 3500 frames (so buffer wraps around!)
        for i in 0..3500 {
            rb.write_frame(i as f32, -(i as f32));
        }
        // Buffer was full: logical 0 is 500, logical 2999 is 3499
        assert_eq!(rb.filled_frames(), cap_30);
        assert_eq!(rb.read_logical(0), Some((500.0, -500.0)));
        assert_eq!(rb.read_logical(cap_30 - 1), Some((3499.0, -3499.0)));

        // Expand duration to 45 seconds (4500 frames): existing 3000 frames must be preserved
        assert!(rb.set_duration_rt_safe(BufferDuration::new(45.0)));
        assert_eq!(rb.capacity_frames(), 4500);
        assert_eq!(rb.filled_frames(), 3000);
        assert_eq!(rb.read_logical(0), Some((500.0, -500.0)));
        assert_eq!(rb.read_logical(2999), Some((3499.0, -3499.0)));

        // Write 500 more frames
        for i in 3500..4000 {
            rb.write_frame(i as f32, -(i as f32));
        }
        assert_eq!(rb.filled_frames(), 3500);
        assert_eq!(rb.read_logical(3499), Some((3999.0, -3999.0)));

        // Shrink duration to 30 seconds (3000 frames): keeps the 3000 most recent frames [1000..3999]
        assert!(rb.set_duration_rt_safe(BufferDuration::new(30.0)));
        assert_eq!(rb.capacity_frames(), 3000);
        assert_eq!(rb.filled_frames(), 3000);
        assert_eq!(rb.read_logical(0), Some((1000.0, -1000.0)));
        assert_eq!(rb.read_logical(2999), Some((3999.0, -3999.0)));
    }

    /// Ring with `frames` frames whose sample value equals the logical index.
    fn filled_ring(frames: usize) -> RingBuffer {
        let mut rb = RingBuffer::with_capacity(48_000.0, (frames * 2).max(2));
        for i in 0..frames {
            rb.write_frame(i as f32, -(i as f32));
        }
        rb
    }

    #[test]
    fn playback_outputs_the_region_then_stops() {
        let mut rb = filled_ring(4);
        assert!(rb.play_range(0.0, 1.0));
        assert_eq!(rb.next_playback_frame(), Some((0.0, 0.0)));
        assert_eq!(rb.next_playback_frame(), Some((1.0, -1.0)));
        assert_eq!(rb.next_playback_frame(), Some((2.0, -2.0)));
        assert_eq!(rb.next_playback_frame(), Some((3.0, -3.0)));
        assert_eq!(rb.next_playback_frame(), None);
        assert!(!rb.is_playing(), "playback must stop at the region end");
        assert_eq!(rb.playhead_fraction(), None);
    }

    #[test]
    fn playback_is_scoped_to_the_requested_range() {
        let mut rb = filled_ring(8);
        assert!(rb.play_range(0.25, 0.5));
        assert!((rb.playhead_fraction().unwrap() - 0.25).abs() < 1e-6);
        assert_eq!(rb.next_playback_frame(), Some((2.0, -2.0)));
        assert_eq!(rb.next_playback_frame(), Some((3.0, -3.0)));
        assert_eq!(rb.next_playback_frame(), None, "must stop at end = 0.5");
        assert!(!rb.is_playing());
    }

    #[test]
    fn playback_cues_midway_through_the_buffer() {
        let mut rb = filled_ring(4);
        assert!(rb.play_range(0.5, 1.0));
        assert!((rb.playhead_fraction().unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(rb.next_playback_frame(), Some((2.0, -2.0)));
        assert_eq!(rb.next_playback_frame(), Some((3.0, -3.0)));
        assert_eq!(rb.next_playback_frame(), None);
    }

    #[test]
    fn playback_holds_the_buffer_still() {
        let mut rb = filled_ring(4);
        assert!(rb.play_range(0.0, 1.0));
        rb.write_frame(100.0, -100.0);
        assert_eq!(rb.filled_frames(), 4, "recording pauses during playback");
        assert_eq!(rb.next_playback_frame(), Some((0.0, 0.0)));
        assert_eq!(rb.next_playback_frame(), Some((1.0, -1.0)));
    }

    #[test]
    fn pause_keeps_the_cue_and_a_new_range_resumes_from_it() {
        let mut rb = filled_ring(4);
        assert!(rb.play_range(0.0, 1.0));
        assert_eq!(rb.next_playback_frame(), Some((0.0, 0.0)));
        rb.pause_playback();
        assert!(!rb.is_playing());
        assert_eq!(rb.next_playback_frame(), None);
        let parked = rb.playhead_fraction().unwrap();
        assert!((parked - 0.25).abs() < 1e-6);

        assert!(rb.play_range(parked, 1.0));
        assert_eq!(rb.next_playback_frame(), Some((1.0, -1.0)));
    }

    #[test]
    fn playback_indexes_correctly_after_the_ring_wraps() {
        let mut rb = RingBuffer::with_capacity(48_000.0, 4);
        for i in 0..8 {
            rb.write_frame(i as f32, -(i as f32));
        }
        // Retained logical frames are 4..8, so half way is frame 6.
        assert!(rb.play_range(0.5, 1.0));
        assert_eq!(rb.next_playback_frame(), Some((6.0, -6.0)));
        assert_eq!(rb.next_playback_frame(), Some((7.0, -7.0)));
        assert_eq!(rb.next_playback_frame(), None);
    }

    #[test]
    fn shifting_content_changes_what_the_loop_plays_without_moving_it() {
        let mut rb = filled_ring(8);
        assert!(rb.loop_range(0.25, 0.5, 0.25));
        assert_eq!(rb.next_playback_frame(), Some((2.0, -2.0)));

        assert_eq!(rb.shift_content(-2), -2, "audio moves two frames earlier");
        assert_eq!(rb.next_playback_frame(), Some((5.0, -5.0)));
        assert_eq!(rb.next_playback_frame(), Some((4.0, -4.0)));
        assert_eq!(
            rb.next_playback_frame(),
            Some((5.0, -5.0)),
            "the loop window itself never moves"
        );
        // The playhead keeps sweeping the same 2..4 window.
        assert_eq!(rb.playhead_fraction(), Some(0.25));
    }

    #[test]
    fn stopping_playback_folds_the_shift_into_the_audio() {
        let mut rb = filled_ring(8);
        assert!(rb.loop_range(0.25, 0.5, 0.25));
        assert_eq!(rb.shift_content(-2), -2);

        rb.pause_playback();
        assert_eq!(rb.shift_frames(), 0, "the rotation is baked in");
        assert_eq!(rb.read_logical(2), Some((4.0, -4.0)));
        assert_eq!(
            rb.read_logical(0),
            Some((2.0, -2.0)),
            "wrap-around preserved"
        );
    }

    #[test]
    fn shifting_a_full_buffer_wraps_inside_the_active_region() {
        let mut rb = RingBuffer::with_capacity(48_000.0, 4);
        for i in 0..8 {
            rb.write_frame(i as f32, -(i as f32));
        }
        // Retained logical frames are 4..8, storage is [4, 5, 6, 7].
        assert!(rb.loop_range(0.0, 0.5, 0.0));
        assert_eq!(rb.next_playback_frame(), Some((4.0, -4.0)));

        assert_eq!(rb.shift_content(-1), -1);
        assert_eq!(
            rb.next_playback_frame(),
            Some((6.0, -6.0)),
            "earlier audio moved into this slot"
        );

        rb.pause_playback();
        assert_eq!(rb.read_logical(0), Some((5.0, -5.0)));
        assert_eq!(
            rb.read_logical(3),
            Some((4.0, -4.0)),
            "the rotation wraps inside the filled audio"
        );
    }

    #[test]
    fn resizing_the_running_loop_changes_what_it_plays() {
        let mut rb = filled_ring(8);
        assert!(rb.loop_range(0.25, 0.5, 0.25));
        assert_eq!(rb.next_playback_frame(), Some((2.0, -2.0)));

        assert!(rb.resize_loop(0.25, 0.75), "the wider window takes over");
        assert_eq!(
            rb.next_playback_frame(),
            Some((3.0, -3.0)),
            "playback continues from the playhead"
        );
        assert_eq!(rb.next_playback_frame(), Some((4.0, -4.0)));
        assert_eq!(rb.next_playback_frame(), Some((5.0, -5.0)));
        assert_eq!(
            rb.next_playback_frame(),
            Some((2.0, -2.0)),
            "the loop wraps at the new wider end"
        );
    }

    #[test]
    fn resizing_pulls_the_playhead_into_the_new_window() {
        let mut rb = filled_ring(8);
        assert!(rb.loop_range(0.25, 0.5, 0.25));
        assert_eq!(rb.next_playback_frame(), Some((2.0, -2.0)));

        // Shrink the window so the playhead (now at 3) sits past its new end.
        assert!(rb.resize_loop(0.25, 0.375));
        assert_eq!(
            rb.next_playback_frame(),
            Some((2.0, -2.0)),
            "a playhead left outside the window re-cues at its start"
        );

        // Moving the start past the playhead also re-cues it.
        assert!(rb.resize_loop(0.5, 0.75));
        assert_eq!(rb.next_playback_frame(), Some((4.0, -4.0)));
    }

    #[test]
    fn resizing_needs_a_running_loop() {
        let mut rb = filled_ring(4);
        assert!(!rb.resize_loop(0.0, 0.5));
        assert!(rb.play_range(0.0, 1.0));
        assert!(
            !rb.resize_loop(0.0, 0.5),
            "one-shot playback has no loop to resize"
        );
    }

    #[test]
    fn shifting_content_needs_active_playback() {
        let mut rb = filled_ring(4);
        assert_eq!(rb.shift_content(2), 0);
        assert_eq!(rb.shift_frames(), 0);
    }

    #[test]
    fn shifting_content_needs_a_loop() {
        let mut rb = filled_ring(4);
        assert!(rb.play_range(0.0, 1.0));
        assert_eq!(rb.shift_content(2), 0, "one-shot playback cannot scrub");
        assert_eq!(rb.shift_frames(), 0);
    }

    #[test]
    fn empty_or_backwards_ranges_do_not_start_playback() {
        let mut rb = filled_ring(4);
        assert!(!rb.play_range(0.5, 0.5));
        assert!(!rb.is_playing());
        assert_eq!(rb.playhead_fraction(), None);

        let mut empty = RingBuffer::with_capacity(48_000.0, 4);
        assert!(!empty.play_range(0.0, 1.0));
        assert!(!empty.is_playing());
    }

    #[test]
    fn duration_change_resets_playback() {
        let mut rb = RingBuffer::with_max_duration(100.0, BufferDuration::new(30.0));
        for i in 0..100 {
            rb.write_frame(i as f32, -(i as f32));
        }
        assert!(rb.play_range(0.0, 1.0));
        assert!(rb.set_duration_rt_safe(BufferDuration::new(60.0)));
        assert!(!rb.is_playing());
        assert_eq!(rb.playhead_fraction(), None);
        assert!(
            rb.play_range(0.0, 1.0),
            "playback works on the new timeline"
        );
        assert_eq!(rb.next_playback_frame(), Some((0.0, 0.0)));
    }
}
