use std::env;
use std::process::Command;
use std::thread;
use std::time::Duration;

use nih_plug::prelude::*;
use pirate_sampler::PirateSampler;

fn main() {
    // 1. Mark standalone execution mode
    env::set_var("PIRATE_STANDALONE", "1");

    // 2. Set PipeWire properties so the JACK client never blocks graph cycles or locks quantum
    if env::var_os("PIPEWIRE_PROPS").is_none() {
        env::set_var(
            "PIPEWIRE_PROPS",
            r#"{ "node.always-process": false, "node.transport.sync": false, "node.passive": false, "node.lock-quantum": false }"#,
        );
    }

    // 3. Set default PulseAudio monitor source if Pulse/ALSA is used
    if env::var_os("PULSE_SOURCE").is_none() {
        env::set_var("PULSE_SOURCE", "@DEFAULT_MONITOR@");
    }

    // 4. Automatically link system audio monitor to inputs on Linux/PipeWire
    thread::spawn(|| {
        auto_connect_system_monitor();
    });

    nih_export_standalone::<PirateSampler>();
}

fn auto_connect_system_monitor() {
    // Poll for the standalone JACK client to register (up to 5 seconds)
    for _ in 0..25 {
        thread::sleep(Duration::from_millis(200));

        let inputs = match Command::new("pw-link").arg("-i").output() {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(_) => return,
        };

        if inputs.contains("pirate_sampler:input_1") {
            // Disconnect all previous/stray links to prevent graph cycles
            disconnect_all_pirate_links();

            let default_sink = Command::new("pactl")
                .arg("get-default-sink")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            let outputs = match Command::new("pw-link").arg("-o").output() {
                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                Err(_) => return,
            };

            let mut mon_l = None;
            let mut mon_r = None;

            for line in outputs.lines() {
                let trimmed = line.trim();
                let is_default_monitor = !default_sink.is_empty() && trimmed.starts_with(&default_sink) && trimmed.contains("monitor");
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

            if let (Some(l_port), Some(r_port)) = (mon_l, mon_r) {
                let _ = Command::new("pw-link")
                    .args(["-P", &l_port, "pirate_sampler:input_1"])
                    .output();
                let _ = Command::new("pw-link")
                    .args(["-P", &r_port, "pirate_sampler:input_2"])
                    .output();
                eprintln!("[Pirate Sampler] Automatically connected system desktop audio (passive): {} / {}", l_port, r_port);
            }
            break;
        }
    }
}

fn disconnect_all_pirate_links() {
    if let Ok(links_output) = Command::new("pw-link").arg("-l").output() {
        let text = String::from_utf8_lossy(&links_output.stdout);
        let mut cur_src = "";
        for line in text.lines() {
            let trimmed = line.trim();
            if !line.starts_with(' ') && !line.starts_with('\t') {
                cur_src = trimmed;
            } else if trimmed.starts_with("|->") {
                let target = trimmed.trim_start_matches("|->").trim();
                if target.starts_with("pirate_sampler:") || cur_src.starts_with("pirate_sampler:") {
                    let _ = Command::new("pw-link").args(["-d", cur_src, target]).output();
                }
            }
        }
    }
}
