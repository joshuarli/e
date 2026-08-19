//! Syntax highlighting engine.
//!
//! Byte-by-byte highlighter inspired by kilo/kibi. Produces one `Kind` per
//! byte, then maps to per-char highlights for the renderer.

use crate::languages::{LexerKind, RuleSet};

fn utf8_char_len(first_byte: u8) -> usize {
    if first_byte < 0xC0 {
        1
    } else if first_byte < 0xE0 {
        2
    } else if first_byte < 0xF0 {
        3
    } else {
        4
    }
}

/// A zero-based logical line and character column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

impl TextPosition {
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

// -- Types ------------------------------------------------------------------

/// Semantic category assigned to one source byte or rendered character.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Kind {
    #[default]
    Normal,
    Keyword,
    Type,
    String,
    Comment,
    Number,
    Bracket,
    Operator,
    Function,
    Constant,
    Macro,
}

/// Internal multiline scanner state.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum StateKind {
    #[default]
    Normal,
    BlockComment,
    RustBlockComment(u16),
    MultiLineString(u8),
    RustRawString(u8),
    CssBlock,
    FencedCodeBlock,
}

/// Opaque state carried from one source line to the next.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct State(StateKind);

impl State {
    /// Return the initial state for a new document or stream.
    pub const fn normal() -> Self {
        Self(StateKind::Normal)
    }

    /// Return whether no multiline lexical construct is active.
    pub const fn is_normal(self) -> bool {
        matches!(self.0, StateKind::Normal)
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

/// Allocate a per-byte result for the internal rule-table tests.
#[cfg(test)]
fn highlight_line(line: &[u8], state: StateKind, rules: &RuleSet) -> (Vec<Kind>, StateKind) {
    let mut hl = Vec::new();
    hl.resize(line.len(), Kind::Normal);
    let next_state = highlight_line_into_rules(line, state, rules, &[], &mut hl);
    (hl, next_state)
}

fn rust_comment_state(depth: u16) -> StateKind {
    if depth <= 1 {
        StateKind::BlockComment
    } else {
        StateKind::RustBlockComment(depth)
    }
}

/// Scan a Rust block comment, retaining nesting depth when it crosses a line.
fn scan_rust_block_comment(
    line: &[u8],
    mark_start: usize,
    scan_start: usize,
    mut depth: u16,
    block_open: &[u8],
    block_close: &[u8],
    hl: &mut [Kind],
) -> (usize, StateKind) {
    let mut i = scan_start;
    while i < line.len() {
        if starts_with_at(line, block_open, i) {
            depth = depth.saturating_add(1);
            i += block_open.len();
        } else if starts_with_at(line, block_close, i) {
            depth = depth.saturating_sub(1);
            i += block_close.len();
            if depth == 0 {
                for byte in &mut hl[mark_start..i] {
                    *byte = Kind::Comment;
                }
                return (i, StateKind::Normal);
            }
        } else {
            i += 1;
        }
    }
    for byte in &mut hl[mark_start..] {
        *byte = Kind::Comment;
    }
    (line.len(), rust_comment_state(depth))
}

fn rust_raw_string_open(line: &[u8], pos: usize) -> Option<(usize, u8)> {
    let prefix_len = if line.get(pos) == Some(&b'r') {
        1
    } else if line.get(pos) == Some(&b'b') && line.get(pos + 1) == Some(&b'r') {
        2
    } else {
        return None;
    };

    let mut hash_count = 0usize;
    let mut quote = pos + prefix_len;
    while line.get(quote) == Some(&b'#') {
        hash_count += 1;
        if hash_count > u8::MAX as usize {
            return None;
        }
        quote += 1;
    }
    (line.get(quote) == Some(&b'"')).then_some((quote + 1, hash_count as u8))
}

fn rust_raw_string_close(line: &[u8], pos: usize, hash_count: u8) -> Option<usize> {
    if line.get(pos) != Some(&b'"') {
        return None;
    }
    let mut end = pos + 1;
    for _ in 0..hash_count {
        if line.get(end) != Some(&b'#') {
            return None;
        }
        end += 1;
    }
    Some(end)
}

fn scan_rust_raw_string(
    line: &[u8],
    start: usize,
    hash_count: u8,
    hl: &mut [Kind],
) -> (usize, StateKind) {
    let mut i = start;
    while i < line.len() {
        if let Some(end) = rust_raw_string_close(line, i, hash_count) {
            for byte in &mut hl[start..end] {
                *byte = Kind::String;
            }
            return (end, StateKind::Normal);
        }
        i += 1;
    }
    for byte in &mut hl[start..] {
        *byte = Kind::String;
    }
    (line.len(), StateKind::RustRawString(hash_count))
}

fn rust_lifetime_end(line: &[u8], pos: usize) -> Option<usize> {
    if line.get(pos) != Some(&b'\'')
        || !line
            .get(pos + 1)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
    {
        return None;
    }
    let mut end = pos + 2;
    while line
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    // `'a'` is a character literal, not a lifetime.
    (line.get(end) != Some(&b'\'')).then_some(end)
}

fn highlight_line_code(
    line: &[u8],
    state: StateKind,
    rules: &RuleSet,
    user_types: &[Vec<u8>],
    hl: &mut [Kind],
) -> StateKind {
    let len = line.len();
    let mut i = 0;
    let mut prev_sep = true;
    let mut current_state = state;

    let block_open = rules.block_comment.0.as_bytes();
    let block_close = rules.block_comment.1.as_bytes();
    let line_com = rules.line_comment.as_bytes();

    // Handle entering in a multiline state
    match current_state {
        StateKind::BlockComment if rules.lexer_kind == LexerKind::Rust => {
            let (next, next_state) =
                scan_rust_block_comment(line, 0, 0, 1, block_open, block_close, hl);
            i = next;
            current_state = next_state;
            if current_state != StateKind::Normal {
                return current_state;
            }
            prev_sep = true;
        }
        StateKind::BlockComment => {
            while i < len {
                if starts_with_at(line, block_close, i) {
                    let end = i + block_close.len();
                    for b in &mut hl[i..end] {
                        *b = Kind::Comment;
                    }
                    i = end;
                    current_state = StateKind::Normal;
                    prev_sep = true;
                    break;
                }
                hl[i] = Kind::Comment;
                i += 1;
            }
            if current_state == StateKind::BlockComment {
                return StateKind::BlockComment;
            }
        }
        StateKind::RustBlockComment(depth) => {
            let (next, next_state) =
                scan_rust_block_comment(line, 0, 0, depth, block_open, block_close, hl);
            i = next;
            current_state = next_state;
            if current_state != StateKind::Normal {
                return current_state;
            }
            prev_sep = true;
        }
        StateKind::MultiLineString(idx) => {
            let close = rules.string_delims[idx as usize].close.as_bytes();
            while i < len {
                // Check for backslash escape
                if line[i] == b'\\' && i + 1 < len {
                    hl[i] = Kind::String;
                    hl[i + 1] = Kind::String;
                    i += 2;
                    continue;
                }
                if starts_with_at(line, close, i) {
                    let end = i + close.len();
                    for b in &mut hl[i..end] {
                        *b = Kind::String;
                    }
                    i = end;
                    current_state = StateKind::Normal;
                    prev_sep = true;
                    break;
                }
                hl[i] = Kind::String;
                i += 1;
            }
            if matches!(current_state, StateKind::MultiLineString(_)) {
                return current_state;
            }
        }
        StateKind::RustRawString(hash_count) => {
            let (next, next_state) = scan_rust_raw_string(line, 0, hash_count, hl);
            i = next;
            current_state = next_state;
            if current_state != StateKind::Normal {
                return current_state;
            }
            prev_sep = true;
        }
        StateKind::CssBlock => {}
        StateKind::Normal => {}
        StateKind::FencedCodeBlock => {}
    }

    // Main loop
    while i < len {
        // Line comment
        if !line_com.is_empty() && starts_with_at(line, line_com, i) {
            for b in &mut hl[i..len] {
                *b = Kind::Comment;
            }
            return StateKind::Normal;
        }

        // Block comment start
        if !block_open.is_empty() && starts_with_at(line, block_open, i) {
            if rules.lexer_kind == LexerKind::Rust {
                let (next, next_state) = scan_rust_block_comment(
                    line,
                    i,
                    i + block_open.len(),
                    1,
                    block_open,
                    block_close,
                    hl,
                );
                i = next;
                if next_state != StateKind::Normal {
                    return next_state;
                }
                prev_sep = true;
                continue;
            }
            let start = i;
            i += block_open.len();
            // Scan for close on same line
            let mut found = false;
            while i < len {
                if starts_with_at(line, block_close, i) {
                    let end = i + block_close.len();
                    for b in &mut hl[start..end] {
                        *b = Kind::Comment;
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
                    *b = Kind::Comment;
                }
                return StateKind::BlockComment;
            }
            continue;
        }

        // Rust raw strings (including raw byte strings) use a matching number
        // of `#` markers and do not treat backslashes as escapes.
        if rules.lexer_kind == LexerKind::Rust
            && prev_sep
            && let Some((content_start, hash_count)) = rust_raw_string_open(line, i)
        {
            let (next, next_state) = scan_rust_raw_string(line, content_start, hash_count, hl);
            for byte in &mut hl[i..content_start] {
                *byte = Kind::String;
            }
            i = next;
            if next_state != StateKind::Normal {
                return next_state;
            }
            prev_sep = true;
            continue;
        }

        // A Rust lifetime (`'a`, `'static`) is an identifier-like token, not
        // an unterminated character literal. Character literals still use the
        // ordinary quoted-string path below.
        if rules.lexer_kind == LexerKind::Rust
            && let Some(end) = rust_lifetime_end(line, i)
        {
            for byte in &mut hl[i..end] {
                *byte = Kind::Type;
            }
            i = end;
            prev_sep = false;
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
                        hl[i] = Kind::String;
                        hl[i + 1] = Kind::String;
                        i += 2;
                        continue;
                    }
                    if starts_with_at(line, close, i) {
                        let end = i + close.len();
                        for b in &mut hl[start..end] {
                            *b = Kind::String;
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
                        *b = Kind::String;
                    }
                    if delim.multiline {
                        return StateKind::MultiLineString(di as u8);
                    }
                    return StateKind::Normal;
                }
                matched_string = true;
                break;
            }
        }
        if matched_string {
            continue;
        }

        // Numbers (after separator)
        let starts_range_after_dot = i > 0 && line[i] == b'.' && line[i - 1] == b'.';
        if rules.highlight_numbers && prev_sep && !starts_range_after_dot && is_digit_start(line, i)
        {
            let start = i;
            i = scan_number_end(line, i);
            for b in &mut hl[start..i] {
                *b = Kind::Number;
            }
            prev_sep = false;
            continue;
        }

        // Keywords, types, constants, macros (after separator)
        if prev_sep && (line[i].is_ascii_alphabetic() || line[i] == b'_') {
            // Extract the full identifier once.
            let id_start = i;
            i += 1;
            while i < len && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
                i += 1;
            }
            let id = &line[id_start..i];

            if let Some(hl_type) = identifier_kind(id, rules) {
                for b in &mut hl[id_start..i] {
                    *b = hl_type;
                }
                prev_sep = false;
                continue;
            }

            // User-defined types (scanned from type declarations)
            if !user_types.is_empty() && user_types.iter().any(|t| t.as_slice() == id) {
                for b in &mut hl[id_start..i] {
                    *b = Kind::Type;
                }
                prev_sep = false;
                continue;
            }

            // Rust-style macros: ident!
            if i < len && line[i] == b'!' && rules.highlight_bang_macros {
                // Only treat as macro if the `!` is not followed by `=` (i.e. not `!=`)
                if i + 1 >= len || line[i + 1] != b'=' {
                    for b in &mut hl[id_start..i] {
                        *b = Kind::Macro;
                    }
                    hl[i] = Kind::Macro; // the `!`
                    i += 1;
                    prev_sep = true;
                    continue;
                }
            }
            // Function calls: ident(
            if rules.highlight_fn_calls && i < len && line[i] == b'(' {
                for b in &mut hl[id_start..i] {
                    *b = Kind::Function;
                }
                // Don't advance i — let the main loop process '(' as a bracket
                prev_sep = true;
                continue;
            }
            // UPPER_SNAKE_CASE constants (at least 2 chars, all uppercase/digit/underscore,
            // at least one letter)
            if rules.highlight_upper_constants && i - id_start >= 2 {
                let all_upper = id
                    .iter()
                    .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
                let has_letter = id.iter().any(|&b| b.is_ascii_uppercase());
                if all_upper && has_letter {
                    for b in &mut hl[id_start..i] {
                        *b = Kind::Constant;
                    }
                    prev_sep = false;
                    continue;
                }
            }
            prev_sep = false;
            continue;
        }

        // Operators (multi-char like &&, ||, !=, etc.)
        if !rules.operators.is_empty()
            && let Some(advance) = try_operator(line, i, rules.operators, hl)
        {
            i += advance;
            prev_sep = true;
            continue;
        }

        if matches!(line[i], b'(' | b')' | b'[' | b']' | b'{' | b'}') {
            hl[i] = Kind::Bracket;
        }
        prev_sep = is_separator(line[i]);
        i += 1;
    }

    StateKind::Normal
}

/// Add the small amount of structure that generic code scanning cannot infer
/// for markup, build files, and shell-like interpolation. The base scanner is
/// deliberately run first so multiline comments and quoted strings retain the
/// same state semantics across every language.
fn highlight_line_specialized(
    line: &[u8],
    state: StateKind,
    rules: &RuleSet,
    user_types: &[Vec<u8>],
    hl: &mut [Kind],
) -> StateKind {
    let mut next_state = highlight_line_code(line, state, rules, user_types, hl);
    match rules.lexer_kind {
        LexerKind::Bash => highlight_bash_variables(line, hl),
        LexerKind::Go => highlight_go_structure(line, hl),
        LexerKind::Python => highlight_python_structure(line, hl),
        LexerKind::C => {
            highlight_c_preprocessor(line, hl);
            highlight_c_typedef_name(line, hl);
        }
        LexerKind::Html => highlight_html_tags(line, hl),
        LexerKind::Css => {
            let in_block = matches!(state, StateKind::CssBlock);
            next_state = if highlight_css_structure(line, hl, in_block) {
                StateKind::CssBlock
            } else {
                StateKind::Normal
            };
        }
        LexerKind::Makefile => highlight_makefile_structure(line, hl),
        LexerKind::Dockerfile => highlight_dockerfile_variables(line, hl),
        LexerKind::Script => {
            highlight_script_interpolation(line, rules, hl);
            highlight_script_capitalized_types(line, hl);
        }
        LexerKind::TypeScript => {
            highlight_script_interpolation(line, rules, hl);
            highlight_typescript_structure(line, hl);
        }
        _ => {}
    }
    next_state
}

fn mark_range(hl: &mut [Kind], start: usize, end: usize, kind: Kind) {
    if start < end && end <= hl.len() {
        for byte in &mut hl[start..end] {
            *byte = kind;
        }
    }
}

fn highlight_bash_variables(line: &[u8], hl: &mut [Kind]) {
    let mut i = 0;
    while i < line.len() {
        if hl[i] == Kind::String || hl[i] == Kind::Comment || line[i] != b'$' {
            i += 1;
            continue;
        }
        let end = if line.get(i + 1) == Some(&b'{') {
            let mut end = i + 2;
            while end < line.len() && line[end] != b'}' {
                end += 1;
            }
            (end + usize::from(end < line.len())).min(line.len())
        } else if line
            .get(i + 1)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'?' | b'#' | b'@' | b'*' | b'!'))
        {
            let mut end = i + 2;
            while end < line.len() && (line[end].is_ascii_alphanumeric() || line[end] == b'_') {
                end += 1;
            }
            end
        } else {
            i += 1;
            continue;
        };
        if (i..end).all(|index| hl[index] == Kind::Normal) {
            mark_range(hl, i, end, Kind::Constant);
        }
        i = end;
    }
}

fn highlight_go_structure(line: &[u8], hl: &mut [Kind]) {
    // Fold punctuation, type adjustment, and dotted-call cleanup into one
    // walk; the generic scanner has already identified function candidates.
    let mut i = 0;
    while i < line.len() {
        if line[i].is_ascii_alphabetic() {
            let start = i;
            i += 1;
            while i < line.len() && line[i].is_ascii_alphanumeric() {
                i += 1;
            }
            if matches!(&line[start..i], b"type" | b"struct") {
                mark_range(hl, start, i, Kind::Type);
            } else if start > 0 && line[start - 1] == b'.' && hl[start] == Kind::Function {
                let mut end = i;
                while end < line.len()
                    && (line[end].is_ascii_alphanumeric() || line[end] == b'_')
                {
                    end += 1;
                }
                mark_range(hl, start, end, Kind::Normal);
                i = end;
            }
            continue;
        }
        if i > 0 && line[i - 1] == b'.' && hl[i] == Kind::Function {
            let mut end = i + 1;
            while end < line.len() && (line[end].is_ascii_alphanumeric() || line[end] == b'_') {
                end += 1;
            }
            mark_range(hl, i, end, Kind::Normal);
            i = end;
            continue;
        }
        if matches!(line[i], b'{' | b'}' | b'(' | b')') {
            hl[i] = Kind::Normal;
        } else if line[i] == b'.' {
            hl[i] = Kind::Operator;
        }
        i += 1;
    }
}

fn highlight_python_structure(line: &[u8], hl: &mut [Kind]) {
    let indent = line.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
    if line[indent..].starts_with(b"def ") {
        let start = indent + 4;
        let mut end = start;
        while line.get(end).is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_') {
            end += 1;
        }
        mark_range(hl, start, end, Kind::Normal);
    }
    for index in 0..line.len().saturating_sub(1) {
        if matches!(line[index], b'f' | b'r' | b'b' | b'F' | b'R' | b'B')
            && matches!(line[index + 1], b'"' | b'\'')
            && hl[index + 1] == Kind::String
        {
            hl[index] = Kind::String;
        }
    }
    let trimmed = &line[indent..];
    let is_definition = trimmed.starts_with(b"def ");
    if (trimmed.starts_with(b"if ") || trimmed.starts_with(b"elif ") || trimmed.starts_with(b"else"))
        && trimmed.iter().rposition(|&byte| byte == b':').is_some_and(|colon| {
            trimmed[colon + 1..].iter().all(u8::is_ascii_whitespace)
        })
    {
        if let Some(colon) = trimmed.iter().rposition(|&byte| byte == b':') {
            let absolute = indent + colon;
            if hl[absolute] == Kind::Normal {
                hl[absolute] = Kind::Bracket;
            }
        }
    }
    let mut index = 0;
    // Keep punctuation, dotted-call cleanup, and capitalized-call cleanup in
    // one walk after the definition and f-string prefix passes above.
    while index < line.len() {
        if hl[index] == Kind::String {
            index += 1;
            continue;
        }
        if !is_definition && matches!(line[index], b'(' | b')') {
            hl[index] = Kind::Normal;
        } else if line[index] == b'.' {
            hl[index] = Kind::Operator;
        }
        if index > 0 && line[index - 1] == b'.' && hl[index] == Kind::Function {
            let mut end = index + 1;
            while end < line.len() && (line[end].is_ascii_alphanumeric() || line[end] == b'_') {
                end += 1;
            }
            mark_range(hl, index, end, Kind::Normal);
            index = end;
            continue;
        }
        if line[index].is_ascii_uppercase() && hl[index] == Kind::Function {
            let mut end = index + 1;
            while end < line.len() && (line[end].is_ascii_alphanumeric() || line[end] == b'_') {
                end += 1;
            }
            mark_range(hl, index, end, Kind::Normal);
            index = end;
            continue;
        }
        index += 1;
    }
}

fn highlight_c_preprocessor(line: &[u8], hl: &mut [Kind]) {
    let indent = line.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
    if line.get(indent) != Some(&b'#') {
        return;
    }
    let mut end = indent + 1;
    while end < line.len() && line[end].is_ascii_alphabetic() {
        end += 1;
    }
    mark_range(hl, indent, end, Kind::Macro);

    if line[indent + 1..end].eq_ignore_ascii_case(b"define") {
        let mut name_start = end;
        while line.get(name_start).is_some_and(|byte| byte.is_ascii_whitespace()) {
            name_start += 1;
        }
        let mut name_end = name_start;
        while line
            .get(name_end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            name_end += 1;
        }
        mark_range(hl, name_start, name_end, Kind::Macro);
    }

    if line.get(end).is_some_and(|byte| byte.is_ascii_whitespace()) {
        let mut i = end;
        while i < line.len() {
            if line[i] == b'<' {
                let start = i;
                i += 1;
                while i < line.len() && line[i] != b'>' {
                    i += 1;
                }
                if i < line.len() {
                    i += 1;
                }
                mark_range(hl, start, i, Kind::String);
                break;
            }
            i += 1;
        }
    }
}

fn highlight_c_typedef_name(line: &[u8], hl: &mut [Kind]) {
    let indent = line.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
    if line.get(indent) != Some(&b'}') {
        return;
    }
    let mut start = indent + 1;
    while line.get(start).is_some_and(|byte| byte.is_ascii_whitespace()) {
        start += 1;
    }
    let mut end = start;
    while line
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    mark_range(hl, start, end, Kind::Type);
}

fn highlight_html_tags(line: &[u8], hl: &mut [Kind]) {
    let mut i = 0;
    while i < line.len() {
        if line[i] != b'<' || hl[i] == Kind::Comment || hl[i] == Kind::String {
            i += 1;
            continue;
        }
        let tag_start = i;
        let mut cursor = i + 1;
        if line.get(cursor) == Some(&b'/') {
            cursor += 1;
        }
        if line.get(cursor) == Some(&b'!') {
            mark_range(hl, tag_start, cursor + 1, Kind::Bracket);
            let name_start = cursor + 1;
            let mut name_end = name_start;
            while name_end < line.len() && line[name_end].is_ascii_alphabetic() {
                name_end += 1;
            }
            mark_range(hl, name_start, name_end, Kind::Keyword);
            i = name_end;
            while i < line.len() && line[i] != b'>' {
                i += 1;
            }
            if i < line.len() {
                mark_range(hl, i, i + 1, Kind::Bracket);
            }
            continue;
        }
        if !line.get(cursor).is_some_and(|byte| byte.is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        let name_start = cursor;
        while cursor < line.len() && (line[cursor].is_ascii_alphanumeric() || matches!(line[cursor], b':' | b'-')) {
            cursor += 1;
        }
        mark_range(hl, tag_start, name_start, Kind::Bracket);
        mark_range(hl, name_start, cursor, Kind::Keyword);
        let mut quote = None;
        while cursor < line.len() {
            if let Some(delimiter) = quote {
                if line[cursor] == delimiter && hl[cursor] == Kind::String {
                    quote = None;
                }
                cursor += 1;
                continue;
            }
            if line[cursor] == b'>' {
                mark_range(hl, cursor, cursor + 1, Kind::Bracket);
                break;
            }
            if matches!(line[cursor], b'"' | b'\'') && hl[cursor] == Kind::String {
                quote = Some(line[cursor]);
                cursor += 1;
                continue;
            }
            if line[cursor].is_ascii_alphabetic() || line[cursor] == b'_' {
                let attr_start = cursor;
                while cursor < line.len() && (line[cursor].is_ascii_alphanumeric() || matches!(line[cursor], b'_' | b'-' | b':')) {
                    cursor += 1;
                }
                let mut lookahead = cursor;
                while line.get(lookahead) == Some(&b' ') {
                    lookahead += 1;
                }
                if line.get(lookahead) == Some(&b'=') {
                    mark_range(hl, attr_start, cursor, Kind::Constant);
                }
                continue;
            }
            cursor += 1;
        }
        i = cursor.saturating_add(1);
    }
}

fn highlight_css_structure(line: &[u8], hl: &mut [Kind], mut in_block: bool) -> bool {
    let mut i = 0;
    while i < line.len() {
        if hl[i] == Kind::Comment || hl[i] == Kind::String {
            i += 1;
            continue;
        }
        match line[i] {
            b'(' | b')' => {
                hl[i] = Kind::Normal;
                i += 1;
            }
            b'{' => {
                hl[i] = Kind::Normal;
                in_block = true;
                i += 1;
            }
            b'}' => {
                hl[i] = Kind::Normal;
                in_block = false;
                i += 1;
            }
            b'#' if !in_block => {
                i += 1;
            }
            b'#' => {
                let start = i;
                i += 1;
                while i < line.len() && line[i].is_ascii_hexdigit() {
                    i += 1;
                }
                if matches!(i - start, 4 | 5 | 7 | 9) {
                    mark_range(hl, start + 1, i, Kind::Constant);
                }
            }
            _ if !in_block => {
                while i < line.len() && !matches!(line[i], b'{' | b'}') {
                    if hl[i] == Kind::Comment || hl[i] == Kind::String {
                        break;
                    }
                    i += 1;
                }
                // Selectors are intentionally left as normal text; syntect's
                // selector scope is structural rather than a name/type token.
            }
            _ if in_block && (line[i].is_ascii_alphabetic() || line[i] == b'-') => {
                let start = i;
                while i < line.len() && (line[i].is_ascii_alphanumeric() || line[i] == b'-') {
                    i += 1;
                }
                let mut lookahead = i;
                while line.get(lookahead) == Some(&b' ') {
                    lookahead += 1;
                }
                if line.get(lookahead) == Some(&b':') {
                    let name_start = start + line[start..i].iter().take_while(|&&byte| byte == b'-').count();
                    mark_range(hl, name_start, i, Kind::Type);
                }
            }
            _ => i += 1,
        }
    }

    // CSS units follow a numeric literal and are semantically distinct from it.
    let mut number_end = 0;
    while number_end < line.len() {
        if hl[number_end] == Kind::Number {
            let mut digit_end = number_end;
            while digit_end < line.len() && hl[digit_end] == Kind::Number {
                digit_end += 1;
            }
            let mut unit_end = digit_end;
            while unit_end < line.len() && line[unit_end].is_ascii_alphabetic() {
                unit_end += 1;
            }
            if unit_end > digit_end {
                mark_range(hl, digit_end, unit_end, Kind::Keyword);
            }
            number_end = unit_end.max(digit_end);
        } else {
            number_end += 1;
        }
    }
    in_block
}

fn highlight_makefile_structure(line: &[u8], hl: &mut [Kind]) {
    let indent = line.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
    if indent == 0 && !line.starts_with(b"#") {
        if let Some(colon) = line.iter().position(|&byte| byte == b':') {
            if colon > 0 && line.get(colon + 1) != Some(&b'=') {
                mark_range(hl, 0, colon, Kind::Function);
                hl[colon] = Kind::Operator;
                let mut prerequisite_start = colon + 1;
                while line.get(prerequisite_start) == Some(&b' ') {
                    prerequisite_start += 1;
                }
                mark_range(hl, prerequisite_start, line.len(), Kind::String);
            }
        }
        if let Some(assign) = line.windows(2).position(|pair| pair == b":=") {
            let mut value_start = assign + 2;
            while line.get(value_start) == Some(&b' ') {
                value_start += 1;
            }
            if value_start < line.len() {
                mark_range(hl, value_start, line.len(), Kind::String);
            }
        }
    }
    highlight_dollar_expansions(line, hl);
}

fn highlight_dockerfile_variables(line: &[u8], hl: &mut [Kind]) {
    highlight_dollar_expansions(line, hl);
}

fn highlight_dollar_expansions(line: &[u8], hl: &mut [Kind]) {
    let mut i = 0;
    while i < line.len() {
        if line[i] != b'$' || hl[i] == Kind::Comment || hl[i] == Kind::String {
            i += 1;
            continue;
        }
        let end = if line.get(i + 1) == Some(&b'(') || line.get(i + 1) == Some(&b'{') {
            let close = if line[i + 1] == b'(' { b')' } else { b'}' };
            let mut end = i + 2;
            while end < line.len() && line[end] != close {
                end += 1;
            }
            (end + usize::from(end < line.len())).min(line.len())
        } else if line.get(i + 1).is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'@' || *byte == b'<') {
            (i + 2).min(line.len())
        } else {
            i += 1;
            continue;
        };
        mark_range(hl, i, end, Kind::Macro);
        i = end;
    }
}

fn highlight_script_interpolation(line: &[u8], rules: &RuleSet, hl: &mut [Kind]) {
    let mut i = 0;
    while i + 1 < line.len() {
        if line[i] != b'$' || line[i + 1] != b'{' || hl[i] != Kind::String {
            i += 1;
            continue;
        }
        mark_range(hl, i, i + 2, Kind::Normal);
        let mut cursor = i + 2;
        while cursor < line.len() {
            if line[cursor] == b'}' {
                mark_range(hl, cursor, cursor + 1, Kind::Normal);
                i = cursor + 1;
                break;
            }
            if line[cursor] == b'.' {
                hl[cursor] = Kind::Operator;
                cursor += 1;
                continue;
            }
            if line[cursor].is_ascii_digit() {
                let start = cursor;
                while cursor < line.len() && line[cursor].is_ascii_digit() {
                    cursor += 1;
                }
                mark_range(hl, start, cursor, Kind::Number);
                continue;
            }
            if line[cursor].is_ascii_alphabetic() || line[cursor] == b'_' {
                let start = cursor;
                cursor += 1;
                while cursor < line.len() && (line[cursor].is_ascii_alphanumeric() || line[cursor] == b'_') {
                    cursor += 1;
                }
                let id = &line[start..cursor];
                let kind = if keyword_search(id, rules.keywords) {
                    Kind::Keyword
                } else if keyword_search(id, rules.types) {
                    Kind::Type
                } else if line.get(cursor) == Some(&b'(') {
                    Kind::Function
                } else {
                    Kind::Normal
                };
                mark_range(hl, start, cursor, kind);
                continue;
            }
            cursor += 1;
        }
        if i == cursor {
            i += 1;
        }
    }
}

fn highlight_script_capitalized_types(line: &[u8], hl: &mut [Kind]) {
    let mut i = 0;
    while i < line.len() {
        if !line[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < line.len() && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
            i += 1;
        }
        if line[start].is_ascii_uppercase()
            && hl[start..i].iter().all(|kind| *kind == Kind::Function)
        {
            mark_range(hl, start, i, Kind::Normal);
        }
    }
}

fn highlight_typescript_structure(line: &[u8], hl: &mut [Kind]) {
    let Some(function_start) = line.windows(8).position(|window| window == b"function") else {
        return;
    };
    let mut cursor = function_start;
    while cursor < line.len() && line[cursor] != b'(' {
        if hl[cursor] == Kind::Keyword || hl[cursor] == Kind::Function {
            hl[cursor] = Kind::Normal;
        }
        cursor += 1;
    }
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

fn is_base_digit(byte: u8, base: u8) -> bool {
    match base {
        2 => matches!(byte, b'0' | b'1'),
        8 => byte.is_ascii_digit() && byte < b'8',
        16 => byte.is_ascii_hexdigit(),
        _ => byte.is_ascii_digit(),
    }
}

fn consume_digits(line: &[u8], mut pos: usize, base: u8) -> usize {
    while pos < line.len() {
        if is_base_digit(line[pos], base) {
            pos += 1;
        } else if line[pos] == b'_' && pos + 1 < line.len() && is_base_digit(line[pos + 1], base) {
            pos += 1;
        } else {
            break;
        }
    }
    pos
}

fn is_number_suffix(suffix: &[u8]) -> bool {
    matches!(
        suffix,
        b"u8"
            | b"u16"
            | b"u32"
            | b"u64"
            | b"u128"
            | b"usize"
            | b"i8"
            | b"i16"
            | b"i32"
            | b"i64"
            | b"i128"
            | b"isize"
            | b"f32"
            | b"f64"
            | b"f"
            | b"j"
            | b"n"
            | b"ms"
            | b"us"
            | b"ns"
            | b"ps"
            | b"s"
            | b"m"
            | b"h"
            | b"d"
    )
}

/// Consume a common numeric literal without swallowing an arbitrary identifier.
fn scan_number_end(line: &[u8], start: usize) -> usize {
    let mut end = start;

    if line.get(start) == Some(&b'0') {
        let prefixed = match line.get(start + 1).copied() {
            Some(b'x' | b'X') => Some(16),
            Some(b'b' | b'B') => Some(2),
            Some(b'o' | b'O') => Some(8),
            _ => None,
        };
        if let Some(base) = prefixed {
            let digits_start = start + 2;
            let digits_end = consume_digits(line, digits_start, base);
            if digits_end == digits_start {
                return start + 1;
            }
            end = digits_end;
        }
    }

    if end == start {
        end = consume_digits(line, start, 10);
        if line.get(end) == Some(&b'.')
            && line.get(end + 1).is_some_and(|byte| byte.is_ascii_digit())
            && line.get(end + 2) != Some(&b'.')
        {
            end = consume_digits(line, end + 1, 10);
        }

        if line
            .get(end)
            .is_some_and(|byte| matches!(byte, b'e' | b'E'))
        {
            let mut exponent_end = end + 1;
            if matches!(line.get(exponent_end), Some(b'+' | b'-')) {
                exponent_end += 1;
            }
            let digits_end = consume_digits(line, exponent_end, 10);
            if digits_end > exponent_end {
                end = digits_end;
            }
        }
    }

    let suffix_start = end;
    let mut suffix_end = suffix_start;
    while line
        .get(suffix_end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
    {
        suffix_end += 1;
    }
    if suffix_end > suffix_start && is_number_suffix(&line[suffix_start..suffix_end]) {
        end = suffix_end;
    } else if line.get(suffix_start) == Some(&b'_') {
        let mut underscored_end = suffix_start + 1;
        while line
            .get(underscored_end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            underscored_end += 1;
        }
        if underscored_end > suffix_start + 1
            && is_number_suffix(&line[suffix_start + 1..underscored_end])
        {
            end = underscored_end;
        }
    }
    end
}

/// Match the longest operator, regardless of static table order.
fn try_operator(line: &[u8], pos: usize, ops: &[&str], hl: &mut [Kind]) -> Option<usize> {
    // Operators in the rule tables always begin with punctuation. Avoid
    // walking every operator candidate for the common identifier/whitespace
    // bytes that reach this fallback path after the specialized scanners.
    let first = line[pos];
    if first.is_ascii_alphanumeric() || first == b'_' || first.is_ascii_whitespace() {
        return None;
    }

    let mut best_len = 0;
    // Operator tables are shared by generic lexers; reject different first
    // bytes before doing a longer slice comparison at each source position.
    for &op in ops {
        let ob = op.as_bytes();
        if ob.len() > best_len
            && ob.first() == Some(&first)
            && starts_with_at(line, ob, pos)
        {
            best_len = ob.len();
        }
    }
    if best_len == 0 {
        return None;
    }
    for b in &mut hl[pos..pos + best_len] {
        *b = Kind::Operator;
    }
    Some(best_len)
}

/// Binary search a **sorted** keyword list for an exact match.
#[inline]
fn keyword_search(id: &[u8], words: &[&str]) -> bool {
    !words.is_empty() && words.binary_search_by(|w| w.as_bytes().cmp(id)).is_ok()
}

/// Classify an identifier using the shared rule-table precedence.
///
/// Empty categories are skipped before entering their binary search. Most
/// languages do not define constants or macros, so this keeps the common
/// identifier path to the categories that can actually match.
#[inline]
fn identifier_kind(id: &[u8], rules: &RuleSet) -> Option<Kind> {
    if keyword_search(id, rules.keywords) {
        Some(Kind::Keyword)
    } else if keyword_search(id, rules.types) {
        Some(Kind::Type)
    } else if keyword_search(id, rules.constants) {
        Some(Kind::Constant)
    } else if keyword_search(id, rules.macros) {
        Some(Kind::Macro)
    } else {
        None
    }
}

// -- Semver highlighting ----------------------------------------------------

/// Post-pass: highlight semver patterns like v1.2.3 or 0.3.5-beta.1
fn highlight_semver(line: &[u8], hl: &mut [Kind], lexer_kind: LexerKind, state: StateKind) {
    let len = line.len();
    if len < 5 || !line.contains(&b'.') {
        return;
    }
    let mut i = 0;
    let mut raw_hash_count = match state {
        StateKind::RustRawString(hash_count) => Some(hash_count),
        _ => None,
    };
    while i < len {
        if lexer_kind == LexerKind::Rust {
            if let Some(hash_count) = raw_hash_count {
                if let Some(end) = rust_raw_string_close(line, i, hash_count) {
                    i = end;
                    raw_hash_count = None;
                } else {
                    i += 1;
                }
                continue;
            }
            // A raw-string prefix starts a fresh String run. Requiring that
            // boundary keeps this post-pass from interpreting `r"..."`-like
            // text embedded in an ordinary string as a second Rust token.
            if hl[i] == Kind::String
                && (i == 0 || hl[i - 1] != Kind::String)
                && let Some((content_start, hash_count)) = rust_raw_string_open(line, i)
            {
                raw_hash_count = Some(hash_count);
                i = content_start;
                continue;
            }
        }
        // Don't start inside a comment
        if hl[i] == Kind::Comment {
            i += 1;
            continue;
        }
        let start = i;
        // Optional v/V prefix
        if line[i] == b'v' || line[i] == b'V' {
            i += 1;
            if i >= len || !line[i].is_ascii_digit() {
                continue; // not a version, resume from after v
            }
        } else if !line[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // Must not be preceded by alphanumeric (avoid matching inside words)
        if start > 0 && (line[start - 1].is_ascii_alphanumeric() || line[start - 1] == b'_') {
            i += 1;
            continue;
        }
        // MAJOR: one or more digits
        while i < len && line[i].is_ascii_digit() {
            i += 1;
        }
        // First dot
        if i >= len || line[i] != b'.' {
            continue;
        }
        i += 1;
        // MINOR: one or more digits
        if i >= len || !line[i].is_ascii_digit() {
            continue;
        }
        while i < len && line[i].is_ascii_digit() {
            i += 1;
        }
        // Second dot
        if i >= len || line[i] != b'.' {
            continue;
        }
        i += 1;
        // PATCH: one or more digits
        if i >= len || !line[i].is_ascii_digit() {
            continue;
        }
        while i < len && line[i].is_ascii_digit() {
            i += 1;
        }
        // Optional pre-release: -alpha.1, -beta.2, -rc.1
        if i < len && line[i] == b'-' {
            i += 1;
            while i < len && (line[i].is_ascii_alphanumeric() || line[i] == b'.' || line[i] == b'-')
            {
                i += 1;
            }
        }
        // Optional build metadata: +build.123
        if i < len && line[i] == b'+' {
            i += 1;
            while i < len && (line[i].is_ascii_alphanumeric() || line[i] == b'.' || line[i] == b'-')
            {
                i += 1;
            }
        }
        // Must not be followed by alphanumeric
        if i < len && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
            continue;
        }
        // Apply highlight
        for b in &mut hl[start..i] {
            *b = Kind::Type;
        }
    }
}

// -- JSON highlighting ------------------------------------------------------

fn highlight_line_json(line: &[u8], _state: StateKind, hl: &mut [Kind]) -> StateKind {
    let len = line.len();
    let mut i = 0;

    while i < len {
        // Skip whitespace
        if line[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // String — determine if it's a key (followed by ':') or a value
        if line[i] == b'"' {
            let start = i;
            i += 1;
            while i < len {
                if line[i] == b'\\' && i + 1 < len {
                    i += 2;
                    continue;
                }
                if line[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let str_end = i;
            // Look ahead past whitespace for ':'
            let mut peek = str_end;
            while peek < len && line[peek].is_ascii_whitespace() {
                peek += 1;
            }
            let hl_type = if peek < len && line[peek] == b':' {
                Kind::Keyword // key → yellow
            } else {
                Kind::String // value → green
            };
            for b in &mut hl[start..str_end] {
                *b = hl_type;
            }
            continue;
        }

        // Numbers
        if line[i] == b'-' || line[i].is_ascii_digit() {
            let start = i;
            if line[i] == b'-' {
                i += 1;
            }
            while i < len
                && (line[i].is_ascii_digit()
                    || line[i] == b'.'
                    || line[i] == b'e'
                    || line[i] == b'E'
                    || line[i] == b'+'
                    || line[i] == b'-')
            {
                i += 1;
            }
            if i > start + (if line[start] == b'-' { 1 } else { 0 }) {
                for b in &mut hl[start..i] {
                    *b = Kind::Number;
                }
                continue;
            }
        }

        // true, false, null
        for &(word, hl_type) in &[
            (&b"true"[..], Kind::Type),
            (&b"false"[..], Kind::Type),
            (&b"null"[..], Kind::Type),
        ] {
            if starts_with_at(line, word, i) {
                let end = i + word.len();
                if end >= len || !line[end].is_ascii_alphabetic() {
                    for b in &mut hl[i..end] {
                        *b = hl_type;
                    }
                    i = end;
                    break;
                }
            }
        }

        // Brackets
        if i < len && matches!(line[i], b'{' | b'}' | b'[' | b']') {
            hl[i] = Kind::Bracket;
        }

        i += 1;
    }

    StateKind::Normal
}

// -- YAML highlighting ------------------------------------------------------

fn highlight_line_yaml(line: &[u8], _state: StateKind, hl: &mut [Kind]) -> StateKind {
    let len = line.len();

    if len == 0 {
        return StateKind::Normal;
    }

    // Comment: # (at start or after whitespace)
    if let Some(comment_start) = find_yaml_comment(line) {
        for b in &mut hl[comment_start..len] {
            *b = Kind::Comment;
        }
        // Highlight the part before the comment
        if comment_start > 0 {
            highlight_yaml_content(&line[..comment_start], &mut hl[..comment_start]);
        }
        return StateKind::Normal;
    }

    highlight_yaml_content(line, hl);
    StateKind::Normal
}

fn find_yaml_comment(line: &[u8]) -> Option<usize> {
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    while i < line.len() {
        if line[i] == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if line[i] == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if line[i] == b'\\' && in_double_quote && i + 1 < line.len() {
            i += 1; // skip escaped char
        } else if line[i] == b'#'
            && !in_single_quote
            && !in_double_quote
            && (i == 0 || line[i - 1].is_ascii_whitespace())
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn highlight_yaml_content(line: &[u8], hl: &mut [Kind]) {
    let len = line.len();
    if len == 0 {
        return;
    }

    // Find the key: colon position (unquoted colon followed by space or end)
    let indent = line
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let rest = &line[indent..];

    // Anchor &name or alias *name
    if rest.starts_with(b"&") || rest.starts_with(b"*") {
        let end = indent
            + rest
                .iter()
                .take_while(|&&b| !b.is_ascii_whitespace() && b != b':')
                .count();
        for b in &mut hl[indent..end] {
            *b = Kind::Type;
        }
        return;
    }

    // Find unquoted colon that marks key: value
    if let Some(colon_pos) = find_yaml_colon(rest) {
        let abs_colon = indent + colon_pos;
        // Key portion (before colon)
        for b in &mut hl[indent..abs_colon] {
            *b = Kind::Keyword;
        }
        // Value portion (after colon + space)
        let val_start = abs_colon + 1;
        if val_start < len {
            highlight_yaml_value(&line[val_start..], &mut hl[val_start..]);
        }
        return;
    }

    // List item: - value
    if rest.starts_with(b"- ") {
        hl[indent] = Kind::Normal;
        let val_start = indent + 2;
        if val_start < len {
            // Check if the list item contains a key
            let item_rest = &line[val_start..];
            if let Some(colon_pos) = find_yaml_colon(item_rest) {
                let abs_colon = val_start + colon_pos;
                for b in &mut hl[val_start..abs_colon] {
                    *b = Kind::Keyword;
                }
                let after = abs_colon + 1;
                if after < len {
                    highlight_yaml_value(&line[after..], &mut hl[after..]);
                }
            } else {
                highlight_yaml_value(&line[val_start..], &mut hl[val_start..]);
            }
        }
    }
}

fn find_yaml_colon(line: &[u8]) -> Option<usize> {
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    while i < line.len() {
        if line[i] == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if line[i] == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if line[i] == b'\\' && in_double_quote && i + 1 < line.len() {
            i += 1;
        } else if line[i] == b':' && !in_single_quote && !in_double_quote {
            // Must be followed by space, end of line, or nothing
            if i + 1 >= line.len() || line[i + 1] == b' ' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn highlight_yaml_value(val: &[u8], hl: &mut [Kind]) {
    let trimmed_start = val.iter().take_while(|&&b| b == b' ').count();
    let trimmed = &val[trimmed_start..];

    if trimmed.is_empty() {
        return;
    }

    // Quoted strings
    if trimmed[0] == b'"' || trimmed[0] == b'\'' {
        let start = trimmed_start;
        let quote = trimmed[0];
        let mut i = 1;
        while i < trimmed.len() {
            if trimmed[i] == b'\\' && quote == b'"' && i + 1 < trimmed.len() {
                i += 2;
                continue;
            }
            if trimmed[i] == quote {
                i += 1;
                break;
            }
            i += 1;
        }
        for b in &mut hl[start..start + i] {
            *b = Kind::String;
        }
        return;
    }

    // true/false/null/yes/no
    for &(word, hl_type) in &[
        (&b"true"[..], Kind::Type),
        (&b"false"[..], Kind::Type),
        (&b"null"[..], Kind::Type),
        (&b"yes"[..], Kind::Type),
        (&b"no"[..], Kind::Type),
    ] {
        if trimmed.len() >= word.len()
            && trimmed[..word.len()].eq_ignore_ascii_case(word)
            && (trimmed.len() == word.len() || trimmed[word.len()].is_ascii_whitespace())
        {
            for b in &mut hl[trimmed_start..trimmed_start + word.len()] {
                *b = hl_type;
            }
            return;
        }
    }

    // Numbers
    if trimmed[0] == b'-' || trimmed[0].is_ascii_digit() || trimmed[0] == b'.' {
        let mut i = 0;
        if trimmed[i] == b'-' {
            i += 1;
        }
        let num_start = i;
        while i < trimmed.len()
            && (trimmed[i].is_ascii_digit()
                || trimmed[i] == b'.'
                || trimmed[i] == b'e'
                || trimmed[i] == b'E')
        {
            i += 1;
        }
        if i > num_start && (i >= trimmed.len() || trimmed[i].is_ascii_whitespace()) {
            for b in &mut hl[trimmed_start..trimmed_start + i] {
                *b = Kind::Number;
            }
            return;
        }
    }

    // Anchor/alias in value position
    if trimmed[0] == b'&' || trimmed[0] == b'*' {
        let end = trimmed
            .iter()
            .take_while(|&&b| !b.is_ascii_whitespace())
            .count();
        for b in &mut hl[trimmed_start..trimmed_start + end] {
            *b = Kind::Type;
        }
    }
}

// -- INI/Config highlighting ------------------------------------------------

fn highlight_line_ini(line: &[u8], _state: StateKind, hl: &mut [Kind]) -> StateKind {
    let len = line.len();

    if len == 0 {
        return StateKind::Normal;
    }

    // Skip leading whitespace
    let indent = line
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let rest = &line[indent..];

    if rest.is_empty() {
        return StateKind::Normal;
    }

    // Comment lines: ; or # at start (after optional whitespace)
    if rest[0] == b';' || rest[0] == b'#' {
        for b in &mut hl[indent..] {
            *b = Kind::Comment;
        }
        return StateKind::Normal;
    }

    // Section headers: [section]
    if rest[0] == b'[' {
        if let Some(close) = rest.iter().position(|&b| b == b']') {
            for b in &mut hl[indent..indent + close + 1] {
                *b = Kind::Keyword;
            }
            // Anything after ] could be an inline comment
            let after = indent + close + 1;
            if after < len {
                highlight_ini_inline_comment(line, hl, after);
            }
        }
        return StateKind::Normal;
    }

    // Key = value pairs
    if let Some(eq_pos) = rest.iter().position(|&b| b == b'=') {
        let abs_eq = indent + eq_pos;
        // Key (before =)
        for b in &mut hl[indent..abs_eq] {
            *b = Kind::Keyword;
        }
        // Value (after =)
        let val_start = abs_eq + 1;
        if val_start < len {
            highlight_ini_value(&line[val_start..], &mut hl[val_start..]);
        }
    }

    StateKind::Normal
}

fn highlight_ini_value(val: &[u8], hl: &mut [Kind]) {
    let trimmed_start = val.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    let trimmed = &val[trimmed_start..];

    if trimmed.is_empty() {
        return;
    }

    // Find inline comment (unquoted ; or # after whitespace)
    let comment_start = find_ini_inline_comment(val);

    let value_end = if let Some(cs) = comment_start {
        // Highlight the comment
        for b in &mut hl[cs..] {
            *b = Kind::Comment;
        }
        cs
    } else {
        val.len()
    };

    let trimmed_end = value_end.min(trimmed_start + trimmed.len());
    if trimmed_start >= trimmed_end {
        return;
    }
    let value_slice = &val[trimmed_start..trimmed_end];
    // Trim trailing whitespace from value for matching
    let value_trimmed_len = value_slice.len()
        - value_slice
            .iter()
            .rev()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
    if value_trimmed_len == 0 {
        return;
    }
    let value_trimmed = &value_slice[..value_trimmed_len];

    // Quoted strings
    if value_trimmed[0] == b'"' || value_trimmed[0] == b'\'' {
        let quote = value_trimmed[0];
        let mut i = 1;
        while i < value_trimmed.len() {
            if value_trimmed[i] == b'\\' && i + 1 < value_trimmed.len() {
                i += 2;
                continue;
            }
            if value_trimmed[i] == quote {
                i += 1;
                break;
            }
            i += 1;
        }
        for b in &mut hl[trimmed_start..trimmed_start + i] {
            *b = Kind::String;
        }
        return;
    }

    // Boolean types (case-insensitive): true, false, yes, no, on, off
    for keyword in &[
        &b"true"[..],
        &b"false"[..],
        &b"yes"[..],
        &b"no"[..],
        &b"on"[..],
        &b"off"[..],
    ] {
        if value_trimmed.len() == keyword.len() && value_trimmed.eq_ignore_ascii_case(keyword) {
            for b in &mut hl[trimmed_start..trimmed_start + keyword.len()] {
                *b = Kind::Type;
            }
            return;
        }
    }

    // Numbers (integers and floats)
    if value_trimmed[0] == b'-' || value_trimmed[0].is_ascii_digit() {
        let mut i = 0;
        if value_trimmed[i] == b'-' {
            i += 1;
        }
        let num_start = i;
        while i < value_trimmed.len()
            && (value_trimmed[i].is_ascii_digit() || value_trimmed[i] == b'.')
        {
            i += 1;
        }
        if i > num_start && i == value_trimmed.len() {
            for b in &mut hl[trimmed_start..trimmed_start + i] {
                *b = Kind::Number;
            }
        }
    }
}

/// Find an inline comment in an INI value: ; or # preceded by whitespace.
fn find_ini_inline_comment(val: &[u8]) -> Option<usize> {
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut i = 0;
    while i < val.len() {
        if val[i] == b'"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if val[i] == b'\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if val[i] == b'\\' && (in_double_quote || in_single_quote) && i + 1 < val.len() {
            i += 1; // skip escaped char
        } else if (val[i] == b';' || val[i] == b'#')
            && !in_double_quote
            && !in_single_quote
            && i > 0
            && val[i - 1].is_ascii_whitespace()
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Highlight inline comment after a section header closing bracket.
fn highlight_ini_inline_comment(line: &[u8], hl: &mut [Kind], start: usize) {
    let rest = &line[start..];
    let ws = rest
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let after_ws = start + ws;
    if after_ws < line.len() && (line[after_ws] == b';' || line[after_ws] == b'#') {
        for b in &mut hl[after_ws..] {
            *b = Kind::Comment;
        }
    }
}

// -- Markdown highlighting --------------------------------------------------

fn highlight_line_markdown(
    line: &[u8],
    state: StateKind,
    rules: &RuleSet,
    hl: &mut [Kind],
) -> StateKind {
    let len = line.len();

    let block_close = rules.block_comment.1.as_bytes();

    // Fenced code block: entering or continuing
    if state == StateKind::FencedCodeBlock {
        if len >= 3 && line[0] == b'`' && line[1] == b'`' && line[2] == b'`' {
            for b in &mut hl[..len] {
                *b = Kind::String;
            }
            return StateKind::Normal;
        }
        for b in &mut hl[..len] {
            *b = Kind::String;
        }
        return StateKind::FencedCodeBlock;
    }

    // Block comment continuation
    if state == StateKind::BlockComment {
        let mut i = 0;
        while i < len {
            if starts_with_at(line, block_close, i) {
                let end = i + block_close.len();
                for b in &mut hl[i..end] {
                    *b = Kind::Comment;
                }
                // hl[0..end] is all Comment; process remainder as inline markdown
                return highlight_line_markdown_inner(&line[end..], rules, &mut hl[end..]);
            }
            hl[i] = Kind::Comment;
            i += 1;
        }
        return StateKind::BlockComment;
    }

    // Fenced code block start
    if len >= 3 && line[0] == b'`' && line[1] == b'`' && line[2] == b'`' {
        for b in &mut hl[..len] {
            *b = Kind::String;
        }
        return StateKind::FencedCodeBlock;
    }

    // Horizontal rules: ---, ***, ___ (optionally with spaces)
    {
        let non_space_count = line.iter().filter(|&&b| b != b' ').count();
        if non_space_count >= 3 {
            let is_hr = line.iter().find(|&&b| b != b' ').is_some_and(|&ch| {
                matches!(ch, b'-' | b'*' | b'_') && line.iter().all(|&b| b == b' ' || b == ch)
            });
            if is_hr {
                for b in &mut hl[..len] {
                    *b = Kind::Comment;
                }
                return StateKind::Normal;
            }
        }
    }

    // Headers: # at line start
    if len > 0 && line[0] == b'#' {
        for b in &mut hl[..len] {
            *b = Kind::Keyword;
        }
        return StateKind::Normal;
    }

    // Blockquote: > at line start
    if len > 0 && line[0] == b'>' {
        hl[0] = Kind::Comment;
        if len > 1 && line[1] == b' ' {
            hl[1] = Kind::Comment;
        }
        let start = if len > 1 && line[1] == b' ' { 2 } else { 1 };
        return highlight_line_markdown_inner(&line[start..], rules, &mut hl[start..]);
    }

    // List markers: - , * , 1. at start (possibly indented)
    {
        let indent = line
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        let rest = &line[indent..];
        let marker_len = if rest.starts_with(b"- ") || rest.starts_with(b"* ") {
            2
        } else if rest.len() >= 2 && rest[0].is_ascii_digit() {
            // Check for "1. " style
            let mut dend = 0;
            while dend < rest.len() && rest[dend].is_ascii_digit() {
                dend += 1;
            }
            if dend > 0
                && dend < rest.len()
                && rest[dend] == b'.'
                && dend + 1 < rest.len()
                && rest[dend + 1] == b' '
            {
                dend + 2
            } else {
                0
            }
        } else {
            0
        };
        if marker_len > 0 {
            for b in &mut hl[indent..indent + marker_len] {
                *b = Kind::Number;
            }
            let after = indent + marker_len;
            return highlight_line_markdown_inner(&line[after..], rules, &mut hl[after..]);
        }
    }

    // Normal line — process inline elements
    highlight_line_markdown_inner(line, rules, hl)
}

/// Process inline markdown elements: inline code, bold, italic, HTML comments.
fn highlight_line_markdown_inner(line: &[u8], rules: &RuleSet, hl: &mut [Kind]) -> StateKind {
    let len = line.len();
    let mut i = 0;

    let block_open = rules.block_comment.0.as_bytes();
    let block_close = rules.block_comment.1.as_bytes();

    while i < len {
        // HTML comment start
        if !block_open.is_empty() && starts_with_at(line, block_open, i) {
            let start = i;
            i += block_open.len();
            let mut found = false;
            while i < len {
                if starts_with_at(line, block_close, i) {
                    let end = i + block_close.len();
                    for b in &mut hl[start..end] {
                        *b = Kind::Comment;
                    }
                    i = end;
                    found = true;
                    break;
                }
                i += 1;
            }
            if !found {
                for b in &mut hl[start..len] {
                    *b = Kind::Comment;
                }
                return StateKind::BlockComment;
            }
            continue;
        }

        // Inline code
        if line[i] == b'`' {
            let start = i;
            i += 1;
            while i < len && line[i] != b'`' {
                i += 1;
            }
            if i < len {
                i += 1; // consume closing `
                for b in &mut hl[start..i] {
                    *b = Kind::String;
                }
            }
            continue;
        }

        // Bold: **text**
        if i + 1 < len && line[i] == b'*' && line[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < len && !(line[i] == b'*' && line[i + 1] == b'*') {
                i += 1;
            }
            if i + 1 < len {
                i += 2; // consume closing **
                for b in &mut hl[start..i] {
                    *b = Kind::Keyword;
                }
            }
            continue;
        }

        // Italic: *text*
        if line[i] == b'*' {
            let start = i;
            i += 1;
            while i < len && line[i] != b'*' {
                i += 1;
            }
            if i < len {
                i += 1; // consume closing *
                for b in &mut hl[start..i] {
                    *b = Kind::Type;
                }
            }
            continue;
        }

        i += 1;
    }

    StateKind::Normal
}

// -- Bracket matching -------------------------------------------------------

fn bracket_pair(ch: u8) -> Option<(u8, bool)> {
    match ch {
        b'(' => Some((b')', true)),
        b')' => Some((b'(', false)),
        b'[' => Some((b']', true)),
        b']' => Some((b'[', false)),
        b'{' => Some((b'}', true)),
        b'}' => Some((b'{', false)),
        _ => None,
    }
}

/// Find the matching bracket for the bracket at `pos`.
/// `get_line(idx, buf)` fills `buf` with the raw bytes for line `idx`.
/// `scratch` is a caller-supplied buffer reused across calls (avoids per-line allocation).
/// Returns the position of the matching bracket, or None.
pub fn find_bracket_match(
    pos: TextPosition,
    get_line: &mut impl FnMut(usize, &mut Vec<u8>),
    scratch: &mut Vec<u8>,
    line_count: usize,
) -> Option<TextPosition> {
    get_line(pos.line, scratch);
    // Convert char column to byte index
    let byte_idx = character_column_to_byte(scratch, pos.column)?;
    if byte_idx >= scratch.len() {
        return None;
    }
    let ch = scratch[byte_idx];
    let (target, forward) = bracket_pair(ch)?;

    let mut depth: i32 = 0;
    let max_lines = 1000;

    if forward {
        let mut l = pos.line;
        let mut bi = byte_idx;
        let mut lines_scanned = 0;
        loop {
            while bi < scratch.len() {
                if scratch[bi] == ch {
                    depth += 1;
                } else if scratch[bi] == target {
                    depth -= 1;
                    if depth == 0 {
                        let column = byte_to_character_column(scratch, bi);
                        return Some(TextPosition::new(l, column));
                    }
                }
                bi += 1;
            }
            l += 1;
            lines_scanned += 1;
            if l >= line_count || lines_scanned >= max_lines {
                return None;
            }
            bi = 0;
            get_line(l, scratch);
        }
    } else {
        let mut l = pos.line;
        let mut bi = byte_idx as i64;
        let mut lines_scanned = 0;
        loop {
            while bi >= 0 {
                let b = bi as usize;
                if scratch[b] == ch {
                    depth += 1;
                } else if scratch[b] == target {
                    depth -= 1;
                    if depth == 0 {
                        let column = byte_to_character_column(scratch, b);
                        return Some(TextPosition::new(l, column));
                    }
                }
                bi -= 1;
            }
            if l == 0 {
                return None;
            }
            l -= 1;
            lines_scanned += 1;
            if lines_scanned >= max_lines {
                return None;
            }
            get_line(l, scratch);
            bi = scratch.len() as i64 - 1;
        }
    }
}

fn is_escaped(line: &[u8], idx: usize) -> bool {
    let mut backslashes = 0;
    let mut i = idx;
    while i > 0 {
        i -= 1;
        if line[i] == b'\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

pub fn find_quote_match(
    pos: TextPosition,
    get_line: &mut impl FnMut(usize, &mut Vec<u8>),
    scratch: &mut Vec<u8>,
    _line_count: usize,
) -> Option<TextPosition> {
    get_line(pos.line, scratch);
    let byte_idx = character_column_to_byte(scratch, pos.column)?;
    if byte_idx >= scratch.len() {
        return None;
    }
    let ch = scratch[byte_idx];
    if ch != b'"' && ch != b'\'' {
        return None;
    }
    if is_escaped(scratch, byte_idx) {
        return None;
    }
    // Collect all unescaped positions of this quote char on this line
    let mut positions = Vec::new();
    for i in 0..scratch.len() {
        if scratch[i] == ch && !is_escaped(scratch, i) {
            positions.push(i);
        }
    }
    // Pair sequentially: 0↔1, 2↔3, etc.
    let mut pair_idx = 0;
    while pair_idx + 1 < positions.len() {
        let open = positions[pair_idx];
        let close = positions[pair_idx + 1];
        if byte_idx == open {
            let column = byte_to_character_column(scratch, close);
            return Some(TextPosition::new(pos.line, column));
        }
        if byte_idx == close {
            let column = byte_to_character_column(scratch, open);
            return Some(TextPosition::new(pos.line, column));
        }
        pair_idx += 2;
    }
    None
}

fn character_column_to_byte(line: &[u8], character_column: usize) -> Option<usize> {
    if line.is_ascii() {
        return Some(character_column.min(line.len()));
    }
    let mut bi = 0;
    let mut ci = 0;
    while ci < character_column && bi < line.len() {
        bi += utf8_char_len(line[bi]);
        ci += 1;
    }
    Some(bi)
}

fn byte_to_character_column(line: &[u8], byte_idx: usize) -> usize {
    if line.is_ascii() {
        return byte_idx.min(line.len());
    }
    let mut bi = 0;
    let mut ci = 0;
    while bi < byte_idx && bi < line.len() {
        bi += utf8_char_len(line[bi]);
        ci += 1;
    }
    ci
}

// -- Byte-to-char mapping ---------------------------------------------------

/// Map byte-indexed highlights to char-indexed highlights, writing into `out`.
/// Tabs expand to 2 display entries, multi-byte UTF-8 collapses to 1 entry.
/// Clears `out` first; reuses its allocation across calls.
pub fn byte_kinds_to_char_kinds_into(raw: &[u8], byte_kinds: &[Kind], out: &mut Vec<Kind>) {
    assert_eq!(
        raw.len(),
        byte_kinds.len(),
        "byte kinds must have one entry per input byte"
    );
    out.clear();
    if raw.is_ascii() {
        // ASCII fast path: 1 byte = 1 char, tabs expand to 2 display positions
        out.reserve(raw.len());
        for (i, &b) in raw.iter().enumerate() {
            out.push(byte_kinds[i]);
            if b == b'\t' {
                out.push(byte_kinds[i]);
            }
        }
    } else {
        let mut bi = 0;
        while bi < raw.len() {
            let ht = byte_kinds[bi];
            if raw[bi] == b'\t' {
                out.push(ht);
                out.push(ht);
                bi += 1;
            } else {
                out.push(ht);
                bi += utf8_char_len(raw[bi]);
            }
        }
    }
}

/// Allocating wrapper around `byte_kinds_to_char_kinds_into`. Used in tests.
#[allow(dead_code)]
pub fn byte_kinds_to_char_kinds(raw: &[u8], byte_kinds: &[Kind]) -> Vec<Kind> {
    let mut out = Vec::with_capacity(raw.len());
    byte_kinds_to_char_kinds_into(raw, byte_kinds, &mut out);
    out
}

/// Scan one line with an internal static rule table and caller-owned output.
fn highlight_line_into_rules(
    line: &[u8],
    state: StateKind,
    rules: &RuleSet,
    user_types: &[Vec<u8>],
    out: &mut [Kind],
) -> StateKind {
    assert_eq!(
        line.len(),
        out.len(),
        "highlight output must have one slot per input byte"
    );
    let next_state = match rules.lexer_kind {
        LexerKind::Markdown => highlight_line_markdown(line, state, rules, out),
        LexerKind::Json => highlight_line_json(line, state, out),
        LexerKind::Yaml => highlight_line_yaml(line, state, out),
        LexerKind::Ini => highlight_line_ini(line, state, out),
        LexerKind::Code | LexerKind::Rust => highlight_line_code(line, state, rules, user_types, out),
        LexerKind::Bash
        | LexerKind::Go
        | LexerKind::Python
        | LexerKind::C
        | LexerKind::Html
        | LexerKind::Css
        | LexerKind::Makefile
        | LexerKind::Dockerfile
        | LexerKind::Script
        | LexerKind::TypeScript => highlight_line_specialized(line, state, rules, user_types, out),
    };
    if rules.lexer_kind == LexerKind::Rust && line.starts_with(b"#") {
        for kind in out.iter_mut() {
            if *kind == Kind::Function {
                *kind = Kind::Normal;
            }
        }
    }
    highlight_semver(line, out, rules.lexer_kind, state);
    next_state
}

/// Highlight one line into a caller-sized byte-kind buffer.
///
/// `out` must have exactly `line.len()` elements. The returned opaque state is
/// the input state for the next logical line.
pub fn highlight_line_into(
    language: crate::Language,
    state: State,
    line: &[u8],
    out: &mut [Kind],
) -> State {
    State(highlight_line_into_rules(
        line,
        state.0,
        language.rules(),
        &[],
        out,
    ))
}

/// Stateful line highlighter for streaming callers.
pub struct Highlighter {
    language: crate::Language,
    state: State,
    user_types: Vec<Vec<u8>>,
}

impl Highlighter {
    /// Start highlighting with `language` and a normal lexical state.
    pub fn new(language: crate::Language) -> Self {
        Self {
            language,
            state: State::normal(),
            user_types: Vec::new(),
        }
    }

    /// Return the selected language.
    pub const fn language(&self) -> crate::Language {
        self.language
    }

    /// Replace the selected language and reset multiline state.
    pub fn set_language(&mut self, language: crate::Language) {
        self.language = language;
        self.reset();
    }

    /// Reset the stream to the beginning of a document.
    pub fn reset(&mut self) {
        self.state = State::normal();
    }

    /// Return the current opaque lexical state.
    pub const fn state(&self) -> State {
        self.state
    }

    /// Set the state used for the next line.
    ///
    /// This is useful for editors that resume from a cached line checkpoint.
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Supply optional application-defined type names for identifier coloring.
    /// The scanner copies the names only when they change.
    pub fn set_user_types(&mut self, user_types: &[Vec<u8>]) {
        if self.user_types != user_types {
            self.user_types.clear();
            self.user_types.extend_from_slice(user_types);
        }
    }

    /// Highlight one line, reusing the caller's scratch allocation.
    ///
    /// Once `scratch` has capacity for the largest line, repeated calls do not
    /// allocate; callers can reserve that capacity once when opening a file.
    pub fn highlight_into<'a>(&mut self, line: &[u8], scratch: &'a mut Vec<Kind>) -> &'a [Kind] {
        scratch.resize(line.len(), Kind::Normal);
        // `Vec::resize` preserves existing elements when the next line has
        // the same length; clear them so a previous comment/string cannot
        // leak into this line's semantic runs.
        scratch.fill(Kind::Normal);
        self.state = State(highlight_line_into_rules(
            line,
            self.state.0,
            self.language.rules(),
            &self.user_types,
            scratch,
        ));
        scratch
    }
}

/// A contiguous byte range with one semantic kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Iterate over sorted, non-overlapping runs of equal byte kinds.
pub fn runs(kinds: &[Kind]) -> impl Iterator<Item = Run> + '_ {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= kinds.len() {
            return None;
        }
        let kind = kinds[start];
        let run_start = start;
        start += 1;
        while start < kinds.len() && kinds[start] == kind {
            start += 1;
        }
        Some(Run {
            start: run_start,
            end: start,
            kind,
        })
    })
}

// -- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Language;
    use crate::languages::*;

    fn hl_types(line: &[u8], rules: &RuleSet) -> Vec<Kind> {
        highlight_line(line, StateKind::Normal, rules).0
    }

    // -- Basic highlighting -------------------------------------------------

    #[test]
    fn test_line_comment() {
        let hl = hl_types(b"let x = 1; // comment", &RUST_RULES);
        // The "// comment" part should all be Comment
        assert_eq!(hl[11], Kind::Comment);
        assert_eq!(hl[20], Kind::Comment);
    }

    #[test]
    fn test_keyword() {
        let hl = hl_types(b"fn main() {}", &RUST_RULES);
        assert_eq!(hl[0], Kind::Keyword); // 'f'
        assert_eq!(hl[1], Kind::Keyword); // 'n'
        assert_eq!(hl[2], Kind::Normal); // ' '
    }

    #[test]
    fn test_type() {
        let hl = hl_types(b"let x: usize = 0;", &RUST_RULES);
        // "usize" starts at index 7
        assert_eq!(hl[7], Kind::Type);
        assert_eq!(hl[11], Kind::Type);
    }

    #[test]
    fn test_string() {
        let hl = hl_types(b"let s = \"hello\";", &RUST_RULES);
        // "hello" starts at index 8, ends at 14
        assert_eq!(hl[8], Kind::String); // opening "
        assert_eq!(hl[13], Kind::String); // closing "
    }

    #[test]
    fn test_number() {
        let hl = hl_types(b"let x = 42;", &RUST_RULES);
        assert_eq!(hl[8], Kind::Number); // '4'
        assert_eq!(hl[9], Kind::Number); // '2'
    }

    #[test]
    fn test_normal_text() {
        let hl = hl_types(b"hello", &RUST_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Normal));
    }

    // -- Block comments -----------------------------------------------------

    #[test]
    fn test_block_comment_single_line() {
        let hl = hl_types(b"x /* comment */ y", &RUST_RULES);
        assert_eq!(hl[0], Kind::Normal); // 'x'
        assert_eq!(hl[2], Kind::Comment); // '/'
        assert_eq!(hl[13], Kind::Comment); // '/'
        assert_eq!(hl[16], Kind::Normal); // 'y'
    }

    #[test]
    fn test_block_comment_multiline() {
        let (hl1, state) = highlight_line(b"/* start", StateKind::Normal, &RUST_RULES);
        assert!(hl1.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state, StateKind::BlockComment);

        let (hl2, state2) = highlight_line(b"end */", StateKind::BlockComment, &RUST_RULES);
        assert!(hl2.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state2, StateKind::Normal);
    }

    #[test]
    fn test_rust_nested_block_comments() {
        let (hl1, state1) = highlight_line(b"/* outer /* inner", StateKind::Normal, &RUST_RULES);
        assert!(hl1.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state1, StateKind::RustBlockComment(2));

        let (hl2, state2) = highlight_line(b"still */", state1, &RUST_RULES);
        assert!(hl2.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state2, StateKind::BlockComment);

        let (hl3, state3) = highlight_line(b"end */ let", state2, &RUST_RULES);
        assert_eq!(state3, StateKind::Normal);
        assert_eq!(hl3[7], Kind::Keyword);
    }

    #[test]
    fn test_rust_lifetimes_are_not_character_literals() {
        let hl = hl_types(b"'a 'static 'x'", &RUST_RULES);
        assert_eq!(hl[0], Kind::Type);
        assert_eq!(hl[1], Kind::Type);
        assert_eq!(hl[3], Kind::Type);
        assert_eq!(hl[9], Kind::Type);
        assert_eq!(hl[11], Kind::String);
        assert_eq!(hl[12], Kind::String);
        assert_eq!(hl[13], Kind::String);
    }

    #[test]
    fn test_rust_raw_strings_and_hashes() {
        let line = b"r#\"1.2.3 \\\" quote\"#";
        let (hl, state) = highlight_line(line, StateKind::Normal, &RUST_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));
        assert_eq!(state, StateKind::Normal);

        let (first, state1) = highlight_line(b"br###\"start", StateKind::Normal, &RUST_RULES);
        assert!(first.iter().all(|&h| h == Kind::String));
        assert_eq!(state1, StateKind::RustRawString(3));
        let (second, state2) = highlight_line(b"1.2.3\"###", state1, &RUST_RULES);
        assert!(second.iter().all(|&h| h == Kind::String));
        assert_eq!(state2, StateKind::Normal);
    }

    // -- Multiline strings --------------------------------------------------

    #[test]
    fn test_python_triple_quote() {
        let (hl1, state) = highlight_line(b"s = \"\"\"hello", StateKind::Normal, &PYTHON_RULES);
        assert_eq!(hl1[4], Kind::String);
        assert!(matches!(state, StateKind::MultiLineString(_)));

        let (hl2, state2) = highlight_line(b"world\"\"\"", state, &PYTHON_RULES);
        assert!(hl2.iter().all(|&h| h == Kind::String));
        assert_eq!(state2, StateKind::Normal);
    }

    #[test]
    fn test_go_backtick_string() {
        let (hl1, state) = highlight_line(b"s := `hello", StateKind::Normal, &GO_RULES);
        assert_eq!(hl1[5], Kind::String);
        assert!(matches!(state, StateKind::MultiLineString(_)));

        let (hl2, state2) = highlight_line(b"world`", state, &GO_RULES);
        assert!(hl2.iter().all(|&h| h == Kind::String));
        assert_eq!(state2, StateKind::Normal);
    }

    // -- Escape handling in strings -----------------------------------------

    #[test]
    fn test_string_escape() {
        let hl = hl_types(b"\"he\\\"llo\"", &RUST_RULES);
        // All should be String since \" is escaped
        assert!(hl.iter().all(|&h| h == Kind::String));
    }

    // -- Keyword boundary ---------------------------------------------------

    #[test]
    fn test_keyword_not_in_identifier() {
        let hl = hl_types(b"format", &RUST_RULES);
        // "for" should not match inside "format"
        assert!(hl.iter().all(|&h| h == Kind::Normal));
    }

    // -- Function call highlighting -----------------------------------------

    #[test]
    fn test_function_call() {
        let hl = hl_types(b"foo(x)", &RUST_RULES);
        assert_eq!(hl[0], Kind::Function); // f
        assert_eq!(hl[1], Kind::Function); // o
        assert_eq!(hl[2], Kind::Function); // o
        assert_eq!(hl[3], Kind::Bracket); // (
    }

    #[test]
    fn test_method_call() {
        let hl = hl_types(b"x.method(y)", &RUST_RULES);
        assert_eq!(hl[0], Kind::Normal); // x
        assert_eq!(hl[2], Kind::Function); // m
        assert_eq!(hl[7], Kind::Function); // d
        assert_eq!(hl[8], Kind::Bracket); // (
    }

    #[test]
    fn test_keyword_not_function() {
        // "if(" should still be keyword, not function
        let hl = hl_types(b"if(x)", &RUST_RULES);
        assert_eq!(hl[0], Kind::Keyword); // i
        assert_eq!(hl[1], Kind::Keyword); // f
    }

    // -- Constant highlighting ----------------------------------------------

    #[test]
    fn test_upper_snake_constant() {
        let hl = hl_types(b"let x = MAX_SIZE;", &RUST_RULES);
        assert_eq!(hl[8], Kind::Constant); // M
        assert_eq!(hl[15], Kind::Constant); // E
    }

    #[test]
    fn test_single_upper_char_not_constant() {
        // Single uppercase letter shouldn't be constant (need >=2 chars)
        let hl = hl_types(b"let X = 1;", &RUST_RULES);
        assert_eq!(hl[4], Kind::Normal); // X
    }

    #[test]
    fn test_mixed_case_not_constant() {
        let hl = hl_types(b"let MyVar = 1;", &RUST_RULES);
        assert_eq!(hl[4], Kind::Normal); // M
    }

    // -- Macro highlighting -------------------------------------------------

    #[test]
    fn test_rust_bang_macro() {
        let hl = hl_types(b"println!(\"hi\")", &RUST_RULES);
        assert_eq!(hl[0], Kind::Macro); // p
        assert_eq!(hl[6], Kind::Macro); // n
        assert_eq!(hl[7], Kind::Macro); // !
        assert_eq!(hl[8], Kind::Bracket); // (
    }

    #[test]
    fn test_bang_not_macro_in_python() {
        // Python doesn't have bang macros, so foo! is not a macro
        let hl = hl_types(b"foo!(x)", &PYTHON_RULES);
        assert_eq!(hl[0], Kind::Normal); // f
        assert_eq!(hl[2], Kind::Normal); // o
    }

    #[test]
    fn test_not_equal_not_macro() {
        // foo != bar — the != should not be treated as a macro invocation
        let hl = hl_types(b"foo != bar", &RUST_RULES);
        assert_eq!(hl[0], Kind::Normal); // f
        assert_eq!(hl[4], Kind::Operator); // !
    }

    // -- byte_kinds_to_char_kinds -------------------------------------------------

    #[test]
    fn test_byte_to_char_ascii() {
        let raw = b"hello";
        let byte_kinds = vec![Kind::Keyword; 5];
        let char_hl = byte_kinds_to_char_kinds(raw, &byte_kinds);
        assert_eq!(char_hl.len(), 5);
        assert!(char_hl.iter().all(|&h| h == Kind::Keyword));
    }

    #[test]
    fn test_byte_to_char_tab() {
        let raw = b"\thello";
        let byte_kinds = vec![Kind::Normal; 6];
        let char_hl = byte_kinds_to_char_kinds(raw, &byte_kinds);
        // Tab expands to 2 entries
        assert_eq!(char_hl.len(), 7);
    }

    #[test]
    fn test_byte_to_char_utf8() {
        let raw = "héllo".as_bytes(); // é is 2 bytes
        let byte_kinds = vec![Kind::Normal; raw.len()];
        let char_hl = byte_kinds_to_char_kinds(raw, &byte_kinds);
        // 5 chars: h, é, l, l, o
        assert_eq!(char_hl.len(), 5);
    }

    // -- Language lookup -----------------------------------------------------

    #[test]
    fn test_language_rules_cover_builtins() {
        let languages = [
            Language::Rust,
            Language::Python,
            Language::Go,
            Language::TypeScript,
            Language::JavaScript,
            Language::Bash,
            Language::C,
            Language::Toml,
            Language::Json,
            Language::Yaml,
            Language::Makefile,
            Language::Html,
            Language::Css,
            Language::Dockerfile,
            Language::Ini,
            Language::Markdown,
            Language::Xsh,
        ];
        for language in languages {
            assert_eq!(Language::from_name(language.name()), Some(language));
            let _ = language.rules();
        }
    }

    #[test]
    fn test_language_unknown() {
        assert!(Language::from_name("Unknown").is_none());
    }

    // -- INI/Config ---------------------------------------------------------

    #[test]
    fn test_ini_config() {
        // Section header
        let hl = hl_types(b"[section]", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword); // [
        assert_eq!(hl[4], Kind::Keyword); // i
        assert_eq!(hl[8], Kind::Keyword); // ]

        // Key = value
        let hl = hl_types(b"key = value", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword); // k
        assert_eq!(hl[2], Kind::Keyword); // y
        assert_eq!(hl[4], Kind::Normal); // =
        assert_eq!(hl[6], Kind::Normal); // v (unquoted string)

        // Quoted string value
        let hl = hl_types(b"name = \"hello\"", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword); // n
        assert_eq!(hl[7], Kind::String); // "
        assert_eq!(hl[12], Kind::String); // o
        assert_eq!(hl[13], Kind::String); // "

        // Single-quoted string value
        let hl = hl_types(b"name = 'hello'", &INI_RULES);
        assert_eq!(hl[7], Kind::String);
        assert_eq!(hl[13], Kind::String);

        // Semicolon comment
        let hl = hl_types(b"; this is a comment", &INI_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Comment));

        // Hash comment
        let hl = hl_types(b"# this is a comment", &INI_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Comment));

        // Indented comment
        let hl = hl_types(b"  ; indented comment", &INI_RULES);
        assert_eq!(hl[0], Kind::Normal);
        assert_eq!(hl[2], Kind::Comment);
        assert_eq!(hl[19], Kind::Comment);

        // Number value
        let hl = hl_types(b"port = 8080", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword); // p
        assert_eq!(hl[7], Kind::Number); // 8
        assert_eq!(hl[10], Kind::Number); // 0

        // Boolean type
        let hl = hl_types(b"enabled = true", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword);
        assert_eq!(hl[10], Kind::Type); // t
        assert_eq!(hl[13], Kind::Type); // e

        // Case-insensitive boolean
        let hl = hl_types(b"flag = TRUE", &INI_RULES);
        assert_eq!(hl[7], Kind::Type);

        let hl = hl_types(b"flag = Yes", &INI_RULES);
        assert_eq!(hl[7], Kind::Type);

        let hl = hl_types(b"debug = off", &INI_RULES);
        assert_eq!(hl[8], Kind::Type);

        // Inline comment after value
        let hl = hl_types(b"key = value ; comment", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword);
        assert_eq!(hl[6], Kind::Normal); // v
        assert_eq!(hl[12], Kind::Comment); // ;
        assert_eq!(hl[20], Kind::Comment);

        // Section header with inline comment
        let hl = hl_types(b"[section] ; comment", &INI_RULES);
        assert_eq!(hl[0], Kind::Keyword);
        assert_eq!(hl[8], Kind::Keyword);
        assert_eq!(hl[10], Kind::Comment);
    }

    // -- Python specifics ---------------------------------------------------

    #[test]
    fn test_python_hash_comment() {
        let hl = hl_types(b"x = 1 # comment", &PYTHON_RULES);
        assert_eq!(hl[6], Kind::Comment);
    }

    // -- Empty line ---------------------------------------------------------

    #[test]
    fn test_empty_line() {
        let (hl, state) = highlight_line(b"", StateKind::Normal, &RUST_RULES);
        assert!(hl.is_empty());
        assert_eq!(state, StateKind::Normal);
    }

    #[test]
    fn test_empty_line_in_block_comment() {
        let (hl, state) = highlight_line(b"", StateKind::BlockComment, &RUST_RULES);
        assert!(hl.is_empty());
        assert_eq!(state, StateKind::BlockComment);
    }

    // -- HTML block comments ------------------------------------------------

    #[test]
    fn test_html_comment() {
        let (hl, state) = highlight_line(b"<!-- comment -->", StateKind::Normal, &HTML_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state, StateKind::Normal);
    }

    #[test]
    fn test_html_multiline_comment() {
        let (hl1, state1) = highlight_line(b"<!-- start", StateKind::Normal, &HTML_RULES);
        assert!(hl1.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state1, StateKind::BlockComment);

        let (hl2, state2) = highlight_line(b"end -->", StateKind::BlockComment, &HTML_RULES);
        assert!(hl2.iter().all(|&h| h == Kind::Comment));
        assert_eq!(state2, StateKind::Normal);
    }

    // -- Dockerfile keywords ------------------------------------------------

    #[test]
    fn test_dockerfile_keywords() {
        let hl = hl_types(b"FROM ubuntu:latest", &DOCKERFILE_RULES);
        assert_eq!(hl[0], Kind::Keyword); // F
        assert_eq!(hl[3], Kind::Keyword); // M
    }

    // -- JSON ---------------------------------------------------------------

    #[test]
    fn test_json_no_comments() {
        let hl = hl_types(b"{\"key\": true}", &JSON_RULES);
        assert_eq!(hl[1], Kind::Keyword); // key is yellow
        assert_eq!(hl[8], Kind::Type); // 't' of true
    }

    // -- Number edge cases --------------------------------------------------

    #[test]
    fn test_hex_number() {
        let hl = hl_types(b"let x = 0xff;", &RUST_RULES);
        assert_eq!(hl[8], Kind::Number); // '0'
        assert_eq!(hl[9], Kind::Number); // 'x'
        assert_eq!(hl[11], Kind::Number); // 'f'
    }

    #[test]
    fn test_float_number() {
        let hl = hl_types(b"let x = 3.14;", &RUST_RULES);
        assert_eq!(hl[8], Kind::Number); // '3'
        assert_eq!(hl[9], Kind::Number); // '.'
        assert_eq!(hl[10], Kind::Number); // '1'
    }

    #[test]
    fn test_number_does_not_swallow_identifier() {
        let hl = hl_types(b"123foobar", &RUST_RULES);
        assert!(hl[..3].iter().all(|&h| h == Kind::Number));
        assert!(hl[3..].iter().all(|&h| h == Kind::Normal));

        let hl = hl_types(b"1..2", &RUST_RULES);
        assert_eq!(hl[0], Kind::Number);
        assert_eq!(hl[1], Kind::Normal);
        assert_eq!(hl[2], Kind::Normal);
        assert_eq!(hl[3], Kind::Number);
    }

    #[test]
    fn test_number_radices_exponents_separators_and_suffixes() {
        for line in [
            b"0xFF_u32".as_slice(),
            b"0b1010u8".as_slice(),
            b"0o755usize".as_slice(),
            b"1_000_000".as_slice(),
            b"1.5e-2f64".as_slice(),
            b"100ms".as_slice(),
        ] {
            let hl = hl_types(line, &RUST_RULES);
            assert!(hl.iter().all(|&h| h == Kind::Number), "{line:?}");
        }

        let malformed = hl_types(b"0xZZ", &RUST_RULES);
        assert_eq!(malformed[0], Kind::Number);
        assert!(malformed[1..].iter().all(|&h| h == Kind::Normal));
    }

    // -- Semver highlighting ------------------------------------------------

    /// Helper: highlight multiple lines and return all per-byte highlights.
    fn hl_multiline(lines: &[&[u8]], rules: &RuleSet) -> Vec<Vec<Kind>> {
        let mut state = StateKind::Normal;
        let mut result = Vec::new();
        for line in lines {
            let (hl, next) = highlight_line(line, state, rules);
            result.push(hl);
            state = next;
        }
        result
    }

    /// Helper: assert a byte range is a specific Kind.
    fn assert_range(hl: &[Kind], range: std::ops::Range<usize>, expected: Kind, label: &str) {
        for i in range {
            assert_eq!(
                hl[i], expected,
                "{}: byte {} expected {:?}",
                label, i, expected
            );
        }
    }

    #[test]
    fn test_semver_in_cargo_toml() {
        // Realistic Cargo.toml snippet
        let lines: &[&[u8]] = &[
            b"[package]",
            b"name = \"my-crate\"",
            b"version = \"0.3.5\"",
            b"edition = \"2021\"",
            b"",
            b"[dependencies]",
            b"serde = \"1.0.197\"",
            b"tokio = { version = \"1.36.0\", features = [\"full\"] }",
            b"regex = \"1.10.3\"",
        ];
        let hls = hl_multiline(lines, &TOML_RULES);
        // line 2: version = "0.3.5" — 0.3.5 at bytes 11..16
        assert_range(&hls[2], 11..16, Kind::Type, "version value");
        // line 6: serde = "1.0.197" — 1.0.197 at bytes 9..16
        assert_range(&hls[6], 9..16, Kind::Type, "serde version");
        // line 7: "1.36.0" — 1.36.0 inside the string
        let l7 = &hls[7];
        let s = b"tokio = { version = \"1.36.0\", features = [\"full\"] }";
        let ver_start = s.windows(5).position(|w| w == b"1.36.").unwrap();
        assert_range(l7, ver_start..ver_start + 6, Kind::Type, "tokio version");
        // line 3: "2021" is NOT semver (only one component)
        assert_ne!(hls[3][11], Kind::Type);
    }

    #[test]
    fn test_semver_in_rust_code() {
        let lines: &[&[u8]] = &[
            b"// Released v2.0.0-beta.1",
            b"const VERSION: &str = \"1.0.0+build.42\";",
            b"let v = 1;",
            b"let x = abc1.2.3;",
            b"println!(\"upgrade to v0.9.0 or 1.2.3x\");",
        ];
        let hls = hl_multiline(lines, &RUST_RULES);
        // line 0: comment — semver should NOT override comment
        assert_range(&hls[0], 0..25, Kind::Comment, "comment line");
        // line 1: "1.0.0+build.42" inside string — semver SHOULD override
        let l1 = &hls[1];
        // const VERSION: &str = "1.0.0+build.42"; — version at byte 23
        let ver_start = b"const VERSION: &str = \"".len();
        assert_range(
            l1,
            ver_start..ver_start + 14,
            Kind::Type,
            "version in string",
        );
        // line 2: "v = 1" — bare v is not semver
        assert_ne!(hls[2][4], Kind::Type);
        // line 3: "abc1.2.3" — preceded by alpha, not semver
        assert_ne!(hls[3][12], Kind::Type);
        // line 4: "v0.9.0" in string should be semver, "1.2.3x" should not
        let l4 = &hls[4];
        let s4 = b"println!(\"upgrade to v0.9.0 or 1.2.3x\");";
        let v_start = s4.windows(6).position(|w| w == b"v0.9.0").unwrap();
        assert_range(l4, v_start..v_start + 6, Kind::Type, "v0.9.0 in string");
        // 1.2.3x should not be Type (trailing x)
        let bad_start = s4.windows(5).position(|w| w == b"1.2.3").unwrap();
        assert_ne!(l4[bad_start], Kind::Type);
    }

    #[test]
    fn test_semver_raw_string_detection_respects_string_boundaries() {
        let line = b"\"r\\\"x\\\"\"; r\"1.2.3\"; 4.5.6";
        let hl = hl_types(line, &RUST_RULES);

        let ordinary = line
            .windows(4)
            .position(|window| window == b"r\\\"x")
            .unwrap();
        assert!(
            hl[ordinary..ordinary + 4]
                .iter()
                .all(|&kind| kind == Kind::String)
        );

        let raw = line
            .windows(5)
            .position(|window| window == b"1.2.3")
            .unwrap();
        assert!(hl[raw..raw + 5].iter().all(|&kind| kind == Kind::String));

        let trailing = line
            .windows(5)
            .position(|window| window == b"4.5.6")
            .unwrap();
        assert!(
            hl[trailing..trailing + 5]
                .iter()
                .all(|&kind| kind == Kind::Type)
        );
    }

    // -- Bracket highlighting -----------------------------------------------

    #[test]
    fn test_brackets_in_rust_function() {
        // Brackets should be purple, but not inside strings or comments
        let lines: &[&[u8]] = &[
            b"fn process(items: Vec<u32>) {",
            b"    let s = \"(not a bracket)\";",
            b"    // {also not a bracket}",
            b"    if items[0] > 0 {",
            b"        println!(\"ok\");",
            b"    }",
            b"}",
        ];
        let hls = hl_multiline(lines, &RUST_RULES);
        // line 0: ( at 10, ) at 26, { at 28
        assert_eq!(hls[0][10], Kind::Bracket); // (
        assert_eq!(hls[0][26], Kind::Bracket); // )
        assert_eq!(hls[0][28], Kind::Bracket); // { at end
        // line 1: ( and ) inside string should be String, not Bracket
        let l1 = &hls[1];
        // The string starts at the " and everything inside is String
        let paren_pos = b"    let s = \"(not a bracket)\";"
            .iter()
            .position(|&b| b == b'(')
            .unwrap();
        assert_eq!(l1[paren_pos], Kind::String);
        // line 2: { inside comment should be Comment (after leading whitespace)
        let comment_start = b"    ".len();
        assert_range(
            &hls[2],
            comment_start..hls[2].len(),
            Kind::Comment,
            "comment with brackets",
        );
        // line 3: [ at some position, { at end
        let l3 = &hls[3];
        let bracket_pos = b"    if items[0] > 0 {"
            .iter()
            .position(|&b| b == b'[')
            .unwrap();
        assert_eq!(l3[bracket_pos], Kind::Bracket);
        // line 6: } is bracket
        assert_eq!(hls[6][0], Kind::Bracket);
    }

    // -- Markdown highlighting ----------------------------------------------

    #[test]
    fn test_markdown_document() {
        let lines: &[&[u8]] = &[
            b"# My Project",
            b"",
            b"Some text with **bold** and *italic* words.",
            b"",
            b"> A blockquote with `inline code`",
            b"",
            b"- first item",
            b"- second item",
            b"1. ordered item",
            b"",
            b"---",
            b"",
            b"```rust",
            b"fn main() {}",
            b"```",
            b"",
            b"<!-- a comment -->",
        ];
        let hls = hl_multiline(lines, &MARKDOWN_RULES);

        // line 0: header — all Keyword
        assert!(
            hls[0].iter().all(|&h| h == Kind::Keyword),
            "header should be all Keyword"
        );

        // line 2: **bold** → Keyword, *italic* → Type, rest Normal
        let l2 = &hls[2];
        let bold_start = b"Some text with ".len();
        assert_range(l2, bold_start..bold_start + 8, Kind::Keyword, "bold");
        let italic_start = bold_start + 8 + " and ".len();
        assert_range(l2, italic_start..italic_start + 8, Kind::Type, "italic");

        // line 4: > marker is Comment, `inline code` is String
        assert_eq!(hls[4][0], Kind::Comment); // >
        let backtick = b"> A blockquote with ".len();
        assert_range(
            &hls[4],
            backtick..backtick + 13,
            Kind::String,
            "inline code",
        );

        // line 6-7: list markers — "- " is Number
        assert_eq!(hls[6][0], Kind::Number); // -
        assert_eq!(hls[6][1], Kind::Number); // space
        assert_eq!(hls[6][2], Kind::Normal); // f
        assert_eq!(hls[7][0], Kind::Number); // -

        // line 8: ordered list — "1. " is Number
        assert_range(&hls[8], 0..3, Kind::Number, "ordered marker");
        assert_eq!(hls[8][3], Kind::Normal);

        // line 10: horizontal rule — all Comment
        assert!(
            hls[10].iter().all(|&h| h == Kind::Comment),
            "hr should be Comment"
        );

        // line 12: fenced code open — all String, state enters FencedCodeBlock
        assert!(hls[12].iter().all(|&h| h == Kind::String), "fence open");
        // line 13: inside fenced block — all String
        assert!(hls[13].iter().all(|&h| h == Kind::String), "fenced content");
        // line 14: fence close — all String
        assert!(hls[14].iter().all(|&h| h == Kind::String), "fence close");

        // line 16: HTML comment — all Comment
        assert!(hls[16].iter().all(|&h| h == Kind::Comment), "html comment");
    }

    #[test]
    fn test_markdown_multiline_html_comment() {
        let lines: &[&[u8]] = &[
            b"before",
            b"<!-- start of",
            b"multiline comment",
            b"end --> after",
        ];
        let hls = hl_multiline(lines, &MARKDOWN_RULES);
        assert!(hls[0].iter().all(|&h| h == Kind::Normal), "before");
        assert!(hls[1].iter().all(|&h| h == Kind::Comment), "comment start");
        assert!(hls[2].iter().all(|&h| h == Kind::Comment), "comment middle");
        // line 3: "end -->" is comment, " after" is normal
        let close_end = b"end -->".len();
        assert_range(&hls[3], 0..close_end, Kind::Comment, "comment end");
    }

    // -- JSON document ------------------------------------------------------

    #[test]
    fn test_json_package_json() {
        let lines: &[&[u8]] = &[
            b"{",
            b"  \"name\": \"my-app\",",
            b"  \"version\": \"2.1.0\",",
            b"  \"private\": true,",
            b"  \"dependencies\": {",
            b"    \"react\": \"18.2.0\",",
            b"    \"next\": \"14.1.3\"",
            b"  },",
            b"  \"count\": 42,",
            b"  \"tags\": [\"web\", \"frontend\"],",
            b"  \"nullable\": null",
            b"}",
        ];
        let hls = hl_multiline(lines, &JSON_RULES);

        // line 0: { is Bracket
        assert_eq!(hls[0][0], Kind::Bracket);
        // line 1: "name" is Keyword (key), "my-app" is String (value)
        assert_range(&hls[1], 2..8, Kind::Keyword, "name key");
        assert_range(&hls[1], 10..18, Kind::String, "my-app value");
        // line 2: "version" is Keyword, "2.1.0" gets semver override
        assert_range(&hls[2], 2..11, Kind::Keyword, "version key");
        let ver_start = b"  \"version\": \"".len();
        assert_range(
            &hls[2],
            ver_start..ver_start + 5,
            Kind::Type,
            "semver 2.1.0",
        );
        // line 3: true is Type
        let true_start = b"  \"private\": ".len();
        assert_range(&hls[3], true_start..true_start + 4, Kind::Type, "true");
        // line 4: "dependencies" key, { bracket
        assert_eq!(hls[4][2], Kind::Keyword); // "
        let brace = hls[4].len() - 1;
        assert_eq!(hls[4][brace], Kind::Bracket);
        // line 5: nested key "react", semver value "18.2.0"
        assert_eq!(hls[5][4], Kind::Keyword);
        let react_ver = b"    \"react\": \"".len();
        assert_range(
            &hls[5],
            react_ver..react_ver + 6,
            Kind::Type,
            "react semver",
        );
        // line 8: 42 is Number
        let num_start = b"  \"count\": ".len();
        assert_range(&hls[8], num_start..num_start + 2, Kind::Number, "42");
        // line 9: [ and ] are brackets, string values
        assert_eq!(hls[9][b"  \"tags\": ".len()], Kind::Bracket); // [
        // line 10: null is Type
        let null_start = b"  \"nullable\": ".len();
        assert_range(&hls[10], null_start..null_start + 4, Kind::Type, "null");
        // line 11: } is Bracket
        assert_eq!(hls[11][0], Kind::Bracket);
    }

    // -- YAML document ------------------------------------------------------

    #[test]
    fn test_yaml_config() {
        let lines: &[&[u8]] = &[
            b"name: my-service",
            b"version: 1.5.0",
            b"debug: false",
            b"port: 8080",
            b"host: \"localhost\"",
            b"database:",
            b"  url: \"postgres://localhost/db\"",
            b"  pool_size: 10",
            b"defaults: &defaults",
            b"  timeout: 30",
            b"production:",
            b"  <<: *defaults",
            b"  debug: false",
            b"tags: # inline comment",
            b"  - web",
            b"  - api",
        ];
        let hls = hl_multiline(lines, &YAML_RULES);

        // line 0: "name" is Keyword, "my-service" is Normal (unquoted)
        assert_range(&hls[0], 0..4, Kind::Keyword, "name key");
        assert_eq!(hls[0][6], Kind::Normal);
        // line 1: "version" Keyword, "1.5.0" semver
        assert_range(&hls[1], 0..7, Kind::Keyword, "version key");
        assert_range(&hls[1], 9..14, Kind::Type, "semver 1.5.0");
        // line 2: "false" is Type
        assert_range(&hls[2], 7..12, Kind::Type, "false");
        // line 3: 8080 is Number
        assert_range(&hls[3], 6..10, Kind::Number, "8080");
        // line 4: "localhost" is String (quoted)
        assert_range(&hls[4], 6..17, Kind::String, "quoted value");
        // line 5: "database" is Keyword, no value
        assert_range(&hls[5], 0..8, Kind::Keyword, "database key");
        // line 6: nested key "url", quoted string value
        assert_range(&hls[6], 2..5, Kind::Keyword, "url key");
        assert_eq!(hls[6][7], Kind::String);
        // line 7: "pool_size" key, 10 number
        assert_range(&hls[7], 2..11, Kind::Keyword, "pool_size key");
        assert_range(&hls[7], 13..15, Kind::Number, "10");
        // line 8: "defaults" key, &defaults anchor
        assert_range(&hls[8], 0..8, Kind::Keyword, "defaults key");
        // line 11: *defaults alias
        let l11 = &hls[11];
        let alias_start = b"  <<: ".len();
        assert_eq!(l11[alias_start], Kind::Type); // *
        // line 13: key then # comment
        assert_range(&hls[13], 0..4, Kind::Keyword, "tags key");
        let comment_start = b"tags: ".len();
        assert_range(
            &hls[13],
            comment_start..hls[13].len(),
            Kind::Comment,
            "inline comment",
        );
    }

    // -- Bracket matching ---------------------------------------------------

    #[test]
    fn test_bracket_matching_in_function() {
        let lines: Vec<Vec<u8>> = vec![
            b"fn process(items: &[u32]) -> Result<(), Error> {".to_vec(),
            b"    if items.is_empty() {".to_vec(),
            b"        return Err(Error::new());".to_vec(),
            b"    }".to_vec(),
            b"    for item in items.iter() {".to_vec(),
            b"        println!(\"{}\", item);".to_vec(),
            b"    }".to_vec(),
            b"    Ok(())".to_vec(),
            b"}".to_vec(),
        ];
        let line_count = lines.len();
        let get = |i: usize| lines[i].clone();
        let mut scratch = Vec::new();

        // Opening { on line 0 column 48 → closing } on line 8 column 0
        let open_brace = lines[0].iter().rposition(|&b| b == b'{').unwrap();
        let result = find_bracket_match(
            TextPosition::new(0, open_brace),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(TextPosition::new(8, 0)));

        // Closing } on line 8 → back to opening { on line 0
        let result = find_bracket_match(
            TextPosition::new(8, 0),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(TextPosition::new(0, open_brace)));

        // Inner if { on line 1 → } on line 3
        let if_brace = lines[1].iter().rposition(|&b| b == b'{').unwrap();
        let result = find_bracket_match(
            TextPosition::new(1, if_brace),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(TextPosition::new(3, 4)));

        // ( on line 0 column 10 → ) matching
        let result = find_bracket_match(
            TextPosition::new(0, 10),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(TextPosition::new(0, 24)));

        // Nested (()) on line 7: Ok(()) — outer ( matches outer )
        let ok_paren = lines[7].iter().position(|&b| b == b'(').unwrap();
        let result = find_bracket_match(
            TextPosition::new(7, ok_paren),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(TextPosition::new(7, ok_paren + 3)));

        // Cursor on non-bracket char → None
        let result = find_bracket_match(
            TextPosition::new(0, 0),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, None);

        // Unmatched: if we only pass first line, { has no match
        let result = find_bracket_match(
            TextPosition::new(0, open_brace),
            &mut |i, b| *b = get(i),
            &mut scratch,
            1,
        );
        assert_eq!(result, None);
    }

    // -- Quote matching -----------------------------------------------------

    #[test]
    fn test_quote_match_double_basic() {
        let line = b"\"hello\"";
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        // Cursor on opening " → match closing
        assert_eq!(
            find_quote_match(TextPosition::new(0, 0), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 6))
        );
        // Cursor on closing " → match opening
        assert_eq!(
            find_quote_match(TextPosition::new(0, 6), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 0))
        );
    }

    #[test]
    fn test_quote_match_single_basic() {
        let line = b"'hello'";
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        assert_eq!(
            find_quote_match(TextPosition::new(0, 0), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 6))
        );
        assert_eq!(
            find_quote_match(TextPosition::new(0, 6), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 0))
        );
    }

    #[test]
    fn test_quote_match_escaped_skipped() {
        // "\"foo\"" — bytes: " \ " f o o \ " " (9 bytes)
        // Unescaped quotes at byte 0 and 8, escaped at 2 and 7
        let line = br#""\"foo\"""#;
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        assert_eq!(
            find_quote_match(TextPosition::new(0, 0), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 8))
        );
        assert_eq!(
            find_quote_match(TextPosition::new(0, 8), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 0))
        );
    }

    #[test]
    fn test_quote_match_multiple_pairs() {
        let line = br#""a" "b""#;
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        // First pair: 0↔2
        assert_eq!(
            find_quote_match(TextPosition::new(0, 0), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 2))
        );
        assert_eq!(
            find_quote_match(TextPosition::new(0, 2), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 0))
        );
        // Second pair: 4↔6
        assert_eq!(
            find_quote_match(TextPosition::new(0, 4), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 6))
        );
        assert_eq!(
            find_quote_match(TextPosition::new(0, 6), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 4))
        );
    }

    #[test]
    fn test_quote_match_unmatched() {
        // Odd number of quotes → last one has no pair
        let line = br#""a" ""#;
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        assert_eq!(
            find_quote_match(TextPosition::new(0, 4), &mut get, &mut scratch, 1),
            None
        );
    }

    #[test]
    fn test_quote_match_not_on_quote() {
        let line = b"hello";
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        assert_eq!(
            find_quote_match(TextPosition::new(0, 2), &mut get, &mut scratch, 1),
            None
        );
    }

    #[test]
    fn test_quote_match_escaped_under_cursor() {
        // Cursor on an escaped quote → no match
        let line = br#""\""#;
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        // byte 1 is \, byte 2 is escaped "
        assert_eq!(
            find_quote_match(TextPosition::new(0, 2), &mut get, &mut scratch, 1),
            None
        );
    }

    #[test]
    fn test_quote_match_double_backslash() {
        // \\" — two backslashes then quote; even backslashes = not escaped
        let line = br##""\\""##;
        let mut scratch = Vec::new();
        let mut get = |_: usize, b: &mut Vec<u8>| {
            b.clear();
            b.extend_from_slice(line);
        };
        // bytes: " \ \ " — positions 0 and 3 are unescaped quotes
        assert_eq!(
            find_quote_match(TextPosition::new(0, 0), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 3))
        );
        assert_eq!(
            find_quote_match(TextPosition::new(0, 3), &mut get, &mut scratch, 1),
            Some(TextPosition::new(0, 0))
        );
    }

    // -- INI section edge cases -----------------------------------------------

    #[test]
    fn test_ini_empty_value() {
        let hl = hl_types(b"key =", &INI_RULES);
        assert_range(&hl, 0..3, Kind::Keyword, "ini key");
    }

    #[test]
    fn test_ini_no_equals() {
        let hl = hl_types(b"just text", &INI_RULES);
        // Without = sign, this isn't a key=value pair
        assert!(hl.iter().all(|&h| h != Kind::Keyword));
    }

    // -- YAML edge cases ------------------------------------------------------

    #[test]
    fn test_yaml_multiline_string() {
        let lines: &[&[u8]] = &[b"description: |", b"  multi line", b"  text here"];
        let hls = hl_multiline(lines, &YAML_RULES);
        assert_range(&hls[0], 0..11, Kind::Keyword, "description key");
    }

    #[test]
    fn test_yaml_empty_value() {
        let hl = hl_types(b"key:", &YAML_RULES);
        assert_range(&hl, 0..3, Kind::Keyword, "yaml key");
    }

    // -- Markdown edge cases --------------------------------------------------

    #[test]
    fn test_markdown_fenced_code_block_with_language() {
        let lines: &[&[u8]] = &[b"```rust", b"fn main() {}", b"```"];
        let hls = hl_multiline(lines, &MARKDOWN_RULES);
        assert!(
            hls[0].iter().all(|&h| h == Kind::String),
            "fence open with lang"
        );
        assert!(hls[1].iter().all(|&h| h == Kind::String), "fenced content");
        assert!(hls[2].iter().all(|&h| h == Kind::String), "fence close");
    }

    #[test]
    fn test_markdown_blockquote() {
        let hl = hl_types(b"> quoted text", &MARKDOWN_RULES);
        // Blockquote marker should be highlighted as Comment
        assert_eq!(hl[0], Kind::Comment);
    }

    // -- Language lookup ------------------------------------------------------

    #[test]
    fn test_language_aliases_and_extensions() {
        assert_eq!(Language::from_name("Rust"), Some(Language::Rust));
        assert_eq!(Language::from_name("shell"), Some(Language::Bash));
        assert_eq!(Language::from_extension(".json"), Some(Language::Json));
        assert_eq!(Language::from_extension("md"), Some(Language::Markdown));
        assert_eq!(Language::from_name("Brainfuck"), None);
    }

    #[test]
    fn test_operator_matching_is_longest_first() {
        static RULES: RuleSet = RuleSet {
            lexer_kind: LexerKind::Code,
            line_comment: "",
            block_comment: ("", ""),
            string_delims: &[],
            keywords: &[],
            types: &[],
            constants: &[],
            macros: &[],
            operators: &["=", "==", "==="],
            highlight_numbers: false,
            highlight_upper_constants: false,
            highlight_fn_calls: false,
            highlight_bang_macros: false,
        };
        let hl = hl_types(b"===", &RULES);
        assert!(hl.iter().all(|&h| h == Kind::Operator));
    }

    #[test]
    fn test_public_streaming_api_keeps_state_opaque() {
        let mut highlighter = Highlighter::new(Language::Rust);
        let mut scratch = Vec::new();
        let first = highlighter.highlight_into(b"/* comment", &mut scratch);
        assert!(first.iter().all(|&kind| kind == Kind::Comment));
        assert!(!highlighter.state().is_normal());

        let second = highlighter.highlight_into(b"done */ let", &mut scratch);
        assert_eq!(second[0], Kind::Comment);
        assert_eq!(second[8], Kind::Keyword);
        assert!(highlighter.state().is_normal());

        highlighter.reset();
        assert_eq!(highlighter.state(), State::default());
    }

    #[test]
    fn go_dotted_function_cleanup_keeps_underscored_names_together() {
        let mut highlighter = Highlighter::new(Language::Go);
        let mut scratch = Vec::new();
        let kinds = highlighter.highlight_into(b"pkg.foo_bar()", &mut scratch);
        assert!(kinds[4..11].iter().all(|&kind| kind == Kind::Normal));
    }

    #[test]
    fn test_stateless_api_and_runs() {
        let line = b"let x = 1";
        let mut kinds = vec![Kind::Normal; line.len()];
        let state = highlight_line_into(Language::Rust, State::default(), line, &mut kinds);
        assert!(state.is_normal());
        assert_eq!(kinds[0], Kind::Keyword);

        let grouped: Vec<_> = runs(&[
            Kind::Keyword,
            Kind::Keyword,
            Kind::Normal,
            Kind::Number,
            Kind::Number,
        ])
        .collect();
        assert_eq!(
            grouped,
            vec![
                Run {
                    start: 0,
                    end: 2,
                    kind: Kind::Keyword,
                },
                Run {
                    start: 2,
                    end: 3,
                    kind: Kind::Normal,
                },
                Run {
                    start: 3,
                    end: 5,
                    kind: Kind::Number,
                },
            ]
        );
    }

    #[test]
    fn test_language_aliases() {
        assert_eq!(Language::from_name("rs"), Some(Language::Rust));
        assert_eq!(Language::from_name("shell"), Some(Language::Bash));
        assert_eq!(Language::from_name("yml"), Some(Language::Yaml));
        assert_eq!(Language::from_extension(".md"), Some(Language::Markdown));
        assert_eq!(Language::from_name("unknown"), None);
    }

    // -- byte_kinds_to_char_kinds with multi-byte chars -----------------------------

    #[test]
    fn test_byte_kinds_to_char_kinds_multibyte() {
        // "é" is 2 bytes → 1 char highlight
        let text = "é".as_bytes();
        let byte_kinds = vec![Kind::String; text.len()];
        let char_hl = byte_kinds_to_char_kinds(text, &byte_kinds);
        assert_eq!(char_hl.len(), 1);
        assert_eq!(char_hl[0], Kind::String);
    }

    // -- Coverage gap: multiline string continuation (lines 166-169, 184-185) --

    #[test]
    fn test_multiline_string_continuation() {
        let rules = Language::from_name("Python").unwrap().rules();
        // Start a triple-quoted string that doesn't close
        let line1 = b"x = \"\"\"hello";
        let (_hl1, state1) = highlight_line(line1, StateKind::Normal, rules);
        assert!(matches!(state1, StateKind::MultiLineString(_)));
        // Continuation line with escape
        let line2 = b"world \\n more";
        let (hl2, state2) = highlight_line(line2, state1, rules);
        // All characters should be string
        assert_eq!(hl2[0], Kind::String);
        assert!(matches!(state2, StateKind::MultiLineString(_)));
        // Closing line
        let line3 = b"end\"\"\"";
        let (_hl3, state3) = highlight_line(line3, state2, rules);
        assert_eq!(state3, StateKind::Normal);
    }

    // -- Coverage gap: unclosed non-multiline string (line 266) ---------------

    #[test]
    fn test_unclosed_string_single_line() {
        let rules = Language::from_name("Rust").unwrap().rules();
        let line = b"let s = \"unterminated";
        let (hl, state) = highlight_line(line, StateKind::Normal, rules);
        // The string characters should be highlighted as String
        assert_eq!(hl[8], Kind::String); // opening quote
        assert_eq!(state, StateKind::Normal);
    }

    // -- Coverage gap: float starting with dot (line 330) ---------------------

    #[test]
    fn test_number_starting_with_dot() {
        let rules = Language::from_name("Rust").unwrap().rules();
        let line = b"let x = .5;";
        let (hl, _) = highlight_line(line, StateKind::Normal, rules);
        assert_eq!(hl[8], Kind::Number); // .
        assert_eq!(hl[9], Kind::Number); // 5
    }

    // -- Coverage gap: semver pre-release (lines 433-436) ---------------------

    #[test]
    fn test_semver_pre_release() {
        let rules = Language::from_name("TOML").unwrap().rules();
        let line = b"version = \"1.2.3-beta.1\"";
        let (hl, _) = highlight_line(line, StateKind::Normal, rules);
        // The version inside quotes should be Type (cyan/semver)
        assert_eq!(hl[11], Kind::Type); // '1' of version
    }

    // -- Coverage gap: YAML anchor/alias (lines 621-629) ----------------------

    #[test]
    fn test_yaml_anchor() {
        let line = b"&my_anchor";
        let mut hl = vec![Kind::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        assert_eq!(hl[0], Kind::Type); // '&'
        assert_eq!(hl[1], Kind::Type); // 'm'
    }

    #[test]
    fn test_yaml_alias() {
        let line = b"*my_alias";
        let mut hl = vec![Kind::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        assert_eq!(hl[0], Kind::Type);
    }

    // -- Coverage gap: YAML list item with key:value (lines 655-661) ----------

    #[test]
    fn test_yaml_list_item_with_key() {
        let line = b"- name: value";
        let mut hl = vec![Kind::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        assert_eq!(hl[2], Kind::Keyword); // 'n' of name
        assert_eq!(hl[5], Kind::Keyword); // 'e' of name
    }

    // -- Keyword lists must be sorted for binary search -----------------------

    #[test]
    fn test_keyword_lists_sorted() {
        let languages = [
            "Rust",
            "Python",
            "Go",
            "TypeScript",
            "JavaScript",
            "Shell",
            "C",
            "TOML",
            "JSON",
            "YAML",
            "Makefile",
            "Dockerfile",
            "Config",
            "XSH",
        ];
        for lang in languages {
            let rules = Language::from_name(lang).unwrap().rules();
            for (name, list) in [
                ("keywords", rules.keywords),
                ("types", rules.types),
                ("constants", rules.constants),
                ("macros", rules.macros),
            ] {
                for w in list.windows(2) {
                    assert!(
                        w[0] < w[1],
                        "{lang} {name} not sorted: {:?} >= {:?}",
                        w[0],
                        w[1]
                    );
                }
            }
        }
    }

    // -- Coverage gap: YAML negative number (lines 744-745) -------------------

    #[test]
    fn test_yaml_negative_number() {
        let line = b"  -42";
        let mut hl = vec![Kind::Normal; line.len()];
        highlight_yaml_value(line, &mut hl);
        assert_eq!(hl[2], Kind::Number); // '-'
        assert_eq!(hl[3], Kind::Number); // '4'
    }

    // -- Coverage gap: find_yaml_colon with quoted colon (lines 675-680) ------

    #[test]
    fn test_yaml_colon_in_quotes() {
        let line = b"\"key:with:colons\": value";
        let mut hl = vec![Kind::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        // The colon inside quotes should not split key/value
        // The actual key ends at the colon after the closing quote
        assert_eq!(hl[0], Kind::Keyword);
    }

    // -- XSH -----------------------------------------------------------------

    #[test]
    fn test_xsh_keyword() {
        let hl = hl_types(b"let x = 1", &XSH_RULES);
        assert_eq!(hl[0], Kind::Keyword);
        assert_eq!(hl[1], Kind::Keyword);
        assert_eq!(hl[2], Kind::Keyword);
    }

    #[test]
    fn test_xsh_comment() {
        let hl = hl_types(b"# a comment", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Comment));
    }

    #[test]
    fn test_xsh_string() {
        let hl = hl_types(b"let s = \"hello\";", &XSH_RULES);
        assert_eq!(hl[8], Kind::String); // "
        assert_eq!(hl[9], Kind::String); // h
        assert_eq!(hl[14], Kind::String); // closing "
    }

    #[test]
    fn test_xsh_prefixed_strings() {
        // bytes literal
        let hl = hl_types(b"b\"data\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));

        // path literal
        let hl = hl_types(b"p\"/usr/bin\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));

        // format string
        let hl = hl_types(b"f\"hello ${name}\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));

        // raw string
        let hl = hl_types(b"r\"raw\\n\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));

        // glob literal
        let hl = hl_types(b"g\"*.rs\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));

        // formatted path
        let hl = hl_types(b"fp\"${root}/child\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));
    }

    #[test]
    fn test_xsh_triple_quoted() {
        let hl = hl_types(b"\"\"\"multi\nline\"\"\"", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));
    }

    #[test]
    fn test_xsh_type() {
        let hl = hl_types(b"Result[Int]", &XSH_RULES);
        assert_eq!(hl[0], Kind::Type); // R
        assert_eq!(hl[5], Kind::Type); // t
    }

    #[test]
    fn test_xsh_operator() {
        let hl = hl_types(b"x ?? y", &XSH_RULES);
        assert_eq!(hl[2], Kind::Operator);
        assert_eq!(hl[3], Kind::Operator);

        let hl = hl_types(b"a |> b", &XSH_RULES);
        assert_eq!(hl[2], Kind::Operator);
        assert_eq!(hl[3], Kind::Operator);

        let hl = hl_types(b"=>", &XSH_RULES);
        assert_eq!(hl[0], Kind::Operator);
        assert_eq!(hl[1], Kind::Operator);

        let hl = hl_types(b"x != y", &XSH_RULES);
        assert_eq!(hl[2], Kind::Operator);
        assert_eq!(hl[3], Kind::Operator);
    }

    #[test]
    fn test_xsh_stdlib_macro() {
        // module names, stream stages, and builtins are Macro (bold magenta)
        let hl = hl_types(b"print total", &XSH_RULES);
        assert_eq!(hl[0], Kind::Macro); // p
        assert_eq!(hl[5], Kind::Normal); // total is ordinary

        let hl = hl_types(b"|> where .kind", &XSH_RULES);
        assert_eq!(hl[3], Kind::Macro); // where
        assert_eq!(hl[4], Kind::Macro); // all of 'where'
        let hl = hl_types(b"|> sort-by .name", &XSH_RULES);
        assert_eq!(hl[3], Kind::Macro); // 'sort' of sort-by
        let hl = hl_types(b"|> count {key}", &XSH_RULES);
        assert_eq!(hl[3], Kind::Macro); // count

        let hl = hl_types(b"fs.files(root)?", &XSH_RULES);
        assert_eq!(hl[0], Kind::Macro); // fs module
        assert_eq!(hl[2], Kind::Normal); // .files is a call, not a static word
        let hl = hl_types(b"net.download(url)?", &XSH_RULES);
        assert_eq!(hl[0], Kind::Macro); // net module
    }

    #[test]
    fn test_xsh_new_keywords() {
        // Keywords added since the registry was introduced.
        for kw in [
            "export", "guard", "loop", "unless", "when", "with", "yield", "stream",
        ] {
            let line = format!("{} x", kw);
            let hl = hl_types(line.as_bytes(), &XSH_RULES);
            assert_eq!(hl[0], Kind::Keyword, "{kw} should be a keyword");
        }
    }

    #[test]
    fn test_xsh_record_types() {
        // Record names from the registry are types (cyan).
        for ty in [
            "FsEntry",
            "LinuxBlockDevice",
            "NetResponse",
            "User",
            "Spawn",
        ] {
            let line = format!("{} value", ty);
            let hl = hl_types(line.as_bytes(), &XSH_RULES);
            assert_eq!(hl[0], Kind::Type, "{ty} should be a type");
            assert_eq!(hl[0], hl[ty.len() - 1], "whole {ty} identifier");
        }
    }

    #[test]
    fn test_xsh_realistic_program() {
        // A realistic XSH pipeline drawn from examples/streams.xsh.
        let lines: &[&[u8]] = &[
            b"let root_handle = fs.tempdir()?",
            b"defer fs.close_root(root_handle)?",
            b"let root = fs.root_path(root_handle)?",
            b"let src = fp\"${root}/src\"",
            b"src.mkdir()",
            b"let reports = fs.files(root)",
            b"  |> where .kind == \"file\"",
            b"  |> map { |entry|",
            b"    {name: entry.name, size: entry.size, parent: entry.path.parent().name}",
            b"  }",
            b"  |> sort-by .name",
            b"pure id(value: Str) -> Str { value }",
        ];
        let hls = hl_multiline(lines, &XSH_RULES);

        // line 0: `let` keyword, `fs` module, `tempdir` is a function call
        assert_eq!(hls[0][0], Kind::Keyword); // let
        assert_eq!(hls[0][18], Kind::Macro); // fs
        assert_eq!(hls[0][21], Kind::Function); // tempdir
        assert_eq!(hls[0][28], Kind::Bracket); // (
        // line 1: `defer` keyword
        assert_eq!(hls[1][0], Kind::Keyword); // defer
        assert_eq!(hls[1][6], Kind::Macro); // fs
        // line 3: fp"..." string
        assert!(hls[3][10..].iter().all(|&h| h == Kind::String));
        // line 5: `let`, `fs`, `files` function
        assert_eq!(hls[5][0], Kind::Keyword); // let
        assert_eq!(hls[5][14], Kind::Macro); // fs
        assert_eq!(hls[5][17], Kind::Function); // files
        // line 6: `|>` operator, `where` stage
        assert_eq!(hls[6][2], Kind::Operator); // |
        assert_eq!(hls[6][3], Kind::Operator); // >
        assert_eq!(hls[6][5], Kind::Macro); // where
        // line 7: `map` stage, `|entry|` is plain
        assert_eq!(hls[7][5], Kind::Macro); // map
        assert_eq!(hls[7][10], Kind::Normal); // entry
        // line 10: `sort` of `sort-by` is a stage
        assert_eq!(hls[10][5], Kind::Macro); // sort
        // line 11: `pure` keyword, `Str` type, `->` operator
        assert_eq!(hls[11][0], Kind::Keyword); // pure
        assert_eq!(hls[11][17], Kind::Type); // Str
        assert_eq!(hls[11][20], Kind::Operator); // ->
    }

    #[test]
    fn test_xsh_type_schema_declaration() {
        let hl = hl_types(b"type Config = { name: Str, retries: Int }", &XSH_RULES);
        assert_eq!(hl[0], Kind::Keyword); // type
        assert_eq!(hl[24], Kind::Type); // Str
        assert_eq!(hl[37], Kind::Type); // Int
        assert_eq!(hl[16], Kind::Normal); // name (field)
    }

    #[test]
    fn test_xsh_number() {
        let hl = hl_types(b"let x = 42;", &XSH_RULES);
        assert_eq!(hl[8], Kind::Number);
        assert_eq!(hl[9], Kind::Number);

        // octal
        let hl = hl_types(b"0o755", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Number));

        // float
        let hl = hl_types(b"3.14", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Number));

        // duration
        let hl = hl_types(b"100ms", &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::Number));
    }

    #[test]
    fn test_xsh_function_call() {
        let hl = hl_types(b"my_func(\"hi\")", &XSH_RULES);
        assert_eq!(hl[0], Kind::Function); // m
        assert_eq!(hl[6], Kind::Function); // c
        assert_eq!(hl[7], Kind::Bracket); // (
    }

    #[test]
    fn test_xsh_stdlib_purple() {
        // stdlib names are Macro (bold magenta / purple)
        let hl = hl_types(b"print(\"hi\")", &XSH_RULES);
        assert_eq!(hl[0], Kind::Macro); // p
        assert_eq!(hl[4], Kind::Macro); // t

        let hl = hl_types(b"fs.mkdir(\"dir\")", &XSH_RULES);
        assert_eq!(hl[0], Kind::Macro); // f
        assert_eq!(hl[1], Kind::Macro); // s

        let hl = hl_types(b"abort(1)", &XSH_RULES);
        assert_eq!(hl[0], Kind::Macro);
    }

    #[test]
    fn test_xsh_multiline_string_state() {
        let (hl, state) = highlight_line(b"\"\"\"hello", StateKind::Normal, &XSH_RULES);
        assert!(hl.iter().all(|&h| h == Kind::String));
        assert!(matches!(state, StateKind::MultiLineString(_)));
    }

    #[test]
    fn test_xsh_proc_definition() {
        let hl = hl_types(b"proc build() [fs] -> Result[Unit] {", &XSH_RULES);
        assert_eq!(hl[0], Kind::Keyword); // proc
        assert_eq!(hl[5], Kind::Function); // build
    }

    #[test]
    fn test_xsh_user_type_highlight() {
        let user = vec![b"MyType".to_vec(), b"MyVariant".to_vec()];

        // Simple alias
        let hl = highlight_line(b"let x: MyType = 1", StateKind::Normal, &XSH_RULES).0;
        assert_eq!(hl[8], Kind::Normal); // MyType not highlighted without user_types

        let mut highlighter = crate::Highlighter::new(Language::Xsh);
        highlighter.set_user_types(&user);
        let mut out = Vec::new();
        highlighter.highlight_into(b"let x: MyType = 1", &mut out);
        assert_eq!(out[8], Kind::Type); // M
        assert_eq!(out[12], Kind::Type); // e

        // Tag variant
        highlighter.reset();
        highlighter.highlight_into(b"match x { MyVariant => 1 }", &mut out);
        assert_eq!(out[10], Kind::Type); // M
        assert_eq!(out[18], Kind::Type); // t
    }
}
