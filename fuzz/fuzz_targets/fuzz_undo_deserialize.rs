#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Must never panic on arbitrary input.
    let _ = e::file_io::fuzz::fuzz_deserialize_undo(data);
    e::file_io::fuzz::fuzz_collect_undo_entries(data);
});
