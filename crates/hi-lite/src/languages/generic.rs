//! Shared rule tables for syntax families that use the common code scanner.
//!
//! Syntect ships many grammars whose useful line-level behavior is the same:
//! comments, quoted strings, numbers, identifiers, and a small vocabulary of
//! keywords.  Keeping those languages on these shared tables gives hi-lite
//! broad coverage without creating one bespoke lexer per grammar.

use super::{LexerKind, RuleSet, StringDelim, string_delim};

static C_LIKE_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static SCRIPT_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

/// A deliberately empty table for plain text and log syntaxes.
pub(crate) static PLAIN_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "",
    block_comment: ("", ""),
    string_delims: &[],
    keywords: &[],
    types: &[],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
};

static SQL_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
    string_delim!("`", "`", false),
];

static C_LIKE_KEYWORDS: &[&str] = &[
    "abstract", "assert", "break", "case", "catch", "class", "const",
    "continue", "default", "do", "else", "enum", "extends", "final",
    "finally", "for", "foreach", "fun", "if", "implements", "import",
    "interface", "new", "operator", "package", "private", "protected",
    "public", "return", "static", "struct", "switch", "this", "throw",
    "throws", "trait", "try", "typedef", "union", "using", "virtual",
    "void", "volatile", "while",
];

static C_LIKE_TYPES: &[&str] = &[
    "bool", "boolean", "byte", "char", "double", "float", "int", "long",
    "object", "short", "size_t", "string", "unsigned", "var",
];

static C_LIKE_OPERATORS: &[&str] = &[
    "!=", "&&", "++", "--", "->", "::", "<=", "==", ">=", "||", "=",
    "+", "-", "*", "/", "%", "<", ">", "&", "|", "^", "!", ".",
];

/// C, C++, JVM, .NET, Swift, and other brace-oriented languages.
pub(crate) static C_LIKE_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "//",
    block_comment: ("/*", "*/"),
    string_delims: C_LIKE_STRINGS,
    keywords: C_LIKE_KEYWORDS,
    types: C_LIKE_TYPES,
    constants: &["false", "null", "true"],
    macros: &[],
    operators: C_LIKE_OPERATORS,
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
};

static HASH_KEYWORDS: &[&str] = &[
    "alias", "and", "begin", "case", "class", "def", "do", "else", "elsif",
    "end", "ensure", "for", "if", "in", "lambda", "module", "next", "not",
    "or", "redo", "rescue", "retry", "return", "then", "unless", "until",
    "when", "while", "yield",
];

/// Ruby, Perl, R, and other hash-comment scripting languages.
pub(crate) static HASH_SCRIPT_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "#",
    block_comment: ("", ""),
    string_delims: SCRIPT_STRINGS,
    keywords: HASH_KEYWORDS,
    types: &["FALSE", "NA", "NULL", "TRUE", "nil"],
    constants: &[],
    macros: &[],
    operators: &["!=", "&&", "=>", "==", "<=", ">=", "||", "=", "+", "-", "*", "/", "%", "."],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
};

static DASH_KEYWORDS: &[&str] = &[
    "as", "case", "class", "data", "default", "deriving", "else", "foreign",
    "if", "import", "in", "instance", "let", "module", "newtype", "of", "then",
    "theorem", "type", "where",
];

/// Haskell, Lua, and ML-family languages with dash comments.
pub(crate) static DASH_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "--",
    block_comment: ("", ""),
    string_delims: SCRIPT_STRINGS,
    keywords: DASH_KEYWORDS,
    types: &["Bool", "Char", "Double", "Float", "Int", "String", "unit"],
    constants: &["false", "nil", "true"],
    macros: &[],
    operators: &["!=", "&&", "->", "<=", "==", ">=", "||", "=", "+", "-", "*", "/", "%", "."],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
};

/// SQL and SQL-derived templates.
pub(crate) static SQL_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "--",
    block_comment: ("/*", "*/"),
    string_delims: SQL_STRINGS,
    keywords: &[
        "alter", "and", "as", "begin", "case", "create", "delete", "drop", "else",
        "end", "from", "group", "having", "insert", "into", "join", "not", "null",
        "on", "or", "order", "select", "set", "table", "then", "union", "update",
        "values", "when", "where", "with",
    ],
    types: &["bigint", "boolean", "char", "date", "decimal", "float", "int", "integer", "text", "time", "timestamp", "varchar"],
    constants: &["false", "true"],
    macros: &[],
    operators: &["!=", "<=", ">=", "=", "+", "-", "*", "/", "%", "<", ">"],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
};

/// Lisp-family languages use semicolon comments.
pub(crate) static LISP_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: ";",
    block_comment: ("", ""),
    string_delims: SCRIPT_STRINGS,
    keywords: &["def", "defn", "do", "else", "fn", "if", "lambda", "let", "let*", "setq", "when"],
    types: &["false", "nil", "true"],
    constants: &[],
    macros: &[],
    operators: &["=", "+", "-", "*", "/", "<", ">"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
};

/// TeX, BibTeX, and documentation grammars.
pub(crate) static TEX_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "%",
    block_comment: ("", ""),
    string_delims: &[],
    keywords: &["begin", "document", "end", "item", "section", "subsection", "usepackage"],
    types: &[],
    constants: &[],
    macros: &["cite", "documentclass", "frac", "include", "label", "ref", "textbf", "textit"],
    operators: &[],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
};

/// Erlang and Erlang-derived syntax.
pub(crate) static ERLANG_RULES: RuleSet = RuleSet {
    lexer_kind: LexerKind::Code,
    line_comment: "%",
    block_comment: ("", ""),
    string_delims: SCRIPT_STRINGS,
    keywords: &["after", "begin", "case", "catch", "end", "fun", "if", "let", "of", "receive", "try", "when"],
    types: &["false", "nil", "true"],
    constants: &[],
    macros: &[],
    operators: &["->", ":=", "==", "/=", "=", "+", "-", "*", "/", "<", ">"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
};
