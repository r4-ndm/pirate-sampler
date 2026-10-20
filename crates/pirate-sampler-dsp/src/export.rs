//! WAV export (SPEC: 32-bit float, interleaved stereo, host sample rate).

use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::ring::{RingBuffer, Selection};

#[derive(Debug)]
pub enum WavExportError {
    EmptySelection,
    OutOfRange,
    Io(std::io::Error),
    Hound(hound::Error),
}

impl From<std::io::Error> for WavExportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<hound::Error> for WavExportError {
    fn from(value: hound::Error) -> Self {
        Self::Hound(value)
    }
}

impl std::fmt::Display for WavExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySelection => write!(f, "empty selection"),
            Self::OutOfRange => write!(f, "selection out of range"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Hound(e) => write!(f, "wav error: {e}"),
        }
    }
}

impl std::error::Error for WavExportError {}

/// Write a logical selection from `buffer` to `path` as 32-bit float stereo WAV.
pub fn write_selection_wav(
    buffer: &RingBuffer,
    selection: Selection,
    path: impl AsRef<Path>,
) -> Result<(), WavExportError> {
    if selection.len() == 0 {
        return Err(WavExportError::EmptySelection);
    }
    let samples = buffer
        .copy_selection_interleaved(selection)
        .ok_or(WavExportError::OutOfRange)?;

    let spec = WavSpec {
        channels: 2,
        sample_rate: buffer.sample_rate().round().max(1.0) as u32,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };

    let mut writer = WavWriter::create(path.as_ref(), spec)?;
    for &s in &samples {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::RingBuffer;

    #[test]
    fn writes_32bit_float_stereo_at_buffer_sample_rate() {
        let mut rb = RingBuffer::with_capacity(48_000.0, 16);
        for i in 0..8 {
            rb.write_frame(i as f32 * 0.1, -(i as f32 * 0.1));
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sel.wav");
        let sel = Selection::new(2, 6).unwrap();
        write_selection_wav(&rb, sel, &path).unwrap();

        let mut reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 32);
        assert_eq!(spec.sample_format, SampleFormat::Float);

        let data: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(data.len(), 4 * 2);
        assert!((data[0] - 0.2).abs() < 1e-6);
        assert!((data[1] - (-0.2)).abs() < 1e-6);
    }
}
