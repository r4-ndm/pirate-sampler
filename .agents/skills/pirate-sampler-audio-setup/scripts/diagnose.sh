#!/usr/bin/env bash
# Read-only diagnostics for Pirate Sampler's Linux/PipeWire audio routing.
# Prints the session, defaults, ports, links, and a checklist. Changes nothing.

set -u

inputs=""
outputs=""
links=""
default_sink=""
default_source=""

have() {
    command -v "$1" >/dev/null 2>&1
}

run() {
    if have timeout; then
        timeout 5 "$@" 2>&1
    else
        "$@" 2>&1
    fi
}

section() {
    printf '\n== %s ==\n' "$1"
}

section "Host"
printf 'Kernel: %s\n' "$(uname -srm 2>/dev/null || echo unknown)"
if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    printf 'Distro: %s\n' "${PRETTY_NAME:-unknown}"
fi
printf 'XDG_RUNTIME_DIR=%s\n' "${XDG_RUNTIME_DIR:-<unset>}"
printf 'DISPLAY=%s\n' "${DISPLAY:-<unset>}"
printf 'WAYLAND_DISPLAY=%s\n' "${WAYLAND_DISPLAY:-<unset>}"
printf 'PIPEWIRE_PROPS=%s\n' "${PIPEWIRE_PROPS:-<unset>}"
printf 'PULSE_SOURCE=%s\n' "${PULSE_SOURCE:-<unset>}"
printf 'PULSE_SERVER=%s\n' "${PULSE_SERVER:-<unset>}"

section "Tools"
for tool in pw-link pactl wpctl pw-cli; do
    if have "$tool"; then
        printf '%-8s %s\n' "$tool" "$(command -v "$tool")"
    else
        printf '%-8s MISSING\n' "$tool"
    fi
done

section "Servers"
if have pgrep; then
    pgrep -af 'pipewire|wireplumber|pulseaudio' 2>/dev/null ||
        echo 'No PipeWire/WirePlumber/PulseAudio process found.'
else
    ps -ef 2>/dev/null | grep -E '[p]ipewire|[w]ireplumber|[p]ulseaudio' ||
        echo 'No PipeWire/WirePlumber/PulseAudio process found.'
fi

section "PulseAudio / PipeWire-Pulse"
if have pactl; then
    info="$(run pactl info)"
    printf '%s\n' "$info" |
        sed -n 's/^Server Name: /Server: /p; s/^Default Sink: /Default sink: /p; s/^Default Source: /Default source: /p'
    default_sink="$(printf '%s\n' "$info" | sed -n 's/^Default Sink: //p' | head -n 1)"
    default_source="$(printf '%s\n' "$info" | sed -n 's/^Default Source: //p' | head -n 1)"
    if [ -z "$default_sink" ] && pactl get-default-sink >/dev/null 2>&1; then
        default_sink="$(pactl get-default-sink 2>/dev/null | head -n 1)"
    fi
    if [ -z "$default_source" ] && pactl get-default-source >/dev/null 2>&1; then
        default_source="$(pactl get-default-source 2>/dev/null | head -n 1)"
    fi
else
    echo 'pactl is missing; cannot query Pulse/PipeWire-Pulse defaults.'
fi

section "Sinks"
if have pactl; then
    pactl list short sinks 2>&1 || true
else
    echo 'pactl is missing.'
fi

section "Sources"
if have pactl; then
    pactl list short sources 2>&1 || true
else
    echo 'pactl is missing.'
fi

section "Pirate Sampler process"
if have pgrep; then
    pgrep -af '(^|/)pirate-sampler( |$)' 2>/dev/null ||
        echo 'Not running, or the binary has a different process name.'
else
    ps -ef 2>/dev/null | grep -E '[ /]pirate-sampler( |$)' ||
        echo 'Not running, or the binary has a different process name.'
fi

section "Pirate Sampler ports"
if have pw-link; then
    inputs="$(run pw-link -i)"
    outputs="$(run pw-link -o)"

    found=0
    if printf '%s\n' "$inputs" | grep -qE 'pirate_sampler:(input_1|input_2)'; then
        printf '%s\n' "$inputs" | grep -E 'pirate_sampler:(input_1|input_2)' | sed 's/^/in : /'
        found=1
    fi
    if printf '%s\n' "$outputs" | grep -qE 'pirate_sampler:(output_1|output_2)'; then
        printf '%s\n' "$outputs" | grep -E 'pirate_sampler:(output_1|output_2)' | sed 's/^/out: /'
        found=1
    fi
    if [ "$found" -eq 0 ]; then
        echo 'No pirate_sampler input/output ports found.'
        echo 'The app may not be running, or it fell back to ALSA/dummy instead of JACK.'
    fi
else
    echo 'pw-link is missing; cannot inspect JACK/PipeWire ports.'
fi

section "Default sink monitor ports"
if have pw-link; then
    if [ -n "$default_sink" ]; then
        matches="$(printf '%s\n' "$outputs" | grep -F "$default_sink" | grep -E 'monitor_(FL|FR)' || true)"
        if [ -n "$matches" ]; then
            printf '%s\n' "$matches"
        else
            echo "No monitor_FL/monitor_FR ports found for the default sink '$default_sink'."
        fi
    else
        matches="$(printf '%s\n' "$outputs" | grep -E ':monitor_(FL|FR)' || true)"
        if [ -n "$matches" ]; then
            printf '%s\n' "$matches"
        else
            echo 'No default sink and no monitor_FL/monitor_FR ports found.'
        fi
    fi
else
    echo 'pw-link is missing.'
fi

section "Default sink playback ports"
if have pw-link; then
    if [ -n "$default_sink" ]; then
        matches="$(printf '%s\n' "$inputs" | grep -F "$default_sink" | grep -E 'playback_(FL|FR)' || true)"
        if [ -n "$matches" ]; then
            printf '%s\n' "$matches"
        else
            echo "No playback_FL/playback_FR ports found for the default sink '$default_sink'."
        fi
    else
        echo 'No default sink; cannot select playback ports.'
    fi
else
    echo 'pw-link is missing.'
fi

section "Links involving Pirate Sampler"
if have pw-link; then
    links="$(run pw-link -l)"
    if printf '%s\n' "$links" | grep -q 'pirate_sampler'; then
        printf '%s\n' "$links" | grep -E -A2 -B1 'pirate_sampler'
    else
        echo 'No links involving pirate_sampler ports.'
    fi
else
    echo 'pw-link is missing.'
fi

section "Summary"
printf 'pw-link:                    %s\n' "$(have pw-link && echo present || echo MISSING)"
printf 'pactl:                      %s\n' "$(have pactl && echo present || echo MISSING)"
printf 'default sink:               %s\n' "${default_sink:-<none>}"
printf 'default source:             %s\n' "${default_source:-<none>}"

pirate_ports="no"
if printf '%s\n%s\n' "$inputs" "$outputs" | grep -qE 'pirate_sampler:(input|output)_[12]'; then
    pirate_ports="yes"
fi
monitor_ports="no"
if [ -n "$default_sink" ] &&
    printf '%s\n' "$outputs" | grep -F "$default_sink" | grep -qE 'monitor_(FL|FR)'; then
    monitor_ports="yes"
fi
playback_ports="no"
if [ -n "$default_sink" ] &&
    printf '%s\n' "$inputs" | grep -F "$default_sink" | grep -qE 'playback_(FL|FR)'; then
    playback_ports="yes"
fi
pirate_links="no"
if printf '%s\n' "$links" | grep -q 'pirate_sampler'; then
    pirate_links="yes"
fi

printf 'Pirate Sampler ports:       %s\n' "$pirate_ports"
printf 'default monitor FL/FR:      %s\n' "$monitor_ports"
printf 'default playback FL/FR:     %s\n' "$playback_ports"
printf 'links involving app:        %s\n' "$pirate_links"
printf '\nRead-only diagnostic. Use SKILL.md to interpret these results.\n'
