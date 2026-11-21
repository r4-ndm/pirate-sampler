//! PipeWire / JACK input routing module for standalone mode.
//!
//! Deep module that encapsulates all OS-level PipeWire (`pw-link`) and PulseAudio (`pactl`)
//! topology queries, graph cycle prevention, and dynamic input switching between
//! System Audio (desktop monitor) and Microphone.

use std::process::Command;

/// Supported audio input capture sources in standalone mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioInputSource {
    SystemAudio,
    Microphone,
}

const PORT_IN_L: &str = "pirate_sampler:input_1";
const PORT_IN_R: &str = "pirate_sampler:input_2";

/// Dynamically switches standalone PipeWire/JACK audio inputs to `source`.
/// Updates `active_links` with the newly created port connections and returns a human-readable status message.
pub fn switch_input_source(
    active_links: &mut Vec<(String, String)>,
    source: AudioInputSource,
) -> String {
    let inputs = match run_stdout("pw-link", &["-i"]) {
        Some(out) => out,
        None => return "Standalone PipeWire/JACK required".to_string(),
    };

    if !inputs.contains(PORT_IN_L) {
        return "Host track routing active".to_string();
    }

    for (src, dst) in active_links.drain(..) {
        let _ = Command::new("pw-link").args(["-d", &src, &dst]).output();
    }
    disconnect_all_pirate_links();

    let Some(outputs) = run_stdout("pw-link", &["-o"]) else {
        return "Failed to list audio sources".to_string();
    };

    match source {
        AudioInputSource::SystemAudio => connect_system_monitor(&outputs, active_links),
        AudioInputSource::Microphone => connect_microphone(&outputs, active_links),
    }
}

/// Polls for the standalone JACK client to register (up to 5 seconds) and connects
/// the system desktop audio monitor passively. Used at standalone startup.
pub fn auto_connect_system_monitor() {
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let Some(inputs) = run_stdout("pw-link", &["-i"]) else {
            return;
        };
        if inputs.contains(PORT_IN_L) {
            let mut links = Vec::new();
            let status = switch_input_source(&mut links, AudioInputSource::SystemAudio);
            eprintln!("[Pirate Sampler] {status}");
            break;
        }
    }
}

/// Removes all active PipeWire links attached to `pirate_sampler:` ports to prevent graph cycles.
pub fn disconnect_all_pirate_links() {
    let Some(text) = run_stdout("pw-link", &["-l"]) else {
        return;
    };
    let mut cur_src = "";
    for line in text.lines() {
        let trimmed = line.trim();
        if !line.starts_with(' ') && !line.starts_with('\t') {
            cur_src = trimmed;
        } else if let Some(target) = trimmed.strip_prefix("|->") {
            let target = target.trim();
            if target.starts_with("pirate_sampler:") || cur_src.starts_with("pirate_sampler:") {
                let _ = Command::new("pw-link")
                    .args(["-d", cur_src, target])
                    .output();
            }
        }
    }
}

fn connect_system_monitor(outputs: &str, active_links: &mut Vec<(String, String)>) -> String {
    let default_sink = run_stdout("pactl", &["get-default-sink"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let (mon_l, mon_r) = find_monitor_ports(outputs, &default_sink);
    if let (Some(l), Some(r)) = (mon_l, mon_r) {
        link_passive(&l, PORT_IN_L, active_links);
        link_passive(&r, PORT_IN_R, active_links);
        "Capturing System Audio".to_string()
    } else {
        "Monitor port not found".to_string()
    }
}

fn connect_microphone(outputs: &str, active_links: &mut Vec<(String, String)>) -> String {
    let default_mic = run_stdout("pactl", &["get-default-source"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let mic_ports = find_microphone_ports(outputs, &default_mic);
    if mic_ports.is_empty() {
        "No microphone detected".to_string()
    } else if mic_ports.len() == 1 || mic_ports[0].contains("mono") {
        let port = &mic_ports[0];
        link_passive(port, PORT_IN_L, active_links);
        link_passive(port, PORT_IN_R, active_links);
        "Capturing Microphone (Mono)".to_string()
    } else {
        link_passive(&mic_ports[0], PORT_IN_L, active_links);
        link_passive(&mic_ports[1], PORT_IN_R, active_links);
        "Capturing Microphone (Stereo)".to_string()
    }
}

fn link_passive(src: &str, dst: &str, active_links: &mut Vec<(String, String)>) {
    let _ = Command::new("pw-link").args(["-P", src, dst]).output();
    active_links.push((src.to_string(), dst.to_string()));
}

fn run_stdout(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

/// Pure port-resolution helper: finds the best left and right system monitor ports.
pub fn find_monitor_ports(outputs: &str, default_sink: &str) -> (Option<String>, Option<String>) {
    let mut mon_l = None;
    let mut mon_r = None;
    for line in outputs.lines() {
        let trimmed = line.trim();
        let is_default_monitor = !default_sink.is_empty()
            && trimmed.starts_with(default_sink)
            && trimmed.contains("monitor");
        if (is_default_monitor && trimmed.contains("FL"))
            || trimmed.contains("analog-stereo:monitor_FL")
            || (trimmed.ends_with(":monitor_FL") && mon_l.is_none())
        {
            mon_l = Some(trimmed.to_string());
        }
        if (is_default_monitor && trimmed.contains("FR"))
            || trimmed.contains("analog-stereo:monitor_FR")
            || (trimmed.ends_with(":monitor_FR") && mon_r.is_none())
        {
            mon_r = Some(trimmed.to_string());
        }
    }
    (mon_l, mon_r)
}

/// Pure port-resolution helper: finds ordered microphone capture ports, prioritizing `default_mic`.
pub fn find_microphone_ports(outputs: &str, default_mic: &str) -> Vec<String> {
    let mut mic_ports = Vec::new();
    for line in outputs.lines() {
        let trimmed = line.trim();
        if (trimmed.contains("capture") || trimmed.contains("input"))
            && !trimmed.contains("monitor")
            && !trimmed.contains("Midi")
        {
            if !default_mic.is_empty() && trimmed.starts_with(default_mic) {
                mic_ports.insert(0, trimmed.to_string());
            } else {
                mic_ports.push(trimmed.to_string());
            }
        }
    }
    mic_ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_default_monitor_ports() {
        let sample = "alsa_output.pci-0000_07_00.4.analog-stereo:monitor_FL\nalsa_output.pci-0000_07_00.4.analog-stereo:monitor_FR\n";
        let (l, r) = find_monitor_ports(sample, "alsa_output.pci-0000_07_00.4.analog-stereo");
        assert_eq!(
            l.as_deref(),
            Some("alsa_output.pci-0000_07_00.4.analog-stereo:monitor_FL")
        );
        assert_eq!(
            r.as_deref(),
            Some("alsa_output.pci-0000_07_00.4.analog-stereo:monitor_FR")
        );
    }

    #[test]
    fn prioritizes_default_microphone_port() {
        let sample = "other_input:capture_1\nusb_mic:capture_MONO\n";
        let ports = find_microphone_ports(sample, "usb_mic");
        assert_eq!(
            ports.first().map(String::as_str),
            Some("usb_mic:capture_MONO")
        );
    }
}
