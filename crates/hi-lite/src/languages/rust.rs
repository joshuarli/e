//! Rust syntax rules.

use super::{LexerKind, RuleSet, StringDelim, string_delim};

static STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

pub(crate) static RULES: RuleSet = RuleSet {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: STRINGS,
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
        "pub", "ref", "return", "self", "static", "struct", "super", "trait", "type", "unsafe",
        "use", "where", "while", "yield",
    ],
    types: &[
        "Box", "Err", "None", "Ok", "Option", "Result", "Self", "Some", "String", "Vec", "bool",
        "char", "f32", "f64", "false", "i128", "i16", "i32", "i64", "i8", "isize", "str", "true",
        "u128", "u16", "u32", "u64", "u8", "usize",
    ],
    constants: &[],
    macros: &[
        "abort", "archive", "args", "bytes", "cli", "cpu", "diff", "dns", "env", "eprint", "fs",
        "group", "hash", "io", "json", "linux", "list", "map", "module", "net", "patch", "path",
        "print", "process", "range", "record", "regex", "system", "test", "text", "time", "tui",
        "unix", "user",
    ],
    operators: &["&&", "||", "!=", "==", "<=", ">=", "=>", "->"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: true,
    lexer_kind: LexerKind::Rust,
};
