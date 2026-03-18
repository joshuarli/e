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
    Bracket,
    Operator,
    Function,
    Constant,
    Macro,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum HlState {
    #[default]
    Normal,
    BlockComment,
    MultiLineString(u8),
    FencedCodeBlock,
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
    pub constants: &'static [&'static str],
    pub macros: &'static [&'static str],
    pub operators: &'static [&'static str],
    pub highlight_numbers: bool,
    /// Highlight UPPER_SNAKE_CASE identifiers as constants.
    pub highlight_upper_constants: bool,
    /// Highlight identifiers followed by `(` as functions.
    pub highlight_fn_calls: bool,
    /// Highlight `ident!` patterns as macros (Rust-style).
    pub highlight_bang_macros: bool,
    pub is_markdown: bool,
    pub is_json: bool,
    pub is_yaml: bool,
    pub is_ini: bool,
}

// -- ANSI color codes -------------------------------------------------------

impl HlType {
    /// Return the ANSI color code for this highlight type, or empty for Normal.
    pub fn ansi_code(self) -> &'static str {
        match self {
            HlType::Normal => "",
            HlType::Comment => "\x1b[90m",    // grey
            HlType::Keyword => "\x1b[33m",    // yellow
            HlType::Type => "\x1b[36m",       // cyan
            HlType::String => "\x1b[32m",     // green
            HlType::Number => "\x1b[31m",     // red
            HlType::Bracket => "\x1b[35m",    // magenta
            HlType::Operator => "\x1b[33m",   // yellow (same as keyword)
            HlType::Function => "\x1b[34m",   // blue
            HlType::Constant => "\x1b[31;1m", // bold red
            HlType::Macro => "\x1b[35;1m",    // bold magenta
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
    let mut hl = Vec::new();
    let next_state = highlight_line_into(line, state, rules, &mut hl);
    (hl, next_state)
}

fn highlight_line_code(
    line: &[u8],
    state: HlState,
    rules: &SyntaxRules,
    hl: &mut [HlType],
) -> HlState {
    let len = line.len();
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
                return HlState::BlockComment;
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
                return current_state;
            }
        }
        HlState::Normal => {}
        HlState::FencedCodeBlock => {}
    }

    // Main loop
    while i < len {
        // Line comment
        if !line_com.is_empty() && starts_with_at(line, line_com, i) {
            for b in &mut hl[i..len] {
                *b = HlType::Comment;
            }
            return HlState::Normal;
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
                return HlState::BlockComment;
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
                        return HlState::MultiLineString(di as u8);
                    }
                    return HlState::Normal;
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

        // Keywords, types, constants, macros (after separator)
        if prev_sep && (line[i].is_ascii_alphabetic() || line[i] == b'_') {
            // Extract the full identifier once.
            let id_start = i;
            i += 1;
            while i < len && (line[i].is_ascii_alphanumeric() || line[i] == b'_') {
                i += 1;
            }
            let id = &line[id_start..i];

            // Binary search each sorted keyword list.
            let matched = if keyword_search(id, rules.keywords) {
                Some(HlType::Keyword)
            } else if keyword_search(id, rules.types) {
                Some(HlType::Type)
            } else if keyword_search(id, rules.constants) {
                Some(HlType::Constant)
            } else if keyword_search(id, rules.macros) {
                Some(HlType::Macro)
            } else {
                None
            };

            if let Some(hl_type) = matched {
                for b in &mut hl[id_start..i] {
                    *b = hl_type;
                }
                prev_sep = false;
                continue;
            }

            // Rust-style macros: ident!
            if i < len && line[i] == b'!' && rules.highlight_bang_macros {
                // Only treat as macro if the `!` is not followed by `=` (i.e. not `!=`)
                if i + 1 >= len || line[i + 1] != b'=' {
                    for b in &mut hl[id_start..i] {
                        *b = HlType::Macro;
                    }
                    hl[i] = HlType::Macro; // the `!`
                    i += 1;
                    prev_sep = true;
                    continue;
                }
            }
            // Function calls: ident(
            if rules.highlight_fn_calls && i < len && line[i] == b'(' {
                for b in &mut hl[id_start..i] {
                    *b = HlType::Function;
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
                        *b = HlType::Constant;
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
            hl[i] = HlType::Bracket;
        }
        prev_sep = is_separator(line[i]);
        i += 1;
    }

    HlState::Normal
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

fn try_operator(line: &[u8], pos: usize, ops: &[&str], hl: &mut [HlType]) -> Option<usize> {
    for &op in ops {
        let ob = op.as_bytes();
        if starts_with_at(line, ob, pos) {
            for b in &mut hl[pos..pos + ob.len()] {
                *b = HlType::Operator;
            }
            return Some(ob.len());
        }
    }
    None
}

/// Binary search a **sorted** keyword list for an exact match.
fn keyword_search(id: &[u8], words: &[&str]) -> bool {
    words.binary_search_by(|w| w.as_bytes().cmp(id)).is_ok()
}

// -- Semver highlighting ----------------------------------------------------

/// Post-pass: highlight semver patterns like v1.2.3 or 0.3.5-beta.1
fn highlight_semver(line: &[u8], hl: &mut [HlType]) {
    let len = line.len();
    let mut i = 0;
    while i < len {
        // Don't start inside a comment
        if hl[i] == HlType::Comment {
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
            *b = HlType::Type;
        }
    }
}

// -- JSON highlighting ------------------------------------------------------

fn highlight_line_json(line: &[u8], _state: HlState, hl: &mut [HlType]) -> HlState {
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
                HlType::Keyword // key → yellow
            } else {
                HlType::String // value → green
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
                    *b = HlType::Number;
                }
                continue;
            }
        }

        // true, false, null
        for &(word, hl_type) in &[
            (&b"true"[..], HlType::Type),
            (&b"false"[..], HlType::Type),
            (&b"null"[..], HlType::Type),
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
            hl[i] = HlType::Bracket;
        }

        i += 1;
    }

    HlState::Normal
}

// -- YAML highlighting ------------------------------------------------------

fn highlight_line_yaml(line: &[u8], _state: HlState, hl: &mut [HlType]) -> HlState {
    let len = line.len();

    if len == 0 {
        return HlState::Normal;
    }

    // Comment: # (at start or after whitespace)
    if let Some(comment_start) = find_yaml_comment(line) {
        for b in &mut hl[comment_start..len] {
            *b = HlType::Comment;
        }
        // Highlight the part before the comment
        if comment_start > 0 {
            highlight_yaml_content(&line[..comment_start], &mut hl[..comment_start]);
        }
        return HlState::Normal;
    }

    highlight_yaml_content(line, hl);
    HlState::Normal
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

fn highlight_yaml_content(line: &[u8], hl: &mut [HlType]) {
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
            *b = HlType::Type;
        }
        return;
    }

    // Find unquoted colon that marks key: value
    if let Some(colon_pos) = find_yaml_colon(rest) {
        let abs_colon = indent + colon_pos;
        // Key portion (before colon)
        for b in &mut hl[indent..abs_colon] {
            *b = HlType::Keyword;
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
        hl[indent] = HlType::Normal;
        let val_start = indent + 2;
        if val_start < len {
            // Check if the list item contains a key
            let item_rest = &line[val_start..];
            if let Some(colon_pos) = find_yaml_colon(item_rest) {
                let abs_colon = val_start + colon_pos;
                for b in &mut hl[val_start..abs_colon] {
                    *b = HlType::Keyword;
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

fn highlight_yaml_value(val: &[u8], hl: &mut [HlType]) {
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
            *b = HlType::String;
        }
        return;
    }

    // true/false/null/yes/no
    for &(word, hl_type) in &[
        (&b"true"[..], HlType::Type),
        (&b"false"[..], HlType::Type),
        (&b"null"[..], HlType::Type),
        (&b"yes"[..], HlType::Type),
        (&b"no"[..], HlType::Type),
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
                *b = HlType::Number;
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
            *b = HlType::Type;
        }
    }
}

// -- INI/Config highlighting ------------------------------------------------

fn highlight_line_ini(line: &[u8], _state: HlState, hl: &mut [HlType]) -> HlState {
    let len = line.len();

    if len == 0 {
        return HlState::Normal;
    }

    // Skip leading whitespace
    let indent = line
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let rest = &line[indent..];

    if rest.is_empty() {
        return HlState::Normal;
    }

    // Comment lines: ; or # at start (after optional whitespace)
    if rest[0] == b';' || rest[0] == b'#' {
        for b in &mut hl[indent..] {
            *b = HlType::Comment;
        }
        return HlState::Normal;
    }

    // Section headers: [section]
    if rest[0] == b'[' {
        if let Some(close) = rest.iter().position(|&b| b == b']') {
            for b in &mut hl[indent..indent + close + 1] {
                *b = HlType::Keyword;
            }
            // Anything after ] could be an inline comment
            let after = indent + close + 1;
            if after < len {
                highlight_ini_inline_comment(line, hl, after);
            }
        }
        return HlState::Normal;
    }

    // Key = value pairs
    if let Some(eq_pos) = rest.iter().position(|&b| b == b'=') {
        let abs_eq = indent + eq_pos;
        // Key (before =)
        for b in &mut hl[indent..abs_eq] {
            *b = HlType::Keyword;
        }
        // Value (after =)
        let val_start = abs_eq + 1;
        if val_start < len {
            highlight_ini_value(&line[val_start..], &mut hl[val_start..]);
        }
    }

    HlState::Normal
}

fn highlight_ini_value(val: &[u8], hl: &mut [HlType]) {
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
            *b = HlType::Comment;
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
            *b = HlType::String;
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
                *b = HlType::Type;
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
                *b = HlType::Number;
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
fn highlight_ini_inline_comment(line: &[u8], hl: &mut [HlType], start: usize) {
    let rest = &line[start..];
    let ws = rest
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let after_ws = start + ws;
    if after_ws < line.len() && (line[after_ws] == b';' || line[after_ws] == b'#') {
        for b in &mut hl[after_ws..] {
            *b = HlType::Comment;
        }
    }
}

// -- Markdown highlighting --------------------------------------------------

fn highlight_line_markdown(
    line: &[u8],
    state: HlState,
    rules: &SyntaxRules,
    hl: &mut [HlType],
) -> HlState {
    let len = line.len();

    let block_close = rules.block_comment.1.as_bytes();

    // Fenced code block: entering or continuing
    if state == HlState::FencedCodeBlock {
        if len >= 3 && line[0] == b'`' && line[1] == b'`' && line[2] == b'`' {
            for b in &mut hl[..len] {
                *b = HlType::String;
            }
            return HlState::Normal;
        }
        for b in &mut hl[..len] {
            *b = HlType::String;
        }
        return HlState::FencedCodeBlock;
    }

    // Block comment continuation
    if state == HlState::BlockComment {
        let mut i = 0;
        while i < len {
            if starts_with_at(line, block_close, i) {
                let end = i + block_close.len();
                for b in &mut hl[i..end] {
                    *b = HlType::Comment;
                }
                // hl[0..end] is all Comment; process remainder as inline markdown
                return highlight_line_markdown_inner(&line[end..], rules, &mut hl[end..]);
            }
            hl[i] = HlType::Comment;
            i += 1;
        }
        return HlState::BlockComment;
    }

    // Fenced code block start
    if len >= 3 && line[0] == b'`' && line[1] == b'`' && line[2] == b'`' {
        for b in &mut hl[..len] {
            *b = HlType::String;
        }
        return HlState::FencedCodeBlock;
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
                    *b = HlType::Comment;
                }
                return HlState::Normal;
            }
        }
    }

    // Headers: # at line start
    if len > 0 && line[0] == b'#' {
        for b in &mut hl[..len] {
            *b = HlType::Keyword;
        }
        return HlState::Normal;
    }

    // Blockquote: > at line start
    if len > 0 && line[0] == b'>' {
        hl[0] = HlType::Comment;
        if len > 1 && line[1] == b' ' {
            hl[1] = HlType::Comment;
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
                *b = HlType::Number;
            }
            let after = indent + marker_len;
            return highlight_line_markdown_inner(&line[after..], rules, &mut hl[after..]);
        }
    }

    // Normal line — process inline elements
    highlight_line_markdown_inner(line, rules, hl)
}

/// Process inline markdown elements: inline code, bold, italic, HTML comments.
fn highlight_line_markdown_inner(line: &[u8], rules: &SyntaxRules, hl: &mut [HlType]) -> HlState {
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
                        *b = HlType::Comment;
                    }
                    i = end;
                    found = true;
                    break;
                }
                i += 1;
            }
            if !found {
                for b in &mut hl[start..len] {
                    *b = HlType::Comment;
                }
                return HlState::BlockComment;
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
                    *b = HlType::String;
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
                    *b = HlType::Keyword;
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
                    *b = HlType::Type;
                }
            }
            continue;
        }

        i += 1;
    }

    HlState::Normal
}

// -- Bracket matching -------------------------------------------------------

use crate::selection::Pos;

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
    pos: Pos,
    get_line: &mut impl FnMut(usize, &mut Vec<u8>),
    scratch: &mut Vec<u8>,
    line_count: usize,
) -> Option<Pos> {
    get_line(pos.line, scratch);
    // Convert char col to byte index
    let byte_idx = char_col_to_byte(scratch, pos.col)?;
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
                        let col = byte_to_char_col(scratch, bi);
                        return Some(Pos::new(l, col));
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
                        let col = byte_to_char_col(scratch, b);
                        return Some(Pos::new(l, col));
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
    pos: Pos,
    get_line: &mut impl FnMut(usize, &mut Vec<u8>),
    scratch: &mut Vec<u8>,
    _line_count: usize,
) -> Option<Pos> {
    get_line(pos.line, scratch);
    let byte_idx = char_col_to_byte(scratch, pos.col)?;
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
            let col = byte_to_char_col(scratch, close);
            return Some(Pos::new(pos.line, col));
        }
        if byte_idx == close {
            let col = byte_to_char_col(scratch, open);
            return Some(Pos::new(pos.line, col));
        }
        pair_idx += 2;
    }
    None
}

fn char_col_to_byte(line: &[u8], char_col: usize) -> Option<usize> {
    if line.is_ascii() {
        return Some(char_col.min(line.len()));
    }
    let mut bi = 0;
    let mut ci = 0;
    while ci < char_col && bi < line.len() {
        bi += buffer::utf8_char_len(line[bi]);
        ci += 1;
    }
    Some(bi)
}

fn byte_to_char_col(line: &[u8], byte_idx: usize) -> usize {
    if line.is_ascii() {
        return byte_idx.min(line.len());
    }
    let mut bi = 0;
    let mut ci = 0;
    while bi < byte_idx && bi < line.len() {
        bi += buffer::utf8_char_len(line[bi]);
        ci += 1;
    }
    ci
}

// -- Byte-to-char mapping ---------------------------------------------------

/// Map byte-indexed highlights to char-indexed highlights, writing into `out`.
/// Tabs expand to 2 display entries, multi-byte UTF-8 collapses to 1 entry.
/// Clears `out` first; reuses its allocation across calls.
pub fn byte_hl_to_char_hl_into(raw: &[u8], byte_hl: &[HlType], out: &mut Vec<HlType>) {
    out.clear();
    if raw.is_ascii() {
        // ASCII fast path: 1 byte = 1 char, tabs expand to 2 display positions
        out.reserve(raw.len());
        for (i, &b) in raw.iter().enumerate() {
            out.push(byte_hl[i]);
            if b == b'\t' {
                out.push(byte_hl[i]);
            }
        }
    } else {
        let mut bi = 0;
        while bi < raw.len() {
            let ht = byte_hl[bi];
            if raw[bi] == b'\t' {
                out.push(ht);
                out.push(ht);
                bi += 1;
            } else {
                out.push(ht);
                bi += buffer::utf8_char_len(raw[bi]);
            }
        }
    }
}

/// Allocating wrapper around `byte_hl_to_char_hl_into`. Used in tests.
#[allow(dead_code)]
pub fn byte_hl_to_char_hl(raw: &[u8], byte_hl: &[HlType]) -> Vec<HlType> {
    let mut out = Vec::with_capacity(raw.len());
    byte_hl_to_char_hl_into(raw, byte_hl, &mut out);
    out
}

/// Like `highlight_line` but writes the per-byte highlights into `out`
/// (clearing it first), reusing its allocation across calls.
/// Returns only the next-line `HlState`.
pub fn highlight_line_into(
    line: &[u8],
    state: HlState,
    rules: &SyntaxRules,
    out: &mut Vec<HlType>,
) -> HlState {
    out.clear();
    out.resize(line.len(), HlType::Normal);
    let next_state = if rules.is_markdown {
        highlight_line_markdown(line, state, rules, out)
    } else if rules.is_json {
        highlight_line_json(line, state, out)
    } else if rules.is_yaml {
        highlight_line_yaml(line, state, out)
    } else if rules.is_ini {
        highlight_line_ini(line, state, out)
    } else {
        highlight_line_code(line, state, rules, out)
    };
    highlight_semver(line, out);
    next_state
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
        "Box", "Err", "None", "Ok", "Option", "Result", "Self", "Some", "String", "Vec", "bool",
        "char", "f32", "f64", "false", "i128", "i16", "i32", "i64", "i8", "isize", "str", "true",
        "u128", "u16", "u32", "u64", "u8", "usize",
    ],
    constants: &[],
    macros: &[],
    operators: &["&&", "||", "!=", "==", "<=", ">=", "=>", "->"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: true,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
        "complex128",
        "complex64",
        "error",
        "false",
        "float32",
        "float64",
        "int",
        "int16",
        "int32",
        "int64",
        "int8",
        "iota",
        "nil",
        "rune",
        "string",
        "true",
        "uint",
        "uint16",
        "uint32",
        "uint64",
        "uint8",
        "uintptr",
    ],
    constants: &[],
    macros: &[],
    operators: &["&&", "||", "!=", "==", "<=", ">=", ":="],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
        "Array",
        "Map",
        "Promise",
        "Set",
        "any",
        "bigint",
        "boolean",
        "false",
        "never",
        "null",
        "number",
        "object",
        "string",
        "symbol",
        "true",
        "undefined",
        "unknown",
        "void",
    ],
    constants: &[],
    macros: &[],
    operators: &["&&", "||", "!==", "===", "!=", "==", "<=", ">=", "=>"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
        "Array",
        "Boolean",
        "Infinity",
        "Map",
        "NaN",
        "Number",
        "Object",
        "Promise",
        "Set",
        "String",
        "false",
        "null",
        "true",
        "undefined",
    ],
    constants: &[],
    macros: &[],
    operators: &["&&", "||", "!==", "===", "!=", "==", "<=", ">=", "=>"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
        "break", "case", "continue", "declare", "do", "done", "elif", "else", "esac", "eval",
        "exec", "exit", "export", "fi", "for", "function", "if", "in", "local", "readonly",
        "return", "set", "shift", "source", "then", "trap", "unset", "while",
    ],
    types: &["false", "true"],
    constants: &[],
    macros: &[],
    operators: &["&&", "||"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
        "NULL", "bool", "char", "double", "false", "float", "int", "int16_t", "int32_t", "int64_t",
        "int8_t", "long", "short", "signed", "size_t", "true", "uint16_t", "uint32_t", "uint64_t",
        "uint8_t", "unsigned", "void",
    ],
    constants: &[],
    macros: &[],
    operators: &["&&", "||", "!=", "==", "<=", ">=", "->"],
    highlight_numbers: true,
    highlight_upper_constants: true,
    highlight_fn_calls: true,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
    types: &["false", "true"],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};

static JSON_STRINGS: &[StringDelim] = &[string_delim!("\"", "\"", false)];

static JSON_RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("", ""),
    string_delims: JSON_STRINGS,
    keywords: &[],
    types: &["false", "null", "true"],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: true,
    is_yaml: false,
    is_ini: false,
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
    types: &["false", "no", "null", "true", "yes"],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: true,
    is_ini: false,
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
        "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef", "ifneq",
        "include", "override", "unexport", "vpath",
    ],
    types: &[],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
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
        "ADD",
        "ARG",
        "AS",
        "CMD",
        "COPY",
        "ENTRYPOINT",
        "ENV",
        "EXPOSE",
        "FROM",
        "HEALTHCHECK",
        "LABEL",
        "ONBUILD",
        "RUN",
        "SHELL",
        "STOPSIGNAL",
        "USER",
        "VOLUME",
        "WORKDIR",
    ],
    types: &[],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: false,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};

static MARKDOWN_RULES: SyntaxRules = SyntaxRules {
    line_comment: "",
    block_comment: ("<!--", "-->"),
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
    is_markdown: true,
    is_json: false,
    is_yaml: false,
    is_ini: false,
};

static INI_STRINGS: &[StringDelim] = &[
    string_delim!("\"", "\"", false),
    string_delim!("'", "'", false),
];

static INI_RULES: SyntaxRules = SyntaxRules {
    line_comment: ";",
    block_comment: ("", ""),
    string_delims: INI_STRINGS,
    keywords: &[],
    types: &["false", "no", "off", "on", "true", "yes"],
    constants: &[],
    macros: &[],
    operators: &[],
    highlight_numbers: true,
    highlight_upper_constants: false,
    highlight_fn_calls: false,
    highlight_bang_macros: false,
    is_markdown: false,
    is_json: false,
    is_yaml: false,
    is_ini: true,
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
        "Markdown" => Some(&MARKDOWN_RULES),
        "Config" => Some(&INI_RULES),
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

    // -- Function call highlighting -----------------------------------------

    #[test]
    fn test_function_call() {
        let hl = hl_types(b"foo(x)", &RUST_RULES);
        assert_eq!(hl[0], HlType::Function); // f
        assert_eq!(hl[1], HlType::Function); // o
        assert_eq!(hl[2], HlType::Function); // o
        assert_eq!(hl[3], HlType::Bracket); // (
    }

    #[test]
    fn test_method_call() {
        let hl = hl_types(b"x.method(y)", &RUST_RULES);
        assert_eq!(hl[0], HlType::Normal); // x
        assert_eq!(hl[2], HlType::Function); // m
        assert_eq!(hl[7], HlType::Function); // d
        assert_eq!(hl[8], HlType::Bracket); // (
    }

    #[test]
    fn test_keyword_not_function() {
        // "if(" should still be keyword, not function
        let hl = hl_types(b"if(x)", &RUST_RULES);
        assert_eq!(hl[0], HlType::Keyword); // i
        assert_eq!(hl[1], HlType::Keyword); // f
    }

    // -- Constant highlighting ----------------------------------------------

    #[test]
    fn test_upper_snake_constant() {
        let hl = hl_types(b"let x = MAX_SIZE;", &RUST_RULES);
        assert_eq!(hl[8], HlType::Constant); // M
        assert_eq!(hl[15], HlType::Constant); // E
    }

    #[test]
    fn test_single_upper_char_not_constant() {
        // Single uppercase letter shouldn't be constant (need >=2 chars)
        let hl = hl_types(b"let X = 1;", &RUST_RULES);
        assert_eq!(hl[4], HlType::Normal); // X
    }

    #[test]
    fn test_mixed_case_not_constant() {
        let hl = hl_types(b"let MyVar = 1;", &RUST_RULES);
        assert_eq!(hl[4], HlType::Normal); // M
    }

    // -- Macro highlighting -------------------------------------------------

    #[test]
    fn test_rust_bang_macro() {
        let hl = hl_types(b"println!(\"hi\")", &RUST_RULES);
        assert_eq!(hl[0], HlType::Macro); // p
        assert_eq!(hl[6], HlType::Macro); // n
        assert_eq!(hl[7], HlType::Macro); // !
        assert_eq!(hl[8], HlType::Bracket); // (
    }

    #[test]
    fn test_bang_not_macro_in_python() {
        // Python doesn't have bang macros, so foo! is not a macro
        let hl = hl_types(b"foo!(x)", &PYTHON_RULES);
        assert_eq!(hl[0], HlType::Normal); // f
        assert_eq!(hl[2], HlType::Normal); // o
    }

    #[test]
    fn test_not_equal_not_macro() {
        // foo != bar — the != should not be treated as a macro invocation
        let hl = hl_types(b"foo != bar", &RUST_RULES);
        assert_eq!(hl[0], HlType::Normal); // f
        assert_eq!(hl[4], HlType::Operator); // !
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
        assert!(rules_for_language("Config").is_some());
    }

    #[test]
    fn test_rules_for_unknown() {
        assert!(rules_for_language("Unknown").is_none());
    }

    #[test]
    fn test_rules_for_markdown() {
        assert!(rules_for_language("Markdown").is_some());
    }

    // -- INI/Config ---------------------------------------------------------

    #[test]
    fn test_ini_config() {
        // Section header
        let hl = hl_types(b"[section]", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword); // [
        assert_eq!(hl[4], HlType::Keyword); // i
        assert_eq!(hl[8], HlType::Keyword); // ]

        // Key = value
        let hl = hl_types(b"key = value", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword); // k
        assert_eq!(hl[2], HlType::Keyword); // y
        assert_eq!(hl[4], HlType::Normal); // =
        assert_eq!(hl[6], HlType::Normal); // v (unquoted string)

        // Quoted string value
        let hl = hl_types(b"name = \"hello\"", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword); // n
        assert_eq!(hl[7], HlType::String); // "
        assert_eq!(hl[12], HlType::String); // o
        assert_eq!(hl[13], HlType::String); // "

        // Single-quoted string value
        let hl = hl_types(b"name = 'hello'", &INI_RULES);
        assert_eq!(hl[7], HlType::String);
        assert_eq!(hl[13], HlType::String);

        // Semicolon comment
        let hl = hl_types(b"; this is a comment", &INI_RULES);
        assert!(hl.iter().all(|&h| h == HlType::Comment));

        // Hash comment
        let hl = hl_types(b"# this is a comment", &INI_RULES);
        assert!(hl.iter().all(|&h| h == HlType::Comment));

        // Indented comment
        let hl = hl_types(b"  ; indented comment", &INI_RULES);
        assert_eq!(hl[0], HlType::Normal);
        assert_eq!(hl[2], HlType::Comment);
        assert_eq!(hl[19], HlType::Comment);

        // Number value
        let hl = hl_types(b"port = 8080", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword); // p
        assert_eq!(hl[7], HlType::Number); // 8
        assert_eq!(hl[10], HlType::Number); // 0

        // Boolean type
        let hl = hl_types(b"enabled = true", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword);
        assert_eq!(hl[10], HlType::Type); // t
        assert_eq!(hl[13], HlType::Type); // e

        // Case-insensitive boolean
        let hl = hl_types(b"flag = TRUE", &INI_RULES);
        assert_eq!(hl[7], HlType::Type);

        let hl = hl_types(b"flag = Yes", &INI_RULES);
        assert_eq!(hl[7], HlType::Type);

        let hl = hl_types(b"debug = off", &INI_RULES);
        assert_eq!(hl[8], HlType::Type);

        // Inline comment after value
        let hl = hl_types(b"key = value ; comment", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword);
        assert_eq!(hl[6], HlType::Normal); // v
        assert_eq!(hl[12], HlType::Comment); // ;
        assert_eq!(hl[20], HlType::Comment);

        // Section header with inline comment
        let hl = hl_types(b"[section] ; comment", &INI_RULES);
        assert_eq!(hl[0], HlType::Keyword);
        assert_eq!(hl[8], HlType::Keyword);
        assert_eq!(hl[10], HlType::Comment);
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
        assert_eq!(hl[1], HlType::Keyword); // key is yellow
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

    // -- Semver highlighting ------------------------------------------------

    /// Helper: highlight multiple lines and return all per-byte highlights.
    fn hl_multiline(lines: &[&[u8]], rules: &SyntaxRules) -> Vec<Vec<HlType>> {
        let mut state = HlState::Normal;
        let mut result = Vec::new();
        for line in lines {
            let (hl, next) = highlight_line(line, state, rules);
            result.push(hl);
            state = next;
        }
        result
    }

    /// Helper: assert a byte range is a specific HlType.
    fn assert_range(hl: &[HlType], range: std::ops::Range<usize>, expected: HlType, label: &str) {
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
        assert_range(&hls[2], 11..16, HlType::Type, "version value");
        // line 6: serde = "1.0.197" — 1.0.197 at bytes 9..16
        assert_range(&hls[6], 9..16, HlType::Type, "serde version");
        // line 7: "1.36.0" — 1.36.0 inside the string
        let l7 = &hls[7];
        let s = b"tokio = { version = \"1.36.0\", features = [\"full\"] }";
        let ver_start = s.windows(5).position(|w| w == b"1.36.").unwrap();
        assert_range(l7, ver_start..ver_start + 6, HlType::Type, "tokio version");
        // line 3: "2021" is NOT semver (only one component)
        assert_ne!(hls[3][11], HlType::Type);
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
        assert_range(&hls[0], 0..25, HlType::Comment, "comment line");
        // line 1: "1.0.0+build.42" inside string — semver SHOULD override
        let l1 = &hls[1];
        // const VERSION: &str = "1.0.0+build.42"; — version at byte 23
        let ver_start = b"const VERSION: &str = \"".len();
        assert_range(
            l1,
            ver_start..ver_start + 14,
            HlType::Type,
            "version in string",
        );
        // line 2: "v = 1" — bare v is not semver
        assert_ne!(hls[2][4], HlType::Type);
        // line 3: "abc1.2.3" — preceded by alpha, not semver
        assert_ne!(hls[3][12], HlType::Type);
        // line 4: "v0.9.0" in string should be semver, "1.2.3x" should not
        let l4 = &hls[4];
        let s4 = b"println!(\"upgrade to v0.9.0 or 1.2.3x\");";
        let v_start = s4.windows(6).position(|w| w == b"v0.9.0").unwrap();
        assert_range(l4, v_start..v_start + 6, HlType::Type, "v0.9.0 in string");
        // 1.2.3x should not be Type (trailing x)
        let bad_start = s4.windows(5).position(|w| w == b"1.2.3").unwrap();
        assert_ne!(l4[bad_start], HlType::Type);
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
        assert_eq!(hls[0][10], HlType::Bracket); // (
        assert_eq!(hls[0][26], HlType::Bracket); // )
        assert_eq!(hls[0][28], HlType::Bracket); // { at end
        // line 1: ( and ) inside string should be String, not Bracket
        let l1 = &hls[1];
        // The string starts at the " and everything inside is String
        let paren_pos = b"    let s = \"(not a bracket)\";"
            .iter()
            .position(|&b| b == b'(')
            .unwrap();
        assert_eq!(l1[paren_pos], HlType::String);
        // line 2: { inside comment should be Comment (after leading whitespace)
        let comment_start = b"    ".len();
        assert_range(
            &hls[2],
            comment_start..hls[2].len(),
            HlType::Comment,
            "comment with brackets",
        );
        // line 3: [ at some position, { at end
        let l3 = &hls[3];
        let bracket_pos = b"    if items[0] > 0 {"
            .iter()
            .position(|&b| b == b'[')
            .unwrap();
        assert_eq!(l3[bracket_pos], HlType::Bracket);
        // line 6: } is bracket
        assert_eq!(hls[6][0], HlType::Bracket);
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
            hls[0].iter().all(|&h| h == HlType::Keyword),
            "header should be all Keyword"
        );

        // line 2: **bold** → Keyword, *italic* → Type, rest Normal
        let l2 = &hls[2];
        let bold_start = b"Some text with ".len();
        assert_range(l2, bold_start..bold_start + 8, HlType::Keyword, "bold");
        let italic_start = bold_start + 8 + " and ".len();
        assert_range(l2, italic_start..italic_start + 8, HlType::Type, "italic");

        // line 4: > marker is Comment, `inline code` is String
        assert_eq!(hls[4][0], HlType::Comment); // >
        let backtick = b"> A blockquote with ".len();
        assert_range(
            &hls[4],
            backtick..backtick + 13,
            HlType::String,
            "inline code",
        );

        // line 6-7: list markers — "- " is Number
        assert_eq!(hls[6][0], HlType::Number); // -
        assert_eq!(hls[6][1], HlType::Number); // space
        assert_eq!(hls[6][2], HlType::Normal); // f
        assert_eq!(hls[7][0], HlType::Number); // -

        // line 8: ordered list — "1. " is Number
        assert_range(&hls[8], 0..3, HlType::Number, "ordered marker");
        assert_eq!(hls[8][3], HlType::Normal);

        // line 10: horizontal rule — all Comment
        assert!(
            hls[10].iter().all(|&h| h == HlType::Comment),
            "hr should be Comment"
        );

        // line 12: fenced code open — all String, state enters FencedCodeBlock
        assert!(hls[12].iter().all(|&h| h == HlType::String), "fence open");
        // line 13: inside fenced block — all String
        assert!(
            hls[13].iter().all(|&h| h == HlType::String),
            "fenced content"
        );
        // line 14: fence close — all String
        assert!(hls[14].iter().all(|&h| h == HlType::String), "fence close");

        // line 16: HTML comment — all Comment
        assert!(
            hls[16].iter().all(|&h| h == HlType::Comment),
            "html comment"
        );
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
        assert!(hls[0].iter().all(|&h| h == HlType::Normal), "before");
        assert!(
            hls[1].iter().all(|&h| h == HlType::Comment),
            "comment start"
        );
        assert!(
            hls[2].iter().all(|&h| h == HlType::Comment),
            "comment middle"
        );
        // line 3: "end -->" is comment, " after" is normal
        let close_end = b"end -->".len();
        assert_range(&hls[3], 0..close_end, HlType::Comment, "comment end");
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
        assert_eq!(hls[0][0], HlType::Bracket);
        // line 1: "name" is Keyword (key), "my-app" is String (value)
        assert_range(&hls[1], 2..8, HlType::Keyword, "name key");
        assert_range(&hls[1], 10..18, HlType::String, "my-app value");
        // line 2: "version" is Keyword, "2.1.0" gets semver override
        assert_range(&hls[2], 2..11, HlType::Keyword, "version key");
        let ver_start = b"  \"version\": \"".len();
        assert_range(
            &hls[2],
            ver_start..ver_start + 5,
            HlType::Type,
            "semver 2.1.0",
        );
        // line 3: true is Type
        let true_start = b"  \"private\": ".len();
        assert_range(&hls[3], true_start..true_start + 4, HlType::Type, "true");
        // line 4: "dependencies" key, { bracket
        assert_eq!(hls[4][2], HlType::Keyword); // "
        let brace = hls[4].len() - 1;
        assert_eq!(hls[4][brace], HlType::Bracket);
        // line 5: nested key "react", semver value "18.2.0"
        assert_eq!(hls[5][4], HlType::Keyword);
        let react_ver = b"    \"react\": \"".len();
        assert_range(
            &hls[5],
            react_ver..react_ver + 6,
            HlType::Type,
            "react semver",
        );
        // line 8: 42 is Number
        let num_start = b"  \"count\": ".len();
        assert_range(&hls[8], num_start..num_start + 2, HlType::Number, "42");
        // line 9: [ and ] are brackets, string values
        assert_eq!(hls[9][b"  \"tags\": ".len()], HlType::Bracket); // [
        // line 10: null is Type
        let null_start = b"  \"nullable\": ".len();
        assert_range(&hls[10], null_start..null_start + 4, HlType::Type, "null");
        // line 11: } is Bracket
        assert_eq!(hls[11][0], HlType::Bracket);
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
        assert_range(&hls[0], 0..4, HlType::Keyword, "name key");
        assert_eq!(hls[0][6], HlType::Normal);
        // line 1: "version" Keyword, "1.5.0" semver
        assert_range(&hls[1], 0..7, HlType::Keyword, "version key");
        assert_range(&hls[1], 9..14, HlType::Type, "semver 1.5.0");
        // line 2: "false" is Type
        assert_range(&hls[2], 7..12, HlType::Type, "false");
        // line 3: 8080 is Number
        assert_range(&hls[3], 6..10, HlType::Number, "8080");
        // line 4: "localhost" is String (quoted)
        assert_range(&hls[4], 6..17, HlType::String, "quoted value");
        // line 5: "database" is Keyword, no value
        assert_range(&hls[5], 0..8, HlType::Keyword, "database key");
        // line 6: nested key "url", quoted string value
        assert_range(&hls[6], 2..5, HlType::Keyword, "url key");
        assert_eq!(hls[6][7], HlType::String);
        // line 7: "pool_size" key, 10 number
        assert_range(&hls[7], 2..11, HlType::Keyword, "pool_size key");
        assert_range(&hls[7], 13..15, HlType::Number, "10");
        // line 8: "defaults" key, &defaults anchor
        assert_range(&hls[8], 0..8, HlType::Keyword, "defaults key");
        // line 11: *defaults alias
        let l11 = &hls[11];
        let alias_start = b"  <<: ".len();
        assert_eq!(l11[alias_start], HlType::Type); // *
        // line 13: key then # comment
        assert_range(&hls[13], 0..4, HlType::Keyword, "tags key");
        let comment_start = b"tags: ".len();
        assert_range(
            &hls[13],
            comment_start..hls[13].len(),
            HlType::Comment,
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

        // Opening { on line 0 col 48 → closing } on line 8 col 0
        let open_brace = lines[0].iter().rposition(|&b| b == b'{').unwrap();
        let result = find_bracket_match(
            Pos::new(0, open_brace),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(Pos::new(8, 0)));

        // Closing } on line 8 → back to opening { on line 0
        let result = find_bracket_match(
            Pos::new(8, 0),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(Pos::new(0, open_brace)));

        // Inner if { on line 1 → } on line 3
        let if_brace = lines[1].iter().rposition(|&b| b == b'{').unwrap();
        let result = find_bracket_match(
            Pos::new(1, if_brace),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(Pos::new(3, 4)));

        // ( on line 0 col 10 → ) matching
        let result = find_bracket_match(
            Pos::new(0, 10),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(Pos::new(0, 24)));

        // Nested (()) on line 7: Ok(()) — outer ( matches outer )
        let ok_paren = lines[7].iter().position(|&b| b == b'(').unwrap();
        let result = find_bracket_match(
            Pos::new(7, ok_paren),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, Some(Pos::new(7, ok_paren + 3)));

        // Cursor on non-bracket char → None
        let result = find_bracket_match(
            Pos::new(0, 0),
            &mut |i, b| *b = get(i),
            &mut scratch,
            line_count,
        );
        assert_eq!(result, None);

        // Unmatched: if we only pass first line, { has no match
        let result = find_bracket_match(
            Pos::new(0, open_brace),
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
            find_quote_match(Pos::new(0, 0), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 6))
        );
        // Cursor on closing " → match opening
        assert_eq!(
            find_quote_match(Pos::new(0, 6), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 0))
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
            find_quote_match(Pos::new(0, 0), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 6))
        );
        assert_eq!(
            find_quote_match(Pos::new(0, 6), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 0))
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
            find_quote_match(Pos::new(0, 0), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 8))
        );
        assert_eq!(
            find_quote_match(Pos::new(0, 8), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 0))
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
            find_quote_match(Pos::new(0, 0), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 2))
        );
        assert_eq!(
            find_quote_match(Pos::new(0, 2), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 0))
        );
        // Second pair: 4↔6
        assert_eq!(
            find_quote_match(Pos::new(0, 4), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 6))
        );
        assert_eq!(
            find_quote_match(Pos::new(0, 6), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 4))
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
            find_quote_match(Pos::new(0, 4), &mut get, &mut scratch, 1),
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
            find_quote_match(Pos::new(0, 2), &mut get, &mut scratch, 1),
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
            find_quote_match(Pos::new(0, 2), &mut get, &mut scratch, 1),
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
            find_quote_match(Pos::new(0, 0), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 3))
        );
        assert_eq!(
            find_quote_match(Pos::new(0, 3), &mut get, &mut scratch, 1),
            Some(Pos::new(0, 0))
        );
    }

    // -- INI section edge cases -----------------------------------------------

    #[test]
    fn test_ini_empty_value() {
        let hl = hl_types(b"key =", &INI_RULES);
        assert_range(&hl, 0..3, HlType::Keyword, "ini key");
    }

    #[test]
    fn test_ini_no_equals() {
        let hl = hl_types(b"just text", &INI_RULES);
        // Without = sign, this isn't a key=value pair
        assert!(hl.iter().all(|&h| h != HlType::Keyword));
    }

    // -- YAML edge cases ------------------------------------------------------

    #[test]
    fn test_yaml_multiline_string() {
        let lines: &[&[u8]] = &[b"description: |", b"  multi line", b"  text here"];
        let hls = hl_multiline(lines, &YAML_RULES);
        assert_range(&hls[0], 0..11, HlType::Keyword, "description key");
    }

    #[test]
    fn test_yaml_empty_value() {
        let hl = hl_types(b"key:", &YAML_RULES);
        assert_range(&hl, 0..3, HlType::Keyword, "yaml key");
    }

    // -- Markdown edge cases --------------------------------------------------

    #[test]
    fn test_markdown_fenced_code_block_with_language() {
        let lines: &[&[u8]] = &[b"```rust", b"fn main() {}", b"```"];
        let hls = hl_multiline(lines, &MARKDOWN_RULES);
        assert!(
            hls[0].iter().all(|&h| h == HlType::String),
            "fence open with lang"
        );
        assert!(
            hls[1].iter().all(|&h| h == HlType::String),
            "fenced content"
        );
        assert!(hls[2].iter().all(|&h| h == HlType::String), "fence close");
    }

    #[test]
    fn test_markdown_blockquote() {
        let hl = hl_types(b"> quoted text", &MARKDOWN_RULES);
        // Blockquote marker should be highlighted as Comment
        assert_eq!(hl[0], HlType::Comment);
    }

    // -- rules_for_language ---------------------------------------------------

    #[test]
    fn test_rules_for_language_known() {
        assert!(rules_for_language("Rust").is_some());
        assert!(rules_for_language("Python").is_some());
        assert!(rules_for_language("Config").is_some());
    }

    #[test]
    fn test_rules_for_language_unknown() {
        assert!(rules_for_language("Brainfuck").is_none());
    }

    // -- byte_hl_to_char_hl with multi-byte chars -----------------------------

    #[test]
    fn test_byte_hl_to_char_hl_multibyte() {
        // "é" is 2 bytes → 1 char highlight
        let text = "é".as_bytes();
        let byte_hl = vec![HlType::String; text.len()];
        let char_hl = byte_hl_to_char_hl(text, &byte_hl);
        assert_eq!(char_hl.len(), 1);
        assert_eq!(char_hl[0], HlType::String);
    }

    // -- Coverage gap: ansi_code for all HlType variants (lines 61-65) --------

    #[test]
    fn test_ansi_code_all_variants() {
        assert_eq!(HlType::Normal.ansi_code(), "");
        assert!(!HlType::Comment.ansi_code().is_empty());
        assert!(!HlType::Keyword.ansi_code().is_empty());
        assert!(!HlType::Type.ansi_code().is_empty());
        assert!(!HlType::String.ansi_code().is_empty());
        assert!(!HlType::Number.ansi_code().is_empty());
        assert!(!HlType::Bracket.ansi_code().is_empty());
        assert!(!HlType::Operator.ansi_code().is_empty());
        assert!(!HlType::Function.ansi_code().is_empty());
        assert!(!HlType::Constant.ansi_code().is_empty());
        assert!(!HlType::Macro.ansi_code().is_empty());
    }

    // -- Coverage gap: multiline string continuation (lines 166-169, 184-185) --

    #[test]
    fn test_multiline_string_continuation() {
        let rules = rules_for_language("Python").unwrap();
        // Start a triple-quoted string that doesn't close
        let line1 = b"x = \"\"\"hello";
        let (_hl1, state1) = highlight_line(line1, HlState::Normal, rules);
        assert!(matches!(state1, HlState::MultiLineString(_)));
        // Continuation line with escape
        let line2 = b"world \\n more";
        let (hl2, state2) = highlight_line(line2, state1, rules);
        // All characters should be string
        assert_eq!(hl2[0], HlType::String);
        assert!(matches!(state2, HlState::MultiLineString(_)));
        // Closing line
        let line3 = b"end\"\"\"";
        let (_hl3, state3) = highlight_line(line3, state2, rules);
        assert_eq!(state3, HlState::Normal);
    }

    // -- Coverage gap: unclosed non-multiline string (line 266) ---------------

    #[test]
    fn test_unclosed_string_single_line() {
        let rules = rules_for_language("Rust").unwrap();
        let line = b"let s = \"unterminated";
        let (hl, state) = highlight_line(line, HlState::Normal, rules);
        // The string characters should be highlighted as String
        assert_eq!(hl[8], HlType::String); // opening quote
        assert_eq!(state, HlState::Normal);
    }

    // -- Coverage gap: float starting with dot (line 330) ---------------------

    #[test]
    fn test_number_starting_with_dot() {
        let rules = rules_for_language("Rust").unwrap();
        let line = b"let x = .5;";
        let (hl, _) = highlight_line(line, HlState::Normal, rules);
        assert_eq!(hl[8], HlType::Number); // .
        assert_eq!(hl[9], HlType::Number); // 5
    }

    // -- Coverage gap: semver pre-release (lines 433-436) ---------------------

    #[test]
    fn test_semver_pre_release() {
        let rules = rules_for_language("TOML").unwrap();
        let line = b"version = \"1.2.3-beta.1\"";
        let (hl, _) = highlight_line(line, HlState::Normal, rules);
        // The version inside quotes should be Type (cyan/semver)
        assert_eq!(hl[11], HlType::Type); // '1' of version
    }

    // -- Coverage gap: YAML anchor/alias (lines 621-629) ----------------------

    #[test]
    fn test_yaml_anchor() {
        let line = b"&my_anchor";
        let mut hl = vec![HlType::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        assert_eq!(hl[0], HlType::Type); // '&'
        assert_eq!(hl[1], HlType::Type); // 'm'
    }

    #[test]
    fn test_yaml_alias() {
        let line = b"*my_alias";
        let mut hl = vec![HlType::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        assert_eq!(hl[0], HlType::Type);
    }

    // -- Coverage gap: YAML list item with key:value (lines 655-661) ----------

    #[test]
    fn test_yaml_list_item_with_key() {
        let line = b"- name: value";
        let mut hl = vec![HlType::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        assert_eq!(hl[2], HlType::Keyword); // 'n' of name
        assert_eq!(hl[5], HlType::Keyword); // 'e' of name
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
        ];
        for lang in languages {
            let rules = rules_for_language(lang).unwrap();
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
        let mut hl = vec![HlType::Normal; line.len()];
        highlight_yaml_value(line, &mut hl);
        assert_eq!(hl[2], HlType::Number); // '-'
        assert_eq!(hl[3], HlType::Number); // '4'
    }

    // -- Coverage gap: find_yaml_colon with quoted colon (lines 675-680) ------

    #[test]
    fn test_yaml_colon_in_quotes() {
        let line = b"\"key:with:colons\": value";
        let mut hl = vec![HlType::Normal; line.len()];
        highlight_yaml_content(line, &mut hl);
        // The colon inside quotes should not split key/value
        // The actual key ends at the colon after the closing quote
        assert_eq!(hl[0], HlType::Keyword);
    }
}
