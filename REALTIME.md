# Real-time safety practices

## Audio-thread rules (v1)

- **No allocation** in `process` after `initialize` / sample-rate change.
  - Ring storage is preallocated for **max duration (10 min)** at the current sample rate.
  - Duration changes only adjust active capacity + clear (`set_duration_rt_safe`).
- **No blocking file I/O** on the audio thread (export runs from the UI / dialog).
- **NaN/Inf** inputs are sanitized to `0.0` on write.
- **Latency** reported as `0` samples.

## Current trade-off

v1 uses `parking_lot::Mutex` around the ring so the UI can snapshot/export.
Locks are kept short; the audio thread should rarely contend. A later revision can
move to a lock-free ring + `triple_buffer` peaks publish.

## Verification

```bash
# Debug build enables nih-plug `assert_process_allocs` (workspace feature).
cargo test -p pirate-sampler-dsp
cargo test -p pirate-sampler

# Manual / optional
cargo install flamegraph
cargo flamegraph -p pirate-sampler --bin pirate-sampler
```

Audit checklist:

1. `RingBuffer::write_frame` — no `Vec` growth, no locks inside DSP crate.
2. `process` — no `resize`/`reinit` except via preallocated capacity path.
3. Export — only from editor / save dialog.
