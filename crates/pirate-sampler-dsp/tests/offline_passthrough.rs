//! Offline acceptance: pass-through equals input (SPEC A4).

use pirate_sampler_dsp::{BufferDuration, RingBuffer, Selection, write_selection_wav};

#[test]
fn offline_process_passthrough_and_capture() {
    let sr = 48_000.0;
    let mut ring = RingBuffer::with_capacity(sr, 256);

    // Synthetic stereo block.
    let mut input = Vec::new();
    for i in 0..128 {
        let l = (i as f32 / 128.0) * 0.5;
        let r = -l;
        input.push(l);
        input.push(r);
    }

    let mut output = Vec::with_capacity(input.len());
    let mut i = 0;
    while i + 1 < input.len() {
        let l = input[i];
        let r = input[i + 1];
        // Pass-through
        output.push(l);
        output.push(r);
        // Capture
        ring.write_frame(l, r);
        i += 2;
    }

    assert_eq!(output, input);
    assert_eq!(ring.filled_frames(), 128);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.wav");
    write_selection_wav(&ring, Selection::new(0, 128).unwrap(), &path).unwrap();

    let mut reader = hound::WavReader::open(&path).unwrap();
    let exported: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
    assert_eq!(exported, input);

    // Duration helper still valid for full-size buffers.
    let full = RingBuffer::new(sr, BufferDuration::new(30.0));
    assert_eq!(full.capacity_frames(), 30 * 48_000);
}
