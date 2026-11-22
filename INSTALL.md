# Installing Pirate Sampler

Pirate Sampler is a Linux x86_64 **standalone** application. There are no prebuilt release
packages yet, so installation means building the binary and placing it on your `PATH`.

The dormant VST3 export is not part of this install. See `README.md` if you are a developer and
need to build that bundle.

## Requirements

- Linux x86_64 with an X11 session or XWayland for the window chrome.
- A Rust **stable** toolchain (`cargo` must be available).
- A running **PipeWire** session with the JACK and Pulse compatibility layers, `pw-link`, and
  `pactl`.
- The system development packages used by the build.

The app uses nih-plug's JACK backend to publish `pirate_sampler` ports. If JACK is unavailable it
can fall back to ALSA or a silent dummy backend, and audio routing will not work.

### Debian / Ubuntu build packages

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config \
  libasound2-dev libgtk-3-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libssl-dev
```

This is the same package set used by CI, plus `build-essential` and `pkg-config`.

### Debian / Ubuntu audio packages

```bash
sudo apt install -y pipewire pipewire-jack pipewire-pulse pulseaudio-utils
```

Package names differ on other distributions. You need the equivalents of PipeWire's JACK and
Pulse compatibility packages, plus `pw-link` and `pactl`. Confirm they are visible before
continuing:

```bash
command -v pw-link pactl
```

## Build

From the repository root (the directory containing `Cargo.toml`):

```bash
cargo build --locked --release -p pirate-sampler
```

The binary is written to:

```text
target/release/pirate-sampler
```

## Install for the current user

Install into your user-local binary directory:

```bash
install -Dm755 target/release/pirate-sampler "$HOME/.local/bin/pirate-sampler"
```

Make sure `~/.local/bin` is on your `PATH`. Add this to `~/.profile` if it is not already there:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Then reload your shell profile or open a new terminal:

```bash
command -v pirate-sampler
```

For a system-wide install instead, use `/usr/local/bin` (and run the desktop entry's `Exec`
without the user-local path):

```bash
sudo install -Dm755 target/release/pirate-sampler /usr/local/bin/pirate-sampler
```

## First run

Start it from a terminal the first time so you can see backend errors:

```bash
pirate-sampler
```

The startup output should include `Using the JACK backend`. The app then connects the desktop
monitor to `pirate_sampler:input_1/2` and its outputs to the default sink automatically.

If capture or playback is silent, run the read-only diagnostic from the repository root:

```bash
bash .agents/skills/pirate-sampler-audio-setup/scripts/diagnose.sh
```

The accompanying skill covers missing monitor ports, microphone setup, wrong sinks, and stale
links:

```text
.agents/skills/pirate-sampler-audio-setup/SKILL.md
```

The app follows the system default sink and has no output-device picker. After changing the
default, restart the app or toggle **Mic -> System** to relink.

## Desktop launcher (optional)

Create a user desktop entry:

```bash
mkdir -p "$HOME/.local/share/applications"
cat > "$HOME/.local/share/applications/pirate-sampler.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Pirate Sampler
Comment=Rolling ring-buffer sampler
Exec="$HOME/.local/bin/pirate-sampler"
Icon=applications-multimedia
Terminal=false
Categories=AudioVideo;Audio;
Keywords=audio;sampler;looper;recorder;
EOF
chmod +x "$HOME/.local/share/applications/pirate-sampler.desktop"
```

The unquoted heredoc expands `$HOME` to your actual home directory in the `Exec` line. Refresh the
application database if the tool is available:

```bash
command -v update-desktop-database >/dev/null &&
  update-desktop-database "$HOME/.local/share/applications"
```

The launcher uses the desktop's generic audio icon so it works without external design assets.
The app window itself uses the M-and-Bones logo embedded in the binary. If you have the
`m-and-bones-256.png` asset, you can install it as a branded launcher icon:

```bash
mkdir -p "$HOME/.local/share/icons/hicolor/256x256/apps"
install -m644 /path/to/m-and-bones-256.png \
  "$HOME/.local/share/icons/hicolor/256x256/apps/pirate-sampler.png"
```

Then change `Icon=applications-multimedia` to `Icon=pirate-sampler` in the desktop file and refresh
the icon cache if `gtk-update-icon-cache` exists.

## Update

From the repository root:

```bash
git pull --ff-only
cargo build --locked --release -p pirate-sampler
install -Dm755 target/release/pirate-sampler "$HOME/.local/bin/pirate-sampler"
```

The desktop entry does not need to be rewritten unless you change its `Exec` or `Icon`.

## Uninstall

```bash
rm -f "$HOME/.local/bin/pirate-sampler"
rm -f "$HOME/.local/share/applications/pirate-sampler.desktop"
rm -f "$HOME/.local/share/icons/hicolor/256x256/apps/pirate-sampler.png"
```

If you used the system-wide install, remove it separately:

```bash
sudo rm -f /usr/local/bin/pirate-sampler
```

Build artifacts remain under `target/` and can be removed with `cargo clean` if desired.

## Troubleshooting

### `cargo: command not found`

Install a Rust stable toolchain (for example with `rustup`) and reload your shell.

### Build fails while looking for headers or a linker

The Debian/Ubuntu package list above is incomplete for your distribution. Install the equivalents
of the GTK, XCB/XKB, ALSA, OpenSSL, and C/C++ build packages, then retry.

### `pirate-sampler: command not found`

`~/.local/bin` is not on `PATH` in the current shell. Reload your profile or use the absolute path:

```bash
"$HOME/.local/bin/pirate-sampler"
```

### No launcher icon appears

Confirm the desktop file is in `~/.local/share/applications`, is executable, and has an `Exec`
path that exists. Run `update-desktop-database`, then log out and back in. Some desktops also
require marking the launcher as trusted.

### The app opens but there is no audio

Run the audio diagnostic above. Also check the terminal output for `Using the JACK backend`:

```bash
"$HOME/.local/bin/pirate-sampler" --backend jack
```

If the app says **Standalone PipeWire/JACK required**, `pw-link` is missing or the JACK client did
not register. If it says **Host track routing active**, you are probably running the VST3 in a
host rather than the standalone binary.

### No window on Wayland

The window chrome is X11-based. Run under XWayland, or use an X11 session.
