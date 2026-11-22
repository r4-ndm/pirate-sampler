# Pirate Sampler — Behavior Specification (v1)

Clean-room reimplementation. This document is the sole product authority for v1.
Do not match undocumented behavior of any third-party product.

## Product

- **Name:** Pirate Sampler
- **Formats:** Standalone (the supported target) + VST3 export (kept dormant, must keep compiling — see `README.md`)
- **Platform:** Linux x86_64 (Windows later)
- **Supported target:** the standalone binary. Everything below describes it; the dormant VST3 export only needs to keep compiling (see `README.md`).

## Core behavior

1. Continuously records incoming audio into a **fixed-length ring buffer** while running, except while playback is active (see Playback & looping). Playback freezes the capture so the region being auditioned cannot be overwritten and a monitored output cannot feed back into the recording.
2. When full, **oldest samples are overwritten**.
3. Memory usage is bounded by: `duration × sample_rate × channels × sizeof(f32)`.
4. The VST3 export **passes audio through** unchanged (input → output). The standalone mutes unplayed output instead, so a monitored capture cannot feed back into itself; you hear the buffer only when playback is running.
5. User **selects a region** on the waveform GUI and **exports it as WAV** (drag-out preferred; Save fallback on Linux if DnD unavailable).

## Playback & looping

- **Right-click** the waveform toggles playback. With a selection, the selection loops; with no selection, the whole capture plays once.
- Playback **freezes recording** for as long as it runs; recording resumes when playback stops.
- A running loop follows live brace resizing: dragging a selection edge changes the played range without stopping playback, and the playhead is cued into the new range if it falls outside.
- While a loop plays, a left drag outside the brace **scrubs** the captured audio under the fixed brace. When playback stops, the rotation is folded into the capture, so the scrub is an edit that survives.
- **Playback gain** adjusts the level of played audio only, from **-60 dB to +24 dB** (default **0 dB**). Pass-through and recording are unaffected.

## Buffer duration

- Range: **30 seconds … 10 minutes** (inclusive), user-configurable.
- Default on first launch: **45 seconds**.

## Channels & audio

- Stereo I/O.
- Transparent pass-through (VST3 build; the standalone mutes unplayed output, see core behavior 4).
- Mono host layouts (if offered later): duplicate mono to L/R on input; for now stereo-only layout is accepted.

## WAV export

- Container: WAV
- Format: **32-bit float PCM**, interleaved stereo
- Sample rate: **host / standalone sample rate** (no resampling in v1)

## GUI (v1)

- Waveform display of the ring buffer contents.
- Click-drag outside an existing brace to create a selection. A press inside the brace starts a clip drag-out, not a nested selection; brace edges resize. `Esc` clears the selection.
- While a selection loops, a drag outside the brace scrubs the audio under it (see Playback & looping).
- Export selection as WAV (drag-out and/or Save).
- **Pause** control: when paused, ring buffer is not written; pass-through continues.
- **Pan / zoom**: supported — right-drag pans the view; right-click toggles playback.
- Playback gain knob (see Playback & looping).
- Window size is remembered between launches.

## Transport

- **Ignore host transport.** Record unless the user presses Pause or playback is running.

## Latency & bypass

- Reported latency: **0 samples**.
- Bypass: pass-through only; no recording side effects required beyond normal processing rules.

## Out of scope (v1)

CLAP, BPM in filenames, MIDI, user-selectable themes.

## Acceptance tests (measurable)

| ID | Requirement | Test |
|----|-------------|------|
| A1 | Ring overwrite | Fill buffer with known sequence; write past capacity; extracted oldest samples match overwrite rule |
| A2 | Duration bounds | Setting 30s and 600s yields matching capacity at a given sample rate |
| A3 | Pause | While paused, buffer contents unchanged after N process blocks |
| A4 | Pass-through (VST3 build) | Offline render: output equals input sample-for-sample |
| A5 | WAV export | Selection written as 32-bit float stereo WAV at process sample rate |
| A6 | Latency | Plugin reports 0 samples latency |
| A7 | Playback freezes recording | Start playback; process blocks; filled frames and contents stay fixed; stop playback and new frames are accepted |
| A8 | Selection looping | Start playback with a selection; output wraps from the selection end to its start |
| A9 | Live loop resize | Drag a brace edge while looping; the next output frames follow the new range without restarting |
| A10 | Playback gain | Played frames use the gain value; bypass/pass-through and recording are unaffected |
| A11 | Scrub edit | While a loop plays, drag outside the brace; the loop reads different frames; after stopping, the rotated content is retained |
| A12 | Window size memory | Resize the standalone window, close the app, and reopen it; the window opens at the last size |
