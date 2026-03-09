#![no_main]

use libfuzzer_sys::fuzz_target;

use e::highlight::{self, HlState};

fuzz_target!(|data: &[u8]| {
    // Run the highlighter with every language's rules on arbitrary input.
    // Must never panic or loop infinitely.
    let languages = [
        "Rust",
        "C",
        "Python",
        "JavaScript",
        "Go",
        "Ruby",
        "Java",
        "Markdown",
        "JSON",
        "YAML",
        "TOML",
    ];

    for lang in &languages {
        if let Some(rules) = highlight::rules_for_language(lang) {
            let mut state = HlState::default();
            // Split input into lines and highlight each one.
            for line in data.split(|&b| b == b'\n') {
                let (_, next_state) = highlight::highlight_line(line, state, rules);
                state = next_state;
            }
        }
    }
});
