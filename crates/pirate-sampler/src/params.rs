use nih_plug::prelude::*;
use pirate_sampler_dsp::{
    BUFFER_DURATION_DEFAULT_SECS, BUFFER_DURATION_MAX_SECS, BUFFER_DURATION_MIN_SECS,
};
use std::sync::Arc;

pub const PLAYBACK_GAIN_MIN_DB: f32 = -60.0;
pub const PLAYBACK_GAIN_MAX_DB: f32 = 24.0;

#[derive(Params)]
pub struct PirateSamplerParams {
    /// When true, ring buffer does not accept new samples (pass-through continues).
    #[id = "pause"]
    pub pause: BoolParam,

    /// Ring buffer duration in seconds (30 … 600).
    #[id = "buf_secs"]
    pub buffer_seconds: FloatParam,

    /// Gain applied only to recorded audio during playback, in decibels.
    #[id = "playback_gain_db"]
    pub playback_gain_db: FloatParam,

    /// Standard bypass.
    #[id = "bypass"]
    pub bypass: BoolParam,
}

impl Default for PirateSamplerParams {
    fn default() -> Self {
        Self {
            pause: BoolParam::new("Pause", false),
            buffer_seconds: FloatParam::new(
                "Buffer Length",
                BUFFER_DURATION_DEFAULT_SECS,
                FloatRange::Linear {
                    min: BUFFER_DURATION_MIN_SECS,
                    max: BUFFER_DURATION_MAX_SECS,
                },
            )
            .with_unit(" s")
            .with_step_size(1.0)
            .with_value_to_string(formatters::v2s_f32_rounded(0))
            .with_string_to_value(Arc::new(|s| {
                s.trim().trim_end_matches('s').trim().parse::<f32>().ok()
            })),
            playback_gain_db: FloatParam::new(
                "Playback Gain",
                0.0,
                FloatRange::Linear {
                    min: PLAYBACK_GAIN_MIN_DB,
                    max: PLAYBACK_GAIN_MAX_DB,
                },
            )
            .with_unit(" dB")
            .with_step_size(0.5)
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            bypass: BoolParam::new("Bypass", false),
        }
    }
}

impl PirateSamplerParams {
    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}
