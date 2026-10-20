# Pirate Sampler — Behavior Specification (v1)

Clean-room reimplementation. This document is the sole product authority for v1.
Do not match undocumented behavior of any third-party product.

## Product

- **Name:** Pirate Sampler
- **Formats:** Standalone + VST3
- **Platform:** Linux x86_64 (Windows later)

## Core behavior

1. Continuously records incoming audio into a **fixed-length ring buffer** while running.
2. When full, **oldest samples are overwritten**.
3. Memory usage is bounded by: `duration × sample_rate × channels × sizeof(f32)`.
4. Audio is **passed through** unchanged (input → output).
5. User **selects a region** on the waveform GUI and **exports it as WAV** (drag-out preferred; Save fallback on Linux if DnD unavailable).

## Buffer duration

- Range: **30 seconds … 10 minutes** (inclusive), user-configurable.
- Default on first launch: **45 seconds**.

## Channels & audio

- Stereo I/O.
- Transparent pass-through.
- Mono host layouts (if offered later): duplicate mono to L/R on input; for now stereo-only layout is accepted.

## WAV export

- Container: WAV
- Format: **32-bit float PCM**, interleaved stereo
- Sample rate: **host / standalone sample rate** (no resampling in v1)

## GUI (v1)

- Waveform display of the ring buffer contents.
- Click-drag to create a selection.
- Export selection as WAV (drag-out and/or Save).
- **Pause** control: when paused, ring buffer is not written; pass-through continues.
- Pan / zoom: **out of scope for v1**.

## Transport

- **Ignore host transport.** Record unless the user presses Pause.

## Latency & bypass

- Reported latency: **0 samples**.
- Bypass: pass-through only; no recording side effects required beyond normal processing rules.

## Out of scope (v1)

CLAP, pan/zoom, themes, bridge / system-audio capture, BPM in filenames, preview scrub, MIDI.

## Acceptance tests (measurable)

| ID | Requirement | Test |
|----|-------------|------|
| A1 | Ring overwrite | Fill buffer with known sequence; write past capacity; extracted oldest samples match overwrite rule |
| A2 | Duration bounds | Setting 30s and 600s yields matching capacity at a given sample rate |
| A3 | Pause | While paused, buffer contents unchanged after N process blocks |
| A4 | Pass-through | Offline render: output equals input sample-for-sample |
| A5 | WAV export | Selection written as 32-bit float stereo WAV at process sample rate |
| A6 | Latency | Plugin reports 0 samples latency |
