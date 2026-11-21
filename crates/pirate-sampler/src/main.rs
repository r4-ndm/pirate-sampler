use std::env;
use std::thread;

use nih_plug::prelude::*;
use pirate_sampler::{routing, PirateSampler};

fn main() {
    // 1. Mark standalone execution mode so audio output zeroes pass-through to avoid feedback loops.
    env::set_var("PIRATE_STANDALONE", "1");

    // 2. Set PipeWire properties so the JACK client never blocks graph cycles or locks quantum.
    if env::var_os("PIPEWIRE_PROPS").is_none() {
        env::set_var(
            "PIPEWIRE_PROPS",
            r#"{ "node.always-process": false, "node.transport.sync": false, "node.passive": false, "node.lock-quantum": false }"#,
        );
    }

    // 3. Set default PulseAudio monitor source if Pulse/ALSA fallback is used.
    if env::var_os("PULSE_SOURCE").is_none() {
        env::set_var("PULSE_SOURCE", "@DEFAULT_MONITOR@");
    }

    // 4. Automatically link system audio monitor to inputs on Linux/PipeWire.
    thread::spawn(|| {
        routing::auto_connect_system_monitor();
    });

    nih_export_standalone::<PirateSampler>();
}
