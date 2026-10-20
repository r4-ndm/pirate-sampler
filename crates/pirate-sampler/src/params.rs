use nih_plug::prelude::*;
use pirate_sampler_dsp::{
    BUFFER_DURATION_DEFAULT_SECS, BUFFER_DURATION_MAX_SECS, BUFFER_DURATION_MIN_SECS,
};
use std::sync::Arc;

#[derive(Params)]
pub struct PirateSamplerParams {
    /// When true, ring buffer does not accept new samples (pass-through continues).
    #[id = "pause"]
    pub pause: BoolParam,

    /// Ring buffer duration in seconds (30 … 600).
    #[id = "buf_secs"]
    pub buffer_seconds: FloatParam,

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
                s.trim()
                    .trim_end_matches('s')
                    .trim()
                    .parse::<f32>()
                    .ok()
            })),
            bypass: BoolParam::new("Bypass", false),
        }
    }
}

impl PirateSamplerParams {
    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}
