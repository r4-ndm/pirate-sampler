---
name: pirate-sampler-audio-setup
description: Diagnose and fix Pirate Sampler's Linux/PipeWire audio routing on a new machine. Use when the standalone app has no input, no playback, a missing monitor or microphone port, the wrong output sink, or reports "Standalone PipeWire/JACK required", or when the user asks to set up Pirate Sampler audio, connect desktop audio or a mic, move it to another Linux machine, or troubleshoot pw-link, pactl, PipeWire, or JACK links for this project.
---

# Pirate Sampler Audio Setup

Pirate Sampler's supported target is the Linux standalone. It captures the desktop monitor
(or the default microphone) and monitors playback through the system **default sink**. There is
no output-device picker in the app.

`crates/pirate-sampler/src/routing.rs` owns the routing:

- system monitor -> `pirate_sampler:input_1/2`
- microphone -> `pirate_sampler:input_1/2`
- `pirate_sampler:output_1/2` -> the default sink's `playback_FL/FR`

All links are passive (`pw-link -P`), so they never force graph quantum locks or feedback loops.
The standalone mutes unplayed output; you only hear the buffer while playback is running.

## Safety

- **Read-only first.** Run the diagnostic, explain what is missing, and ask before changing the
  default sink/source, installing packages, or removing links.
- **Stay in your lane.** Only unlink ports starting with `pirate_sampler:` or links you created.
  Never dismantle unrelated PipeWire/JACK links.
- **Use passive links.** The app uses `pw-link -P`. A non-passive manual link can force graph
  cycles or quantum locks, and can cause feedback.
- **Don't use `sudo` for session tools.** `pactl` and `pw-link` must run in the user's PipeWire
  session; sudo loses it. Package installation is the only likely sudo step, and only with
  confirmation.
- **Expect stale-looking state.** The app does not watch for default-sink changes. After changing
  the system default, restart the standalone or click **Mic** then **System** to relink.

## 1. Run the diagnostic

Resolve paths against this skill's directory:

```bash
bash scripts/diagnose.sh
```

It is read-only. It prints the session environment, whether `pw-link`/`pactl`/`wpctl` exist,
the Pulse/PipeWire defaults, sinks/sources, the Pirate Sampler process, its JACK ports, the
default sink's monitor and playback ports, its active links, and a short checklist.

Also ask the app to show its backend. Run the standalone from a terminal and read the `nih-plug`
lines at startup; `Using the JACK backend` is the expected line. With `--backend jack` you can
force it.

## 2. New-machine checklist

1. **PipeWire is running** for the logged-in user. `pw-link` must work; `pactl` must reach a
   Pulse-compatible server.
2. **JACK compatibility is installed.** The app publishes `pirate_sampler` ports through
   nih-plug's JACK backend. Without `pipewire-jack`/`libjack` the backend can fall back to ALSA or
   the dummy backend, and there are no ports to route.
3. **Start the standalone**, not the VST3 in a host:
   `./target/release/pirate-sampler`.
4. **Check the ports** in the diagnostic. The app registers `input_1/2` and `output_1/2` only
   when JACK initialized.
5. **Check the default sink** has `monitor_FL/FR` (capture) and `playback_FL/FR` (monitoring).
6. **Exercise the app:** select a region, right-click to loop it, then export. Switch
   **System/Mic** and confirm the status text updates.

Package names vary by distribution. On Debian/Ubuntu the relevant packages are usually
`pipewire`, `pipewire-jack`, `pipewire-pulse`, and `pulseaudio-utils`; on Fedora the JACK and
Pulse compatibility packages have different names. Verify with the package manager rather than
guessing, and install only after the user agrees.

## 3. Decision tree

### `pw-link` or `pactl` is missing / "Standalone PipeWire/JACK required"

The app cannot route. Install the missing PipeWire JACK/Pulse compatibility package (with
confirmation), make sure the user session is running PipeWire, then log out and back in or restart
the app. Do not fall back to `--backend alsa` while relying on this routing: ALSA/CPAL does not
create JACK ports, so the app cannot be linked.

### No `pirate_sampler` ports

Check, in order:

1. Is the standalone process running? `pgrep -af pirate-sampler`.
2. Did startup log `Using the JACK backend`? If it logged ALSA or dummy, fix JACK availability
   and restart.
3. Is this the standalone? The UI status **Host track routing active** means no standalone JACK
   client was found; a VST3 loaded in a DAW takes a different path.
4. Force the backend with `./target/release/pirate-sampler --backend jack` and read the error.

### Ports exist, but capture is silent

- Confirm the default sink in `pactl info` and look for its `monitor_FL/FR` ports in
  `pw-link -o`.
- If the default sink has no monitor ports (some Bluetooth/HDMI/virtual devices), either choose a
  sink that exposes them or switch to **Mic**. Changing the system default requires confirmation.
- Set the default with `pactl set-default-sink <sink>` only after confirming the exact sink name.
  Restart the app or toggle **Mic -> System**; the app does not follow the change live.
- Confirm the app's links: `pw-link -l` should show a monitor source linked to
  `pirate_sampler:input_1` and `input_2`.
- Check that the sink/source is not muted and its volume is up (`pactl list sinks`,
  `pactl list sources`).

### Ports exist, but playback is silent

- The standalone mutes unplayed output by design. You only hear audio while playback is running;
  right-click the waveform to start it.
- Check the playback gain knob; `-60 dB` is effectively silent.
- Confirm `pirate_sampler:output_1/2` links to the default sink's `playback_FL/FR` in
  `pw-link -l`. If they are missing, rerun the input switch or restart; the app's status should not
  say **Playback sink not found**.
- Check the default sink is not muted and is the device you are listening to.

### Microphone is silent or missing

- Confirm the default source: `pactl get-default-source`.
- `pactl set-default-source <source>` changes it, with confirmation. Then restart or toggle
  **System -> Mic**.
- Check the source is not muted and the app has permission to capture it.
- Mono microphones are duplicated to both inputs; stereo uses the first two capture ports.

### Wrong output device

There is no in-app picker. Set the system default sink, then restart or toggle **Mic -> System**.
Verify the `pw-link -l` targets end in the new sink's `playback_FL/FR`.

### Feedback, howling, or a graph cycle

- Do not manually link the app's outputs back into its own inputs.
- Keep links passive (`-P`). Non-passive links can force the graph to run in a tight cycle.
- Stop playback before relinking; changing links under a running loop can surprise the user.
- If a stale link is involved, remove only the specific `pirate_sampler:` link after confirmation.

### Stale or duplicate links

List them first:

```bash
pw-link -l
```

Remove only the exact bad connection, after confirming it belongs to this app:

```bash
pw-link -d <source-port> <destination-port>
```

The app already sweeps all `pirate_sampler:` links when switching input sources, so a restart or
**Mic -> System** is usually safer than manual surgery.

## 4. Commands worth knowing

Read-only:

```bash
pactl info
pactl get-default-sink
pactl get-default-source
pactl list short sinks
pactl list short sources
pw-link -i
pw-link -o
pw-link -l
```

Changing (only after confirmation, using names from the diagnostic):

```bash
pactl set-default-sink <sink-name>
pactl set-default-source <source-name>
pw-link -P <source-port> <pirate_sampler:input_1>
pw-link -P <pirate_sampler:output_1> <sink:playback_FL>
pw-link -d <source-port> <destination-port>
```

Never hardcode a device name from this document. Use the names the diagnostic reports.

## 5. Verify and report

- Capture: the desktop monitor reaches `pirate_sampler:input_1/2`; the waveform moves.
- Playback: right-click loops a selection; sound comes from the default sink; gain changes level.
- Microphone: switch to **Mic**; the status changes and the waveform moves.
- Wrong-sink recovery: change the system default, relink, and confirm the new target.
- Export: select a region and export a WAV.

Report what was missing, what you changed (exact commands), what still does not work, and any
step you skipped because it needed user confirmation. Do not claim audio works if you only
inspected ports; the user has to confirm they can hear it.
