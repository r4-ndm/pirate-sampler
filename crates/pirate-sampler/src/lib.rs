mod chrome;
mod editor;
mod params;
pub mod theme;

use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use parking_lot::Mutex;
use pirate_sampler_dsp::{BufferDuration, RingBuffer, LATENCY_SAMPLES};

use crate::params::PirateSamplerParams;

/// Shared GUI state.
pub type SharedEditor = Arc<Mutex<editor::EditorState>>;

pub struct PirateSampler {
    params: Arc<PirateSamplerParams>,
    ring: Arc<Mutex<RingBuffer>>,
    editor_state: Arc<EguiState>,
    gui: SharedEditor,
    /// Last applied buffer duration (seconds), to detect changes on the audio thread.
    last_duration_secs: f32,
    is_standalone: bool,
}

impl Default for PirateSampler {
    fn default() -> Self {
        let params = PirateSamplerParams::new_arc();
        let ring = Arc::new(Mutex::new(RingBuffer::with_max_duration(
            48_000.0,
            BufferDuration::default(),
        )));
        let gui = Arc::new(Mutex::new(editor::EditorState::new(Arc::clone(&ring))));
        let is_standalone = std::env::var_os("PIRATE_STANDALONE").is_some();
        Self {
            params,
            ring,
            editor_state: EguiState::from_size(840, 400),
            gui,
            last_duration_secs: BUFFER_DURATION_DEFAULT,
            is_standalone,
        }
    }
}

const BUFFER_DURATION_DEFAULT: f32 = pirate_sampler_dsp::BUFFER_DURATION_DEFAULT_SECS;

impl Plugin for PirateSampler {
    const NAME: &'static str = "Pirate Sampler";
    const VENDOR: &'static str = "Pirate Sampler";
    const URL: &'static str = "https://github.com/local/pirate-sampler";
    const EMAIL: &'static str = "noreply@example.com";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            Arc::clone(&self.params),
            Arc::clone(&self.editor_state),
            Arc::clone(&self.gui),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate;
        let duration = BufferDuration::new(self.params.buffer_seconds.value());
        {
            let mut ring = self.ring.lock();
            ring.reinit(sr, duration);
        }
        self.last_duration_secs = duration.seconds();
        true
    }

    fn reset(&mut self) {
        self.ring.lock().clear();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        context.set_latency_samples(LATENCY_SAMPLES);

        let bypass = self.params.bypass.value();
        let paused = self.params.pause.value();
        let duration_secs = self.params.buffer_seconds.value();

        // Duration changes: RT-safe capacity adjust (preallocated to max).
        if (duration_secs - self.last_duration_secs).abs() > 0.5 {
            let mut ring = self.ring.lock();
            editor::apply_duration(&mut ring, duration_secs);
            self.last_duration_secs = duration_secs;
        }

        {
            let mut ring = self.ring.lock();
            ring.set_paused(paused || bypass);

            for mut channel_samples in buffer.iter_samples() {
                let mut iter = channel_samples.iter_mut();
                let Some(l) = iter.next() else {
                    break;
                };
                let Some(r) = iter.next() else {
                    // Mono-like: duplicate
                    let sample = *l;
                    if !bypass {
                        ring.write_frame(sample, sample);
                    }
                    continue;
                };

                let left = *l;
                let right = *r;
                // Transparent pass-through: outputs already hold inputs in-place.
                if !bypass {
                    ring.write_frame(left, right);
                }
                if self.is_standalone {
                    *l = 0.0;
                    *r = 0.0;
                }
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for PirateSampler {
    const CLAP_ID: &'static str = "com.piratesampler.pirate-sampler";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Rolling ring-buffer audio capture with WAV export");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for PirateSampler {
    const VST3_CLASS_ID: [u8; 16] = *b"PirateSamplerV3!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_vst3!(PirateSampler);
// CLAP optional later — trait implemented for forward compatibility; export disabled for v1 formats.
// nih_export_clap!(PirateSampler);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_zero_latency_constant() {
        assert_eq!(LATENCY_SAMPLES, 0);
    }

    #[test]
    fn default_params_in_spec_range() {
        let p = PirateSamplerParams::default();
        let secs = p.buffer_seconds.default_plain_value();
        assert!(secs >= 30.0 && secs <= 600.0);
    }

    #[test]
    fn side_panel_and_min_waveform_fit_window_width() {
        assert!(
            editor::SIDE_W + editor::MIN_WAVEFORM_W + editor::HORIZONTAL_MARGINS
                <= editor::WINDOW_W,
            "side panel + min waveform + margins exceeds window width"
        );
        assert_eq!(
            editor::WAVEFORM_H,
            editor::WINDOW_H - editor::VERTICAL_MARGINS,
            "waveform height must equal window height minus vertical margins"
        );
    }
}
