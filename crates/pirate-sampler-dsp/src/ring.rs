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

    /// Write one stereo frame. No-op when paused. RT-safe.
    #[inline]
    pub fn write_frame(&mut self, left: f32, right: f32) {
        if self.paused || self.capacity == 0 {
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
        let i = self.physical_index(logical);
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
}
