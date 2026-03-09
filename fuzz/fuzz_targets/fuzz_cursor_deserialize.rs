#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must never panic on arbitrary input.
    e::file_io::fuzz::fuzz_collect_cursor_entries(data);
});
