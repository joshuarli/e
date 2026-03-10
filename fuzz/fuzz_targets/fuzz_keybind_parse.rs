#![no_main]

use libfuzzer_sys::fuzz_target;

use e::keybind::{parse_action, parse_key};

fuzz_target!(|data: &[u8]| {
    // Must never panic on arbitrary input.
    if let Ok(s) = std::str::from_utf8(data) {
        // Fuzz the INI-style config parser line by line
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            if let Some((key_str, action_str)) = line.split_once('=') {
                let key_str = key_str.trim().to_lowercase();
                let action_str = action_str.trim().to_lowercase();
                let _ = parse_key(&key_str);
                let _ = parse_action(&action_str);
            }
        }
    }
});
