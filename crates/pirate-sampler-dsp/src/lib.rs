//! Pirate Sampler DSP — no audio-plugin framework dependencies.
//!
//! Real-time contract for [`RingBuffer::write_frame`]:
//! - no heap allocation
//! - no locks
//! - no syscalls

mod export;
mod params;
mod ring;

pub use export::{write_selection_wav, WavExportError};
pub use params::{
    buffer_capacity_frames, BufferDuration, BUFFER_DURATION_DEFAULT_SECS,
    BUFFER_DURATION_MAX_SECS, BUFFER_DURATION_MIN_SECS, NUM_CHANNELS,
};
pub use ring::{RingBuffer, Selection};

/// Reported plugin latency in samples (v1: none).
pub const LATENCY_SAMPLES: u32 = 0;
