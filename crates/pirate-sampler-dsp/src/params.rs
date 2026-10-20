//! Parameter ranges from SPEC.md.

/// Stereo.
pub const NUM_CHANNELS: usize = 2;

pub const BUFFER_DURATION_MIN_SECS: f32 = 30.0;
pub const BUFFER_DURATION_MAX_SECS: f32 = 600.0; // 10 minutes
pub const BUFFER_DURATION_DEFAULT_SECS: f32 = 45.0;

/// User-facing buffer duration in seconds, clamped to the v1 range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BufferDuration(f32);

impl BufferDuration {
    pub fn new(seconds: f32) -> Self {
        Self(seconds.clamp(BUFFER_DURATION_MIN_SECS, BUFFER_DURATION_MAX_SECS))
    }

    pub fn default_duration() -> Self {
        Self(BUFFER_DURATION_DEFAULT_SECS)
    }

    pub fn seconds(self) -> f32 {
        self.0
    }
}

impl Default for BufferDuration {
    fn default() -> Self {
        Self::default_duration()
    }
}

/// Capacity in frames for a given sample rate and duration.
pub fn buffer_capacity_frames(sample_rate: f32, duration: BufferDuration) -> usize {
    let sr = sample_rate.max(1.0);
    (sr * duration.seconds()).round().max(1.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_duration_bounds() {
        assert_eq!(BufferDuration::new(1.0).seconds(), BUFFER_DURATION_MIN_SECS);
        assert_eq!(
            BufferDuration::new(10_000.0).seconds(),
            BUFFER_DURATION_MAX_SECS
        );
        assert_eq!(
            BufferDuration::new(45.0).seconds(),
            BUFFER_DURATION_DEFAULT_SECS
        );
    }

    #[test]
    fn capacity_matches_duration_at_sample_rate() {
        let sr = 48_000.0;
        let frames_30 = buffer_capacity_frames(sr, BufferDuration::new(30.0));
        let frames_600 = buffer_capacity_frames(sr, BufferDuration::new(600.0));
        assert_eq!(frames_30, 30 * 48_000);
        assert_eq!(frames_600, 600 * 48_000);
    }
}
