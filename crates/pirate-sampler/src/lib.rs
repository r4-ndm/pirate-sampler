pub mod chrome;
pub mod clips;
pub mod editor;
pub mod params;
pub mod routing;
pub mod theme;
pub mod waveform;
pub mod xdnd;

use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use parking_lot::Mutex;
use pirate_sampler_dsp::{BufferDuration, RingBuffer, LATENCY_SAMPLES};

use crate::params::PirateSamplerParams;

/// Shared GUI state.
pub type SharedEditor = Arc<Mutex<editor::EditorState>>;

/// True when running as the standalone binary, which is the supported target.
///
/// `main.rs` sets `PIRATE_STANDALONE=1` before the plugin is created. The VST3 export below is kept
/// deliberately, but it must not claim the standalone-only behaviour: desktop routing, frameless
/// window chrome, and the pass-through feedback mute.
///
/// Cached because the GUI asks on every frame and the answer cannot change while `main.rs` is
/// still setting it up.
pub fn is_standalone() -> bool {
    static STANDALONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *STANDALONE.get_or_init(|| std::env::var_os("PIRATE_STANDALONE").is_some())
}

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
        let is_standalone = is_standalone();
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

    fn deactivate(&mut self) {
        self.ring.lock().set_paused(true);
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
                // Advance the parameter smoother once per frame, then apply its current value.
                let gain = util::db_to_gain(self.params.playback_gain_db.smoothed.next());
                let Some(r) = iter.next() else {
                    let sample = *l;
                    *l =
                        render_frame(&mut ring, sample, sample, bypass, gain, self.is_standalone).0;
                    continue;
                };
                let (left, right) =
                    render_frame(&mut ring, *l, *r, bypass, gain, self.is_standalone);
                *l = left;
                *r = right;
            }
        }

        ProcessStatus::Normal
    }
}

/// Record or play one stereo frame, returning the output while preserving pass-through.
///
/// Playback freezes recording, and the standalone mutes unplayed output so a monitored capture
/// cannot feed back into itself. Mono host layouts pass the same sample in both channels.
fn render_frame(
    ring: &mut RingBuffer,
    left: f32,
    right: f32,
    bypass: bool,
    gain: f32,
    standalone: bool,
) -> (f32, f32) {
    let playback = if bypass {
        None
    } else {
        ring.next_playback_frame()
    };
    // Recording and playback never happen in the same frame.
    if !bypass && !ring.is_playing() {
        ring.write_frame(left, right);
    }
    match playback {
        Some((played_l, played_r)) => (played_l * gain, played_r * gain),
        None if standalone => (0.0, 0.0),
        // Transparent pass-through: outputs already hold the inputs.
        None => (left, right),
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

/// Dormant VST3 export, kept on purpose so the plugin can be loaded in a host later.
///
/// The standalone binary is the supported product (see `README.md`). In a host there is no desktop
/// routing and [`chrome`] stays inert, and host state/automation is not wired up, so don't build
/// host-specific features without a concrete need.
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
        assert!((30.0..=600.0).contains(&secs));
    }

    #[test]
    fn mono_and_stereo_frames_record_the_same_capture() {
        let mut mono = RingBuffer::with_capacity(48_000.0, 4);
        let mut stereo = RingBuffer::with_capacity(48_000.0, 4);

        let mono_out = render_frame(&mut mono, 0.5, 0.5, false, 1.0, true).0;
        let (stereo_l, stereo_r) = render_frame(&mut stereo, 0.5, 0.5, false, 1.0, true);

        assert_eq!(mono_out, 0.0, "standalone mutes unplayed output");
        assert_eq!(stereo_l, 0.0);
        assert_eq!(stereo_r, 0.0);
        assert_eq!(mono.read_logical(0), Some((0.5, 0.5)));
        assert_eq!(mono.read_logical(0), stereo.read_logical(0));
    }

    #[test]
    fn rendered_playback_applies_gain_and_vst3_passes_through() {
        let mut ring = RingBuffer::with_capacity(48_000.0, 4);
        for i in 0..4 {
            ring.write_frame(i as f32, (i * 10) as f32);
        }
        assert!(ring.play_range(0.0, 1.0));

        let (left, right) = render_frame(&mut ring, 9.0, 9.0, false, 0.5, false);
        assert_eq!((left, right), (0.0, 0.0));
        assert!(ring.is_playing());

        let (left, right) = render_frame(
            &mut RingBuffer::with_capacity(48_000.0, 2),
            3.0,
            4.0,
            true,
            0.5,
            false,
        );
        assert_eq!((left, right), (3.0, 4.0), "bypass stays transparent");
    }

    #[test]
    fn side_panel_and_min_waveform_fit_window_width() {
        const {
            assert!(
                editor::SIDE_W + editor::MIN_WAVEFORM_W + editor::HORIZONTAL_MARGINS
                    <= editor::WINDOW_W,
                "side panel + min waveform + margins exceeds window width"
            );
        }
        assert_eq!(
            editor::WAVEFORM_H,
            editor::WINDOW_H - editor::VERTICAL_MARGINS,
            "waveform height must equal window height minus vertical margins"
        );
    }
}
