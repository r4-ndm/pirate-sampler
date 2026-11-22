# Pirate Sampler: Agent Guide & Architecture (`AGENTS.md`)

This document follows the [`mattpocock/skills`](https://github.com/mattpocock/skills) principles (**Codebase Design**, **Deep Modules**, and **Writing for Agents**). Read this first before modifying the codebase.

> **Target: standalone first.** The standalone binary (`target/release/pirate-sampler`) is the supported product; behaviour is defined against it. `impl Vst3Plugin` in `lib.rs` is kept deliberately so the plugin can be loaded in a host later, and it must stay compiling, but it is dormant: inside a host there is no desktop routing, no frameless chrome, and no host state/automation. Do not add host-specific work without a concrete need.

## Setup Skill

Linux/PipeWire audio setup and troubleshooting lives in
`.agents/skills/pirate-sampler-audio-setup/SKILL.md`. It starts with a read-only diagnostic and
requires user confirmation before changing system defaults or links. Keep that contract when
editing it.

## Glossary & Domain Vocabulary

Use these exact terms across code, tests, and commits:

- **RingBuffer** (`crates/pirate-sampler-dsp/src/ring.rs`): Pre-allocated, real-time safe circular stereo sample buffer (`duration * sample_rate * 2 * sizeof(f32)`). Overwrites oldest frames when full. Zero allocations on the audio thread.
- **Selection** (`crates/pirate-sampler-dsp/src/export.rs`): Normalized `(0.0..=1.0)` UI interval resolved to integer frame offsets `[start_frame..end_frame)` within the filled portion of the `RingBuffer`.
- **Grab-less XDND** (`crates/pirate-sampler/src/xdnd.rs`): In-process X11 Drag-and-Drop v5 state machine (`x11rb`) that attaches to an already-held mouse button (`Button1`), negotiates `TARGETS` and `text/uri-list` with the target DAW window under the cursor, and fires `XdndDrop` upon mouse button release.
- **Passive Link** (`crates/pirate-sampler/src/routing.rs`): A PipeWire connection (`pw-link -P`) that never forces graph quantum locks or feedback loops: a desktop monitor sink or microphone source into `pirate_sampler:input_1/2`, and `pirate_sampler:output_1/2` out to the default sink's playback ports so recorded playback is audible.

## Deep Module Map

Each module hides complex internal mechanics behind a small, high-leverage interface at a clean seam:

| Module | Path | Interface Seam | Hidden Implementation |
|---|---|---|---|
| **DSP Engine** | `crates/pirate-sampler-dsp` | `RingBuffer::write_frame`, `waveform_peaks`, `write_selection_wav` | Circular wrap-around indexing, decimated peak buckets, 32-bit float IEEE WAV encoding. |
| **Plugin Core** | `crates/pirate-sampler/src/lib.rs` | `Plugin for PirateSampler` (`nih-plug`), `is_standalone()` | RT-safe duration resizing, `is_standalone()`-gated feedback muting, dormant VST3 pass-through. |
| **X11 XDND** | `crates/pirate-sampler/src/xdnd.rs` | `xdnd::start_file_drag(PathBuf)` | Full X11 XDND v5 protocol, recursive `XdndAware` window lookup, `TARGETS` negotiation, button-release drop detection. |
| **Clip IO** | `crates/pirate-sampler/src/clips.rs` | `drag_out_selection`, `copy_selection_to_clipboard`, `export_selection`, `open_clips_folder` | Frame resolution, timestamped cache rendering (`~/.cache/pirate-sampler/clips`), `latest.wav` syncing, X11/Wayland clipboard piping. |
| **Audio Routing** | `crates/pirate-sampler/src/routing.rs` | `switch_input_source`, `auto_connect_standalone` | `pw-link` & `pactl` topology discovery, stale link cleanup, mono/stereo port matching, output monitoring into the default sink. |
| **Window Chrome** | `crates/pirate-sampler/src/chrome.rs` | `attach`, `minimize`, `begin_move`, `pump_move`, `begin_resize`, `pump_resize` | Frameless X11 `_MOTIF_WM_HINTS` decoration stripping and self-driven root-coordinate window move/resize. Standalone-only: `attach` returns early unless `is_standalone()`, which leaves every other entry point inert when the plugin is loaded in a host. |
| **Waveform UI** | `crates/pirate-sampler/src/waveform.rs` | `draw_waveform_card` | Envelope rendering, playhead, edge-handle hit-testing (`ResizeHorizontal`), click-hold clip-out from inside the brace (`Grab`), dragging a brace edge resizes a running loop live, a drag outside a playing loop scrubs the captured audio under the fixed brace, right-click toggles selection looping (right-drag still pans). A press inside an existing selection never draws a new one: fresh selections start outside the brace, or after `Esc` clears it. |
| **Editor Shell** | `crates/pirate-sampler/src/editor.rs` | `editor::create`, `EditorState` | Side-panel layout, two-tier vertical overflow folding (`More` menu), keyboard shortcuts (`1..0`, `H`, `Esc`, `Ctrl+C`), dB peak meter. |
| **Theme** | `crates/pirate-sampler/src/theme.rs` | `theme::apply`, color & font constants | High-contrast monochrome palette and typography tokens. |

## Invariants & Guardrails

1. **Real-Time Audio Thread Purity**: Inside `PirateSampler::process` (`lib.rs`) and `RingBuffer::write_frame` (`ring.rs`), never allocate memory, never block on OS I/O, and never spawn threads. Capacity is pre-allocated up to `BUFFER_DURATION_MAX_SECS` (600 s).
2. **Playback Holds The Buffer Still**: `RingBuffer::write_frame` no-ops while playback is active, so auditioning never overwrites the region being played and a monitored output cannot feed back into the capture. Right-click toggles playback: `RingBuffer::loop_range` loops the selection, or `play_range` plays the whole capture once when nothing is selected; right-drag pans. While a loop runs, a left drag outside the brace calls `RingBuffer::shift_content` to rotate the captured audio under the fixed loop without moving the brace or stopping playback; pausing folds that rotation into the buffer so the move survives after the loop stops. The GUI bakes that rotation off the shared ring lock (a small parking ring records through the bake) so `process()` is never blocked by the O(filled) rotation. Dragging a brace edge calls `RingBuffer::resize_loop` so the running loop follows the new brace immediately, pulling the playhead into the window when the resize leaves it outside. Playback gain is smoothed per sample and applies only to played frames.
3. **In-Process Native Drag-Out**: Click-holding and dragging from inside an active waveform selection (the brace interior) calls `clips::drag_out_selection`, which writes the WAV clip and invokes `xdnd::start_file_drag`. The brace edges stay reserved for resizing, and nothing inside a selection ever becomes a nested selection, so the same press that used to start a loop keeps working as a Bitwig drop. Keep XDND self-contained in `xdnd.rs` via `x11rb` so the plugin requires zero external helper binaries.
4. **Single Source of Truth for Routing**: Both standalone startup (`main.rs`) and the UI toggle (`editor.rs`) delegate to `routing.rs`.

## Verification Commands

Run formatting check, strict Clippy lints (`-D warnings`), the full test suite, and build the standalone binary:

```bash
cargo fmt --all -- --check && cargo clippy -p pirate-sampler-dsp -p pirate-sampler --all-targets -- -D warnings && cargo test --workspace --exclude xtask && cargo build --release -p pirate-sampler
```

Restart `target/release/pirate-sampler` to pick up GUI/DSP changes.

The VST3 bundle is optional. Build and install it only when touching the `Vst3Plugin` path, to confirm the dormant target still compiles:

```bash
cargo xtask bundle pirate-sampler --release
cp -r target/bundled/pirate-sampler.vst3 ~/.vst3/
```
