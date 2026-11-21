# Pirate Sampler: Agent Guide & Architecture (`AGENTS.md`)

This document follows the [`mattpocock/skills`](https://github.com/mattpocock/skills) principles (**Codebase Design**, **Deep Modules**, and **Writing for Agents**). Read this first before modifying the codebase.

## Glossary & Domain Vocabulary

Use these exact terms across code, tests, and commits:

- **RingBuffer** (`crates/pirate-sampler-dsp/src/ring.rs`): Pre-allocated, real-time safe circular stereo sample buffer (`duration * sample_rate * 2 * sizeof(f32)`). Overwrites oldest frames when full. Zero allocations on the audio thread.
- **Selection** (`crates/pirate-sampler-dsp/src/export.rs`): Normalized `(0.0..=1.0)` UI interval resolved to integer frame offsets `[start_frame..end_frame)` within the filled portion of the `RingBuffer`.
- **Grab-less XDND** (`crates/pirate-sampler/src/xdnd.rs`): In-process X11 Drag-and-Drop v5 state machine (`x11rb`) that attaches to an already-held mouse button (`Button1`), negotiates `TARGETS` and `text/uri-list` with the target DAW window under the cursor, and fires `XdndDrop` upon mouse button release.
- **Passive Link** (`crates/pirate-sampler/src/routing.rs`): A PipeWire connection (`pw-link -P`) from a desktop monitor sink or microphone source to `pirate_sampler:input_1/2` that never forces graph quantum locks or feedback loops.

## Deep Module Map

Each module hides complex internal mechanics behind a small, high-leverage interface at a clean seam:

| Module | Path | Interface Seam | Hidden Implementation |
|---|---|---|---|
| **DSP Engine** | `crates/pirate-sampler-dsp` | `RingBuffer::write_frame`, `waveform_peaks`, `write_selection_wav` | Circular wrap-around indexing, decimated peak buckets, 32-bit float IEEE WAV encoding. |
| **Plugin Core** | `crates/pirate-sampler/src/lib.rs` | `Plugin for PirateSampler` (`nih-plug`) | RT-safe duration resizing, standalone feedback muting, VST3 pass-through. |
| **X11 XDND** | `crates/pirate-sampler/src/xdnd.rs` | `xdnd::start_file_drag(PathBuf)` | Full X11 XDND v5 protocol, recursive `XdndAware` window lookup, `TARGETS` negotiation, button-release drop detection. |
| **Clip IO** | `crates/pirate-sampler/src/clips.rs` | `drag_out_selection`, `copy_selection_to_clipboard`, `export_selection`, `open_clips_folder` | Frame resolution, timestamped cache rendering (`~/.cache/pirate-sampler/clips`), `latest.wav` syncing, X11/Wayland clipboard piping. |
| **Audio Routing** | `crates/pirate-sampler/src/routing.rs` | `switch_input_source`, `auto_connect_system_monitor` | `pw-link` & `pactl` topology discovery, stale link cleanup, mono/stereo port matching. |
| **Window Chrome** | `crates/pirate-sampler/src/chrome.rs` | `attach`, `minimize`, `begin_move`, `pump_move`, `begin_resize`, `pump_resize` | Frameless X11 `_MOTIF_WM_HINTS` decoration stripping and self-driven root-coordinate window move/resize. |
| **Waveform UI** | `crates/pirate-sampler/src/waveform.rs` | `draw_waveform_card` | Envelope rendering, edge-handle hit-testing (`ResizeHorizontal`), interior grab-to-drag trigger (`Grab`), `mm:ss.mmm` pill badge. |
| **Editor Shell** | `crates/pirate-sampler/src/editor.rs` | `editor::create`, `EditorState` | Side-panel layout, two-tier vertical overflow folding (`More` menu), keyboard shortcuts (`1..0`, `H`, `Esc`, `Ctrl+C`), dB peak meter. |
| **Theme** | `crates/pirate-sampler/src/theme.rs` | `theme::apply`, color & font constants | High-contrast monochrome palette and typography tokens. |

## Invariants & Guardrails

1. **Real-Time Audio Thread Purity**: Inside `PirateSampler::process` (`lib.rs`) and `RingBuffer::write_frame` (`ring.rs`), never allocate memory, never block on OS I/O, and never spawn threads. Capacity is pre-allocated up to `BUFFER_DURATION_MAX_SECS` (600 s).
2. **In-Process Native Drag-Out**: Clicking and dragging from inside an active waveform selection calls `clips::drag_out_selection`, which writes the WAV clip and invokes `xdnd::start_file_drag`. Keep XDND self-contained in `xdnd.rs` via `x11rb` so the plugin requires zero external helper binaries.
3. **Single Source of Truth for Routing**: Both standalone startup (`main.rs`) and the UI toggle (`editor.rs`) delegate to `routing.rs`.

## Verification Commands

Run formatting check, strict Clippy lints (`-D warnings`), the full test suite, and build both the standalone binary and VST3 bundle:

```bash
cargo fmt --all -- --check && cargo clippy -p pirate-sampler-dsp -p pirate-sampler --all-targets -- -D warnings && cargo test --workspace --exclude xtask && cargo build --release -p pirate-sampler && cargo xtask bundle pirate-sampler --release
```

Deploy the updated VST3 bundle to the user's local plugin directory:

```bash
cp -r target/bundled/pirate-sampler.vst3 ~/.vst3/
```
