# Pirate Sampler

Linux-first rolling ring-buffer capture plugin (**VST3 + standalone**), written in Rust.

Clean-room implementation driven only by [`SPEC.md`](./SPEC.md). Not derived from any proprietary binary.

## Features (v1)

- Fixed-length stereo **ring buffer** (oldest overwritten)
- Buffer length **30 s … 10 min** (default 45 s)
- Transparent **pass-through**
- **Pause** recording
- Waveform GUI with region select + **Export WAV** (32-bit float, host sample rate)
- Reported latency: **0 samples**

## Workspace

| Crate | Role |
|-------|------|
| `pirate-sampler-dsp` | Framework-free ring buffer + WAV export + unit/offline tests |
| `pirate-sampler` | nih-plug plugin (VST3 + standalone) + egui editor |
| `xtask` | Bundle helper (`cargo xtask bundle pirate-sampler --release`) |

## Build (Linux)

```bash
cd pirate-sampler
cargo test
cargo build --release -p pirate-sampler

# VST3 bundle
cargo xtask bundle pirate-sampler --release
```

Standalone binary: `target/release/pirate-sampler`

Install VST3 (example):

```bash
cp -r target/bundled/pirate-sampler.vst3 ~/.vst3/
```

## Real-time safety

See [`REALTIME.md`](./REALTIME.md).

## CI guidance

See [`.github/workflows/ci.yml`](./.github/workflows/ci.yml).

## License

GPL-3.0-or-later (compatible with nih-plug).
