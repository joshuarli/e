//! Python syntax rules.

use super::{StringDelim, SyntaxRules, string_delim};

static STRINGS: &[StringDelim] = &[
    string_delim!("\"\"\"", "\"\"\"", true),
    string_delim!("'''", "'''", true),
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

pub static RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: STRINGS,
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
        "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
        "yield",
    ],
    types: &[
        "False", "None", "True", "bool", "bytes", "dict", "float", "int", "list", "self", "set",
        "str", "tuple",
    ],
    constants: &[],
    macros: &[],
    operators: &["!=", "==", "<=", ">="],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};
