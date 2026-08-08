//! Guard `src/xsh_lang.rs` against drift from the XSH registry.
//!
//! Re-runs the same `syn`-based vocabulary extraction as
//! `examples/gen_xsh.rs` and compares it against the committed file.  The
//! comparison ignores the `// Generated from <path>` header line so it works
//! regardless of where the registry checkout lives.  When the registry is not
//! present (no `XSH_REPO`, no default checkout), the test skips instead of
//! failing so the editor still tests clean on machines without the registry.

// The generator is only executed through `default_repo()`/`regenerate()` here;
// the CLI entry points it also defines are dead code in this crate.
#[allow(dead_code)]
#[path = "../examples/gen_xsh.rs"]
mod gen_xsh;

use std::path::Path;

/// Drop the `// Generated from <path>` line, which records the local checkout
/// location rather than a property of the vocabulary.
fn strip_generated_from(content: &str) -> String {
    content
        .lines()
        .filter(|line| !line.starts_with("// Generated from"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn xsh_lang_matches_registry() {
    let repo = gen_xsh::default_repo();
    if !repo.join("Cargo.toml").exists() {
        eprintln!(
            "skipping: {} not found; not submitting XSH freshness checks",
            repo.display()
        );
        return;
    }
    let vocab = gen_xsh::regenerate(&repo).expect("parsing the XSH registry source");
    let committed = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("xsh_lang.rs"),
    )
    .expect("reading src/xsh_lang.rs");
    assert_eq!(
        strip_generated_from(&vocab.content),
        strip_generated_from(&committed),
        "src/xsh_lang.rs is out of date with the registry at {}; run `make gen-xsh`",
        repo.display()
    );
}
