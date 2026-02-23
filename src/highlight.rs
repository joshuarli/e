//! Syntax highlighting engine.
//!
//! Byte-by-byte highlighter inspired by kilo/kibi. Produces one `HlType` per
//! byte, then maps to per-char highlights for the renderer.

use crate::buffer;

// -- Types ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum HlType {
    #[default]
    Normal,
    Keyword,
    Type,
    String,
    Comment,
    Number,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum HlState {
    #[default]
    Normal,
    BlockComment,
    MultiLineString(u8),
}

pub struct StringDelim {
    pub open: &'static str,
    pub close: &'static str,
    pub multiline: bool,
}

pub struct SyntaxRules {
    pub line_comment: &'static str,
    pub block_comment: (&'static str, &'static str),
    pub string_delims: &'static [StringDelim],
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    pub highlight_numbers: bool,
}

// -- ANSI color codes -------------------------------------------------------

impl HlType {
    /// Return the ANSI color code for this highlight type, or empty for Normal.
    pub fn ansi_code(self) -> &'static str {
        match self {
            HlType::Normal => "",
            HlType::Comment => "\x1b[90m", // grey
            HlType::Keyword => "\x1b[33m", // yellow
            HlType::Type => "\x1b[36m",    // cyan
            HlType::String => "\x1b[32m",  // green
            HlType::Number => "\x1b[31m",  // red
        }
    }
}

// -- Algorithm --------------------------------------------------------------

fn is_separator(c: u8) -> bool {
    c.is_ascii_whitespace()
        || c == b'\0'
        || matches!(
            c,
            b',' | b'.'
                | b'('
                | b')'
                | b'+'
                | b'-'
                | b'/'
                | b'*'
                | b'='
                | b'~'
                | b'%'
                | b'<'
                | b'>'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b';'
                | b':'
                | b'&'
                | b'|'
                | b'!'
                | b'^'
                | b'@'
                | b'#'
                | b'?'
        )
}

fn starts_with_at(haystack: &[u8], needle: &[u8], pos: usize) -> bool {
    if needle.is_empty() || pos + needle.len() > haystack.len() {
        return false;
    }
    &haystack[pos..pos + needle.len()] == needle
}

/// Highlight a single line. Returns (per-byte HlType vec, next-line state).
pub fn highlight_line(line: &[u8], state: HlState, rules: &SyntaxRules) -> (Vec<HlType>, HlState) {
    let len = line.len();
    let mut hl = vec![HlType::Normal; len];
    let mut i = 0;
    let mut prev_sep = true;
    let mut current_state = state;

    let block_open = rules.block_comment.0.as_bytes();
    let block_close = rules.block_comment.1.as_bytes();
    let line_com = rules.line_comment.as_bytes();

    // Handle entering in a multiline state
    match current_state {
        HlState::BlockComment => {
            while i < len {
                if starts_with_at(line, block_close, i) {
                    let end = i + block_close.len();
                    for b in &mut hl[i..end] {
                        *b = HlType::Comment;
                    }
                    i = end;
                    current_state = HlState::Normal;
                    prev_sep = true;
                    break;
                }
                hl[i] = HlType::Comment;
                i += 1;
            }
            if current_state == HlState::BlockComment {
                return (hl, HlState::BlockComment);
            }
        }
        HlState::MultiLineString(idx) => {
            let close = rules.string_delims[idx as usize].close.as_bytes();
            while i < len {
                // Check for backslash escape
                if line[i] == b'\\' && i + 1 < len {
                    hl[i] = HlType::String;
                    hl[i + 1] = HlType::String;
                    i += 2;
                    continue;
                }
                if starts_with_at(line, close, i) {
                    let end = i + close.len();
                    for b in &mut hl[i..end] {
                        *b = HlType::String;
                    }
                    i = end;
                    current_state = HlState::Normal;
                    prev_sep = true;
                    break;
                }
                hl[i] = HlType::String;
                i += 1;
            }
            if matches!(current_state, HlState::MultiLineString(_)) {
                return (hl, current_state);
            }
        }
        HlState::Normal => {}
    }

    // Main loop
    while i < len {
        // Line comment
        if !line_com.is_empty() && starts_with_at(line, line_com, i) {
            for b in &mut hl[i..len] {
                *b = HlType::Comment;
            }
            return (hl, HlState::Normal);
        }

        // Block comment start
        if !block_open.is_empty() && starts_with_at(line, block_open, i) {
            let start = i;
            i += block_open.len();
            // Scan for close on same line
            let mut found = false;
            while i < len {
                if starts_with_at(line, block_close, i) {
                    let end = i + block_close.len();
                    for b in &mut hl[start..end] {
                        *b = HlType::Comment;
                    }
                    i = end;
                    prev_sep = true;
                    found = true;
                    break;
                }
                i += 1;
            }
            if !found {
                for b in &mut hl[start..len] {
                    *b = HlType::Comment;
                }
                return (hl, HlState::BlockComment);
            }
            continue;
        }

        // String delimiters (longest open first)
        let mut matched_string = false;
        for (di, delim) in rules.string_delims.iter().enumerate() {
            let open = delim.open.as_bytes();
            let close = delim.close.as_bytes();
            if starts_with_at(line, open, i) {
                let start = i;
                i += open.len();
                // Scan for close
                let mut found = false;
                while i < len {
                    if line[i] == b'\\' && i + 1 < len {
                        hl[i] = HlType::String;
                        hl[i + 1] = HlType::String;
                        i += 2;
                        continue;
                    }
                    if starts_with_at(line, close, i) {
                        let end = i + close.len();
                        for b in &mut hl[start..end] {
                            *b = HlType::String;
                        }
                        i = end;
                        prev_sep = true;
                        found = true;
                        break;
                    }
                    i += 1;
                }
                if !found {
                    for b in &mut hl[start..len] {
                        *b = HlType::String;
                    }
                    if delim.multiline {
                        return (hl, HlState::MultiLineString(di as u8));
                    }
                    return (hl, HlState::Normal);
                }
                matched_string = true;
                break;
            }
        }
        if matched_string {
            continue;
        }

        // Numbers (after separator)
        if rules.highlight_numbers && prev_sep && is_digit_start(line, i) {
            let start = i;
            i += 1;
            while i < len && is_number_char(line[i]) {
                i += 1;
            }
            for b in &mut hl[start..i] {
                *b = HlType::Number;
            }
            prev_sep = false;
            continue;
        }

        // Keywords and types (after separator)
        if prev_sep && (line[i].is_ascii_alphabetic() || line[i] == b'_') {
            if let Some(advance) = try_keyword(line, i, rules.keywords, HlType::Keyword, &mut hl) {
                i += advance;
                prev_sep = false;
                continue;
            }
            if let Some(advance) = try_keyword(line, i, rules.types, HlType::Type, &mut hl) {
                i += advance;
                prev_sep = false;
                continue;
            }
        }

        prev_sep = is_separator(line[i]);
        i += 1;
    }

    (hl, HlState::Normal)
}

fn is_digit_start(line: &[u8], i: usize) -> bool {
    let c = line[i];
    if c.is_ascii_digit() {
        return true;
    }
    // .5 style floats
    if c == b'.' && i + 1 < line.len() && line[i + 1].is_ascii_digit() {
        return true;
    }
    false
}

fn is_number_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'.'
}

fn try_keyword(
    line: &[u8],
    pos: usize,
    words: &[&str],
    hl_type: HlType,
    hl: &mut [HlType],
) -> Option<usize> {
    for &word in words {
        let wb = word.as_bytes();
        if starts_with_at(line, wb, pos) {
            let end = pos + wb.len();
            // Must be followed by separator or end of line
            if end >= line.len() || is_separator(line[end]) {
                for b in &mut hl[pos..end] {
                    *b = hl_type;
                }
                return Some(wb.len());
            }
        }
    }
    None
}

// -- Byte-to-char mapping ---------------------------------------------------

/// Map byte-indexed highlights to char-indexed highlights.
/// Tabs expand to 2 display entries, multi-byte UTF-8 collapses to 1 entry.
pub fn byte_hl_to_char_hl(raw: &[u8], byte_hl: &[HlType]) -> Vec<HlType> {
    let mut char_hl = Vec::with_capacity(raw.len());
    let mut bi = 0;
    while bi < raw.len() {
        let ht = byte_hl[bi];
        if raw[bi] == b'\t' {
            // Tab expands to 2 display positions
            char_hl.push(ht);
            char_hl.push(ht);
            bi += 1;
        } else {
            char_hl.push(ht);
            bi += buffer::utf8_char_len(raw[bi]);
        }
    }
    char_hl
}

// -- Language rules ---------------------------------------------------------

macro_rules! string_delim {
    ($open:expr, $close:expr, $ml:expr) => {
        StringDelim {
            open: $open,
            close: $close,
            multiline: $ml,
        }
    };
}

static RUST_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static RUST_RULES: SyntaxRules = SyntaxRules {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: RUST_STRINGS,
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
        "pub", "ref", "return", "self", "static", "struct", "super", "trait", "type", "unsafe",
        "use", "where", "while", "yield",
    ],
    types: &[
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
        "u16", "u32", "u64", "u128", "usize", "String", "Vec", "Option", "Result", "Box", "Self",
        "true", "false", "None", "Some", "Ok", "Err",
    ],
    highlight_numbers: true,
};

static PYTHON_STRINGS: &[StringDelim] = &[
    string_delim!("\"\"\"", "\"\"\"", true),
    string_delim!("'''", "'''", true),
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static PYTHON_RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: PYTHON_STRINGS,
    keywords: &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
        "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
        "yield",
    ],
    types: &[
        "True", "False", "None", "int", "float", "str", "bool", "list", "dict", "tuple", "set",
        "bytes", "self",
    ],
    highlight_numbers: true,
};

static GO_STRINGS: &[StringDelim] = &[
    string_delim!("`", "`", true),
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static GO_RULES: SyntaxRules = SyntaxRules {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: GO_STRINGS,
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
    ],
    types: &[
        "bool",
        "byte",
        "complex64",
        "complex128",
        "error",
        "float32",
        "float64",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "rune",
        "string",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "true",
        "false",
        "nil",
        "iota",
    ],
    highlight_numbers: true,
};

static TS_STRINGS: &[StringDelim] = &[
    string_delim!("`", "`", true),
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static TS_RULES: SyntaxRules = SyntaxRules {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: TS_STRINGS,
    keywords: &[
        "abstract",
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "of",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &[
        "any",
        "boolean",
        "bigint",
        "never",
        "null",
        "number",
        "object",
        "string",
        "symbol",
        "undefined",
        "unknown",
        "void",
        "true",
        "false",
        "Array",
        "Map",
        "Set",
        "Promise",
    ],
    highlight_numbers: true,
};

static JS_STRINGS: &[StringDelim] = &[
    string_delim!("`", "`", true),
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static JS_RULES: SyntaxRules = SyntaxRules {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: JS_STRINGS,
    keywords: &[
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "let",
        "new",
        "of",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &[
        "null",
        "undefined",
        "true",
        "false",
        "NaN",
        "Infinity",
        "Array",
        "Object",
        "Map",
        "Set",
        "Promise",
        "Number",
        "String",
        "Boolean",
    ],
    highlight_numbers: true,
};

static BASH_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static BASH_RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: BASH_STRINGS,
    keywords: &[
        "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "in",
        "function", "return", "local", "export", "source", "set", "unset", "readonly", "declare",
        "eval", "exec", "exit", "shift", "trap", "break", "continue",
    ],
    types: &["true", "false"],
    highlight_numbers: true,
};

static C_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static C_RULES: SyntaxRules = SyntaxRules {
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: C_STRINGS,
    keywords: &[
        "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern",
        "for", "goto", "if", "inline", "register", "restrict", "return", "sizeof", "static",
        "struct", "switch", "typedef", "union", "volatile", "while",
    ],
    types: &[
        "char", "double", "float", "int", "long", "short", "signed", "unsigned", "void", "NULL",
        "size_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t",
        "uint64_t", "bool", "true", "false",
    ],
    highlight_numbers: true,
};

static TOML_STRINGS: &[StringDelim] = &[
    string_delim!("\"\"\"", "\"\"\"", true),
    string_delim!("'''", "'''", true),
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static TOML_RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: TOML_STRINGS,
    keywords: &[],
    types: &["true", "false"],
    highlight_numbers: true,
};

static JSON_STRINGS: &[StringDelim] = &[string_delim!("\"", "\"", false)];

static JSON_RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("", ""),
    string_delims: JSON_STRINGS,
    keywords: &[],
    types: &["true", "false", "null"],
    highlight_numbers: true,
};

static YAML_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static YAML_RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: YAML_STRINGS,
    keywords: &[],
    types: &["true", "false", "null", "yes", "no"],
    highlight_numbers: true,
};

static MAKEFILE_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static MAKEFILE_RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: MAKEFILE_STRINGS,
    keywords: &[
        "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "define", "endef", "include",
        "override", "export", "unexport", "vpath",
    ],
    types: &[],
    highlight_numbers: false,
};

static HTML_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static HTML_RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("<!--", "-->"),
    string_delims: HTML_STRINGS,
    keywords: &[],
    types: &[],
    highlight_numbers: false,
};

static CSS_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static CSS_RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("/*", "*/"),
    string_delims: CSS_STRINGS,
    keywords: &[],
    types: &[],
    highlight_numbers: true,
};

static DOCKERFILE_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static DOCKERFILE_RULES: SyntaxRules = SyntaxRules {
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: DOCKERFILE_STRINGS,
    keywords: &[
        "FROM",
        "RUN",
        "CMD",
        "LABEL",
        "EXPOSE",
        "ENV",
        "ADD",
        "COPY",
        "ENTRYPOINT",
        "VOLUME",
        "USER",
        "WORKDIR",
        "ARG",
        "ONBUILD",
        "STOPSIGNAL",
        "HEALTHCHECK",
        "SHELL",
        "AS",
    ],
    types: &[],
    highlight_numbers: false,
};

/// Look up syntax rules for a language name (from `language::Language::name`).
pub fn rules_for_language(name: &str) -> Option<&'static SyntaxRules> {
    match name {
        "Rust" => Some(&RUST_RULES),
        "Python" => Some(&PYTHON_RULES),
        "Go" => Some(&GO_RULES),
        "TypeScript" => Some(&TS_RULES),
        "JavaScript" => Some(&JS_RULES),
        "Shell" => Some(&BASH_RULES),
        "C" => Some(&C_RULES),
        "TOML" => Some(&TOML_RULES),
        "JSON" => Some(&JSON_RULES),
        "YAML" => Some(&YAML_RULES),
        "Makefile" => Some(&MAKEFILE_RULES),
        "HTML" => Some(&HTML_RULES),
        "CSS" => Some(&CSS_RULES),
        "Dockerfile" => Some(&DOCKERFILE_RULES),
        _ => None,
    }
}

// -- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn hl_types(line: &[u8], rules: &SyntaxRules) -> Vec<HlType> {
        highlight_line(line, HlState::Normal, rules).0
    }

    // -- Basic highlighting -------------------------------------------------

    #[test]
    fn test_line_comment() {
        let hl = hl_types(b"let x = 1; // comment", &RUST_RULES);
        // The "// comment" part should all be Comment
        assert_eq!(hl[11], HlType::Comment);
        assert_eq!(hl[20], HlType::Comment);
    }

    #[test]
    fn test_keyword() {
        let hl = hl_types(b"fn main() {}", &RUST_RULES);
        assert_eq!(hl[0], HlType::Keyword); // 'f'
        assert_eq!(hl[1], HlType::Keyword); // 'n'
        assert_eq!(hl[2], HlType::Normal); // ' '
    }

    #[test]
    fn test_type() {
        let hl = hl_types(b"let x: usize = 0;", &RUST_RULES);
        // "usize" starts at index 7
        assert_eq!(hl[7], HlType::Type);
        assert_eq!(hl[11], HlType::Type);
    }

    #[test]
    fn test_string() {
        let hl = hl_types(b"let s = \"hello\";", &RUST_RULES);
        // "hello" starts at index 8, ends at 14
        assert_eq!(hl[8], HlType::String); // opening "
        assert_eq!(hl[13], HlType::String); // closing "
    }

    #[test]
    fn test_number() {
        let hl = hl_types(b"let x = 42;", &RUST_RULES);
        assert_eq!(hl[8], HlType::Number); // '4'
        assert_eq!(hl[9], HlType::Number); // '2'
    }

    #[test]
    fn test_normal_text() {
        let hl = hl_types(b"hello", &RUST_RULES);
        assert!(hl.iter().all(|&h| h == HlType::Normal));
    }

    // -- Block comments -----------------------------------------------------

    #[test]
    fn test_block_comment_single_line() {
        let hl = hl_types(b"x /* comment */ y", &RUST_RULES);
        assert_eq!(hl[0], HlType::Normal); // 'x'
        assert_eq!(hl[2], HlType::Comment); // '/'
        assert_eq!(hl[13], HlType::Comment); // '/'
        assert_eq!(hl[16], HlType::Normal); // 'y'
    }

    #[test]
    fn test_block_comment_multiline() {
        let (hl1, state) = highlight_line(b"/* start", HlState::Normal, &RUST_RULES);
        assert!(hl1.iter().all(|&h| h == HlType::Comment));
        assert_eq!(state, HlState::BlockComment);

        let (hl2, state2) = highlight_line(b"end */", HlState::BlockComment, &RUST_RULES);
        assert!(hl2.iter().all(|&h| h == HlType::Comment));
        assert_eq!(state2, HlState::Normal);
    }

    // -- Multiline strings --------------------------------------------------

    #[test]
    fn test_python_triple_quote() {
        let (hl1, state) = highlight_line(b"s = \"\"\"hello", HlState::Normal, &PYTHON_RULES);
        assert_eq!(hl1[4], HlType::String);
        assert!(matches!(state, HlState::MultiLineString(_)));

        let (hl2, state2) = highlight_line(b"world\"\"\"", state, &PYTHON_RULES);
        assert!(hl2.iter().all(|&h| h == HlType::String));
        assert_eq!(state2, HlState::Normal);
    }

    #[test]
    fn test_go_backtick_string() {
        let (hl1, state) = highlight_line(b"s := `hello", HlState::Normal, &GO_RULES);
        assert_eq!(hl1[5], HlType::String);
        assert!(matches!(state, HlState::MultiLineString(_)));

        let (hl2, state2) = highlight_line(b"world`", state, &GO_RULES);
        assert!(hl2.iter().all(|&h| h == HlType::String));
        assert_eq!(state2, HlState::Normal);
    }

    // -- Escape handling in strings -----------------------------------------

    #[test]
    fn test_string_escape() {
        let hl = hl_types(b"\"he\\\"llo\"", &RUST_RULES);
        // All should be String since \" is escaped
        assert!(hl.iter().all(|&h| h == HlType::String));
    }

    // -- Keyword boundary ---------------------------------------------------

    #[test]
    fn test_keyword_not_in_identifier() {
        let hl = hl_types(b"format", &RUST_RULES);
        // "for" should not match inside "format"
        assert!(hl.iter().all(|&h| h == HlType::Normal));
    }

    // -- byte_hl_to_char_hl -------------------------------------------------

    #[test]
    fn test_byte_to_char_ascii() {
        let raw = b"hello";
        let byte_hl = vec![HlType::Keyword; 5];
        let char_hl = byte_hl_to_char_hl(raw, &byte_hl);
        assert_eq!(char_hl.len(), 5);
        assert!(char_hl.iter().all(|&h| h == HlType::Keyword));
    }

    #[test]
    fn test_byte_to_char_tab() {
        let raw = b"\thello";
        let byte_hl = vec![HlType::Normal; 6];
        let char_hl = byte_hl_to_char_hl(raw, &byte_hl);
        // Tab expands to 2 entries
        assert_eq!(char_hl.len(), 7);
    }

    #[test]
    fn test_byte_to_char_utf8() {
        let raw = "héllo".as_bytes(); // é is 2 bytes
        let byte_hl = vec![HlType::Normal; raw.len()];
        let char_hl = byte_hl_to_char_hl(raw, &byte_hl);
        // 5 chars: h, é, l, l, o
        assert_eq!(char_hl.len(), 5);
    }

    // -- rules_for_language -------------------------------------------------

    #[test]
    fn test_rules_for_known_languages() {
        assert!(rules_for_language("Rust").is_some());
        assert!(rules_for_language("Python").is_some());
        assert!(rules_for_language("Go").is_some());
        assert!(rules_for_language("TypeScript").is_some());
        assert!(rules_for_language("JavaScript").is_some());
        assert!(rules_for_language("Shell").is_some());
        assert!(rules_for_language("C").is_some());
        assert!(rules_for_language("TOML").is_some());
        assert!(rules_for_language("JSON").is_some());
        assert!(rules_for_language("YAML").is_some());
        assert!(rules_for_language("Makefile").is_some());
        assert!(rules_for_language("HTML").is_some());
        assert!(rules_for_language("CSS").is_some());
        assert!(rules_for_language("Dockerfile").is_some());
    }

    #[test]
    fn test_rules_for_unknown() {
        assert!(rules_for_language("Markdown").is_none());
        assert!(rules_for_language("Unknown").is_none());
    }

    // -- Python specifics ---------------------------------------------------

    #[test]
    fn test_python_hash_comment() {
        let hl = hl_types(b"x = 1 # comment", &PYTHON_RULES);
        assert_eq!(hl[6], HlType::Comment);
    }

    // -- Empty line ---------------------------------------------------------

    #[test]
    fn test_empty_line() {
        let (hl, state) = highlight_line(b"", HlState::Normal, &RUST_RULES);
        assert!(hl.is_empty());
        assert_eq!(state, HlState::Normal);
    }

    #[test]
    fn test_empty_line_in_block_comment() {
        let (hl, state) = highlight_line(b"", HlState::BlockComment, &RUST_RULES);
        assert!(hl.is_empty());
        assert_eq!(state, HlState::BlockComment);
    }

    // -- HTML block comments ------------------------------------------------

    #[test]
    fn test_html_comment() {
        let (hl, state) = highlight_line(b"<!-- comment -->", HlState::Normal, &HTML_RULES);
        assert!(hl.iter().all(|&h| h == HlType::Comment));
        assert_eq!(state, HlState::Normal);
    }

    #[test]
    fn test_html_multiline_comment() {
        let (hl1, state1) = highlight_line(b"<!-- start", HlState::Normal, &HTML_RULES);
        assert!(hl1.iter().all(|&h| h == HlType::Comment));
        assert_eq!(state1, HlState::BlockComment);

        let (hl2, state2) = highlight_line(b"end -->", HlState::BlockComment, &HTML_RULES);
        assert!(hl2.iter().all(|&h| h == HlType::Comment));
        assert_eq!(state2, HlState::Normal);
    }

    // -- Dockerfile keywords ------------------------------------------------

    #[test]
    fn test_dockerfile_keywords() {
        let hl = hl_types(b"FROM ubuntu:latest", &DOCKERFILE_RULES);
        assert_eq!(hl[0], HlType::Keyword); // F
        assert_eq!(hl[3], HlType::Keyword); // M
    }

    // -- JSON ---------------------------------------------------------------

    #[test]
    fn test_json_no_comments() {
        let hl = hl_types(b"{\"key\": true}", &JSON_RULES);
        assert_eq!(hl[1], HlType::String); // "
        assert_eq!(hl[8], HlType::Type); // 't' of true
    }

    // -- Number edge cases --------------------------------------------------

    #[test]
    fn test_hex_number() {
        let hl = hl_types(b"let x = 0xff;", &RUST_RULES);
        assert_eq!(hl[8], HlType::Number); // '0'
        assert_eq!(hl[9], HlType::Number); // 'x'
        assert_eq!(hl[11], HlType::Number); // 'f'
    }

    #[test]
    fn test_float_number() {
        let hl = hl_types(b"let x = 3.14;", &RUST_RULES);
        assert_eq!(hl[8], HlType::Number); // '3'
        assert_eq!(hl[9], HlType::Number); // '.'
        assert_eq!(hl[10], HlType::Number); // '1'
    }
}
