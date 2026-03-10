#![no_main]

use libfuzzer_sys::fuzz_target;

use e::command::{CommandRegistry, parse_args};

fuzz_target!(|data: &[u8]| {
    // Must never panic on arbitrary input.
    if let Ok(s) = std::str::from_utf8(data) {
        // Fuzz the command dispatcher
        let reg = CommandRegistry::new();
        let _ = reg.execute(s);

        // Fuzz the argument parser directly
        let _ = parse_args(s);
    }
});
