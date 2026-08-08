//! XSH language: rule table plus tag-union type scanning.
//!
//! The vocabulary arrays (`XSH_KEYWORDS`, `XSH_TYPES`, `XSH_MACROS`) live
//! between the `BEGIN/END GENERATED XSH VOCABULARY` markers below and are
//! rewritten in place by `examples/gen_xsh.rs` (via `make gen-xsh`).  The
//! string delimiters, rule table, and the `type`/tag-union scanner are
//! hand-maintained and sit outside the markers.

use super::{StringDelim, SyntaxRules, string_delim};

// BEGIN GENERATED XSH VOCABULARY
// Generated from /Users/josh/d/laputa-systems/xsh
// XSH syntax-highlighting vocabulary, generated from the XSH registry source.
// DO NOT EDIT BY HAND.  Regenerate after the XSH language or standard library
// changes keywords, types, modules, or stream stages: `make gen-xsh`.
#[rustfmt::skip]
pub static XSH_KEYWORDS: &[&str] = &[
    "and",
    "break",
    "continue",
    "defer",
    "else",
    "export",
    "false",
    "for",
    "guard",
    "if",
    "in",
    "let",
    "loop",
    "match",
    "not",
    "null",
    "or",
    "proc",
    "pure",
    "retry",
    "return",
    "run",
    "spawn",
    "stream",
    "true",
    "type",
    "unless",
    "use",
    "var",
    "wait",
    "when",
    "while",
    "with",
    "yield",
];

#[rustfmt::skip]
pub static XSH_TYPES: &[&str] = &[
    "Any",
    "ArchiveEntry",
    "Bool",
    "Bytes",
    "Command",
    "DiffResult",
    "Digest",
    "DnsHost",
    "DnsLookup",
    "Duration",
    "ElfDynamicTag",
    "ElfInfo",
    "EnvEntry",
    "EnvPathEntry",
    "EnvPathList",
    "Err",
    "Error",
    "Float",
    "FsCopyTreeResult",
    "FsEntry",
    "FsFilesystemStats",
    "FsLock",
    "FsMount",
    "FsRemoveManifestResult",
    "FsRoot",
    "Group",
    "Int",
    "LinuxBlkid",
    "LinuxBlockDevice",
    "LinuxDiskUsage",
    "LinuxFileAttrs",
    "LinuxFsck",
    "LinuxInterface",
    "LinuxInterfaceAddress",
    "LinuxLoopDevice",
    "LinuxMemInfo",
    "LinuxModinfo",
    "LinuxModule",
    "LinuxModuleParam",
    "LinuxOpenFile",
    "LinuxPartition",
    "LinuxPartitionTable",
    "LinuxRfkill",
    "LinuxRoute",
    "LinuxUevent",
    "List",
    "Map",
    "MeasuredCommand",
    "MimeInfo",
    "MimeParse",
    "Module",
    "NetHeader",
    "NetPool",
    "NetResponse",
    "Null",
    "Ok",
    "PatchResult",
    "Path",
    "Proc",
    "ProcessEntry",
    "ProcessError",
    "ProcessHandle",
    "ProcessPort",
    "ProcessThread",
    "Pure",
    "Record",
    "Regex",
    "Result",
    "Signal",
    "Spawn",
    "Status",
    "Str",
    "Stream",
    "SystemMemory",
    "SystemOsRelease",
    "TestCall",
    "TestContext",
    "UInt",
    "Uname",
    "Unit",
    "UnixChildEvent",
    "UnixGroupId",
    "UnixId",
    "UnixKillAllResult",
    "UnixLoggedProcessGroup",
    "UnixPid1Event",
    "UnixPid1Shutdown",
    "UnixSpawnedChild",
    "UnixTtyAttrs",
    "User",
];

#[rustfmt::skip]
pub static XSH_MACROS: &[&str] = &[
    "abort",
    "all",
    "any",
    "applet",
    "archive",
    "batch",
    "bytes",
    "capture",
    "cli",
    "collect",
    "count",
    "cpu",
    "diff",
    "dns",
    "drop",
    "each",
    "elf",
    "enumerate",
    "env",
    "error",
    "first",
    "fold",
    "fs",
    "group",
    "hash",
    "ini",
    "io",
    "json",
    "last",
    "linux",
    "map",
    "max",
    "mime",
    "min",
    "module",
    "net",
    "patch",
    "path",
    "print",
    "process",
    "range",
    "record",
    "reduce",
    "regex",
    "repeat",
    "set",
    "shlex",
    "shuffle",
    "sort",
    "status",
    "sum",
    "system",
    "take",
    "tee",
    "test",
    "text",
    "time",
    "tui",
    "unix",
    "user",
    "utils",
    "where",
    "zip",
];
// END GENERATED XSH VOCABULARY

// XSH: ordered longest-open-first so the generic string scanner picks the
// right delimiter.  Triple-quoted forms are multiline.
static STRINGS: &[StringDelim] = &[
    string_delim!("fp\"\"\"", "\"\"\"", true),
    string_delim!("fr\"\"\"", "\"\"\"", true),
    string_delim!("rf\"\"\"", "\"\"\"", true),
    string_delim!("f\"\"\"", "\"\"\"", true),
    string_delim!("g\"\"\"", "\"\"\"", true),
    string_delim!("p\"\"\"", "\"\"\"", true),
    string_delim!("b\"\"\"", "\"\"\"", true),
    string_delim!("r\"\"\"", "\"\"\"", true),
    string_delim!("\"\"\"", "\"\"\"", true),
    string_delim!("fr\"", "\"", false),
    string_delim!("rf\"", "\"", false),
    string_delim!("fp\"", "\"", false),
    string_delim!("b\"", "\"", false),
    string_delim!("p\"", "\"", false),
    string_delim!("f\"", "\"", false),
    string_delim!("g\"", "\"", false),
    string_delim!("r\"", "\"", false),
    string_delim!("\"", "\"", false),
];

pub static RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: STRINGS,
    keywords: XSH_KEYWORDS,
    types: XSH_TYPES,
    constants: &[],
    macros: XSH_MACROS,
    operators: &["!=", "->", "==", "=>", "<=", ">=", ">>", "|>", "??"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};

/// Scan a single line for `type` declaration names and tag-union variants.
///
/// When `in_continuation` is true the line is treated as a continuation of a
/// previous tag-union declaration: a leading `|` is expected before variant
/// names.  When the line does not start with `|`, the scanner falls through
/// and checks for a *new* `type` declaration instead.
///
/// Returns `(names, continues)` where *names* contains the type name (for a
/// new declaration) and/or variant names, and *continues* indicates whether
/// the declaration spans to the next line.
pub fn scan_type_line(line: &[u8], in_continuation: bool) -> (Vec<Vec<u8>>, bool) {
    let len = line.len();
    let mut i = 0;

    // Skip leading whitespace
    while i < len && (line[i] == b' ' || line[i] == b'\t') {
        i += 1;
    }

    // --- continuation: expect leading '|' -----------------------------------
    if in_continuation && i < len && line[i] == b'|' {
        i += 1; // skip '|'
        return (extract_variants(line, &mut i), true);
    }

    // --- new type declaration -----------------------------------------------
    if i + 4 > len || &line[i..i + 4] != b"type" {
        return (Vec::new(), false);
    }
    i += 4;
    if i >= len || !line[i].is_ascii_whitespace() {
        return (Vec::new(), false);
    }
    i += 1;

    while i < len && (line[i] == b' ' || line[i] == b'\t') {
        i += 1;
    }

    // Extract type name
    if i >= len || !(line[i].is_ascii_alphabetic() || line[i] == b'_') {
        return (Vec::new(), false);
    }
    let name_start = i;
    i += 1;
    while i < len && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
        i += 1;
    }
    let type_name = line[name_start..i].to_vec();

    // Find '='
    let Some(offset) = line[i..].iter().position(|&b| b == b'=') else {
        return (vec![type_name], false);
    };
    i += offset + 1;

    while i < len && (line[i] == b' ' || line[i] == b'\t') {
        i += 1;
    }
    if i >= len {
        // Nothing after '=' — multi-line, continues
        return (vec![type_name], true);
    }

    // Record schemas and module contracts: no variants
    if line[i] == b'{' || line[i..].starts_with(b"module") {
        return (vec![type_name], false);
    }

    // No '|' → simple alias (or empty continuation trigger)
    if !line[i..].contains(&b'|') {
        let rest_is_blank = line[i..].iter().all(|&b| b == b' ' || b == b'\t');
        return (vec![type_name], rest_is_blank);
    }

    // Tag union with '|' — extract variants
    let mut names = vec![type_name];
    names.extend(extract_variants(line, &mut i));
    (names, line_continues(line, len))
}

/// Extract `|`-separated variant identifiers starting at `*i`.
fn extract_variants(line: &[u8], i: &mut usize) -> Vec<Vec<u8>> {
    let len = line.len();
    let mut names = Vec::new();
    loop {
        while *i < len && (line[*i] == b' ' || line[*i] == b'\t' || line[*i] == b'|') {
            *i += 1;
        }
        if *i >= len || !(line[*i].is_ascii_alphabetic() || line[*i] == b'_') {
            break;
        }
        let vstart = *i;
        *i += 1;
        while *i < len && (line[*i].is_ascii_alphanumeric() || line[*i] == b'_') {
            *i += 1;
        }
        names.push(line[vstart..*i].to_vec());
        // Skip optional (payload) parens
        if *i < len && line[*i] == b'(' {
            let mut depth = 1u32;
            *i += 1;
            while *i < len && depth > 0 {
                match line[*i] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                *i += 1;
            }
        }
    }
    names
}

/// True when the last non-whitespace byte on the line is `|`.
fn line_continues(line: &[u8], len: usize) -> bool {
    let mut j = len;
    while j > 0 && (line[j - 1] == b' ' || line[j - 1] == b'\t') {
        j -= 1;
    }
    j > 0 && line[j - 1] == b'|'
}

/// Full-file scan for `type` declarations.  Used by tests only; the renderer
/// calls `scan_type_line` per-line directly.
#[cfg(test)]
pub fn collect_user_types(buf: &crate::buffer::GapBuffer) -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    let mut seen = fxhash::FxHashSet::default();
    let mut line_buf = Vec::new();
    let mut in_continuation = false;
    for line_idx in 0..buf.line_count() {
        buf.line_text_into(line_idx, &mut line_buf);
        let (names, continues) = scan_type_line(&line_buf, in_continuation);
        for name in names {
            if seen.insert(name.clone()) {
                all.push(name);
            }
        }
        in_continuation = continues;
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_user_types_simple_alias() {
        let buf = crate::buffer::GapBuffer::from_bytes(b"type Foo = Bar\n".to_vec());
        let types = collect_user_types(&buf);
        assert_eq!(types.len(), 1);
        assert_eq!(types[0], b"Foo");
    }

    #[test]
    fn test_collect_user_types_tag_union() {
        let buf = crate::buffer::GapBuffer::from_bytes(
            b"type Level = Info | Warn | Error(Str)\n".to_vec(),
        );
        let types = collect_user_types(&buf);
        assert_eq!(types.len(), 4); // Level, Info, Warn, Error
        assert_eq!(types[0], b"Level");
        assert_eq!(types[1], b"Info");
        assert_eq!(types[2], b"Warn");
        assert_eq!(types[3], b"Error");
    }

    #[test]
    fn test_collect_user_types_record_schema() {
        let buf = crate::buffer::GapBuffer::from_bytes(
            b"type Config = { name: Str, version: Int }\n".to_vec(),
        );
        let types = collect_user_types(&buf);
        assert_eq!(types.len(), 1); // only Config, no variants
        assert_eq!(types[0], b"Config");
    }

    #[test]
    fn test_collect_user_types_module_contract() {
        let buf = crate::buffer::GapBuffer::from_bytes(
            b"type Plugin = module { export let name: Str }\n".to_vec(),
        );
        let types = collect_user_types(&buf);
        assert_eq!(types.len(), 1); // only Plugin
        assert_eq!(types[0], b"Plugin");
    }

    #[test]
    fn test_collect_user_types_multiple() {
        let buf = crate::buffer::GapBuffer::from_bytes(
            b"type Foo = A | B\ntype Bar = { x: Int }\n".to_vec(),
        );
        let types = collect_user_types(&buf);
        assert_eq!(types.len(), 4); // Foo, A, B, Bar
        assert!(types.contains(&b"Foo".to_vec()));
        assert!(types.contains(&b"A".to_vec()));
        assert!(types.contains(&b"B".to_vec()));
        assert!(types.contains(&b"Bar".to_vec()));
    }

    // -- Multi-line tag unions -----------------------------------------------

    #[test]
    fn test_collect_multiline_bare_equals() {
        // type Foo =\n  | A\n  | B(Str)\n  | C
        let buf = crate::buffer::GapBuffer::from_bytes(
            b"type Foo =\n  | A\n  | B(Str)\n  | C\n".to_vec(),
        );
        let types = collect_user_types(&buf);
        assert!(types.contains(&b"Foo".to_vec()));
        assert!(types.contains(&b"A".to_vec()));
        assert!(types.contains(&b"B".to_vec()));
        assert!(types.contains(&b"C".to_vec()));
    }

    #[test]
    fn test_collect_multiline_trailing_pipe() {
        // type Foo = A |\n  | B |\n  | C   (trailing pipe triggers continuation,
        // subsequent lines use leading |)
        let buf =
            crate::buffer::GapBuffer::from_bytes(b"type Foo = A |\n  | B |\n  | C\n".to_vec());
        let types = collect_user_types(&buf);
        assert!(types.contains(&b"Foo".to_vec()));
        assert!(types.contains(&b"A".to_vec()));
        assert!(types.contains(&b"B".to_vec()));
        assert!(types.contains(&b"C".to_vec()));
    }

    #[test]
    fn test_scan_type_line_unit() {
        // Not a type declaration
        let (names, cont) = scan_type_line(b"let x = 1", false);
        assert!(names.is_empty());
        assert!(!cont);

        // Simple alias
        let (names, cont) = scan_type_line(b"type Foo = Bar", false);
        assert_eq!(names, vec![b"Foo".to_vec()]);
        assert!(!cont);

        // Bare = continuation trigger
        let (names, cont) = scan_type_line(b"type Foo =", false);
        assert_eq!(names, vec![b"Foo".to_vec()]);
        assert!(cont);

        // Continuation line with variants (always continues when leading |)
        let (names, cont) = scan_type_line(b"  | A | B", true);
        assert_eq!(names, vec![b"A".to_vec(), b"B".to_vec()]);
        assert!(cont);

        // Continuation line ending with pipe
        let (names, cont) = scan_type_line(b"  | A |", true);
        assert_eq!(names, vec![b"A".to_vec()]);
        assert!(cont);

        // Continuation line without leading '|' — falls through, no type keyword
        let (names, cont) = scan_type_line(b"  something", true);
        assert!(names.is_empty());
        assert!(!cont);

        // Record schema
        let (names, cont) = scan_type_line(b"type Config = { x: Int }", false);
        assert_eq!(names, vec![b"Config".to_vec()]);
        assert!(!cont);
    }

    /// Regression: a freshly loaded buffer has `take_dirty_line() == usize::MAX`.
    /// The renderer must still discover user types on first render despite this.
    #[test]
    fn test_user_types_found_on_fresh_buffer() {
        let mut buf = crate::buffer::GapBuffer::from_bytes(
            b"type Stats = {blanks: Int, code: Int}\ntype FileReport = {stats: Stats, name: Str}\n"
                .to_vec(),
        );
        // Fresh buffer reports no dirty line — the exact scenario that caused
        // the initial-load miss before the fix.
        assert_eq!(buf.take_dirty_line(), usize::MAX);

        // collect_user_types scans the full file and must still find them.
        let types = collect_user_types(&buf);
        assert!(types.contains(&b"Stats".to_vec()));
        assert!(types.contains(&b"FileReport".to_vec()));
    }
}
