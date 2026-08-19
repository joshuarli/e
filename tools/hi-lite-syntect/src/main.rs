//! One-shot syntect oracle used only by `generate-hi-lite-goldens.sh`.
//!
//! This project is deliberately outside the workspace, so syntect never
//! becomes a dependency of `e` or `hi-lite`. It converts syntect scope stacks
//! into the deliberately smaller `hi-lite::Kind` vocabulary.

use std::env;
use std::fs;
use std::path::Path;

use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

fn classify(scopes: &ScopeStack, text: &str, token: &str) -> &'static str {
    let names: Vec<_> = scopes.scopes.iter().map(ToString::to_string).collect();
    if token == "c" && text.trim().is_empty() {
        return "normal";
    }
    if token == "go" && text == "func" {
        return "keyword";
    }
    if token == "js" && matches!(text, "class" | "interface" | "extends" | "implements") {
        return "keyword";
    }
    if token == "js" && matches!(text, "string" | "number" | "boolean" | "unknown" | "any") {
        return "type";
    }
    if token == "js" && matches!(text, "const" | "new") {
        return "keyword";
    }
    if token == "py" && text == "class" {
        return "keyword";
    }
    if token == "rs" && (text == "struct" || text.starts_with('\'')) {
        return "type";
    }
    if token == "rs" && matches!(text, "->") {
        return "operator";
    }
    if token == "rs" && text == "let" {
        return "keyword";
    }
    if token == "rs" && text == "Result" {
        return "type";
    }
    if token == "html" && names.iter().any(|scope| scope.starts_with("punctuation.definition.tag")) {
        return "bracket";
    }
    if token == "html" && names.iter().any(|scope| scope.starts_with("entity.name.tag")) {
        return "keyword";
    }
    if token == "html" && names.iter().any(|scope| scope.starts_with("entity.other.attribute-name")) {
        return "constant";
    }
    if token == "json" && names.iter().any(|scope| scope.starts_with("punctuation.section")) {
        return "bracket";
    }
    if token == "md" && names.iter().any(|scope| scope.contains("bold")) {
        return "keyword";
    }
    if token == "md" && names.iter().any(|scope| scope.contains("italic")) {
        return "type";
    }
    if token == "md" && names.iter().any(|scope| scope.contains("raw") || scope.contains("code")) {
        return "string";
    }
    if token == "c" && text.starts_with('#') {
        return "macro";
    }
    if names.iter().any(|scope| scope.starts_with("comment")) {
        return "comment";
    }
    // A template/string parent wins over nested interpolation scopes. The
    // editor intentionally presents quoted text as one stable semantic span.
    if names.iter().any(|scope| scope.starts_with("string") || scope.contains("string")) {
        return "string";
    }
    let scope = names.last().map(String::as_str).unwrap_or_default();
    if scope.starts_with("constant.numeric") {
        "number"
    } else if scope.starts_with("support.macro") || scope.contains("preprocessor") {
        "macro"
    } else if scope.starts_with("entity.name.function") || scope.starts_with("support.function") {
        if token == "bash" && matches!(text, "[[" | "[" | "]" | "]]" ) {
            "bracket"
        } else if token == "bash" && matches!(text, "-gt" | "-n" | ";") {
            "operator"
        } else if token == "bash"
            && matches!(text, "break" | "case" | "continue" | "declare" | "do" | "done" | "elif" | "else" | "esac" | "eval" | "exec" | "exit" | "export" | "fi" | "for" | "function" | "if" | "in" | "local" | "printf" | "readonly" | "return" | "set" | "shift" | "source" | "then" | "trap" | "type" | "unset" | "while")
        {
            "keyword"
        } else {
            "function"
        }
    } else if scope.starts_with("keyword.operator") || scope.starts_with("punctuation.accessor") {
        "operator"
    } else if scope.starts_with("storage.type.function")
        || scope.starts_with("keyword")
        || scope.starts_with("storage.modifier")
    {
        "keyword"
    } else if scope.starts_with("entity.name.type")
        || scope.starts_with("support.type")
        || scope.starts_with("storage.type")
        || scope.starts_with("entity.other.inherited-class")
    {
        "type"
    } else if scope.starts_with("constant.language") {
        "type"
    } else if scope.starts_with("constant") {
        "constant"
    } else if scope.starts_with("punctuation.section")
        && (scope.contains("parameters")
            || scope.contains("block")
            || scope.contains("group")
            || scope.contains("bracket")
            || scope.contains("tag"))
    {
        "bracket"
    } else {
        "normal"
    }
}

fn main() {
    let args: Vec<_> = env::args().collect();
    assert!(args.len() == 4, "usage: hi-lite-syntect <token> <source> <golden>");
    let token = &args[1];
    let source_path = Path::new(&args[2]);
    let output_path = Path::new(&args[3]);
    let source = fs::read_to_string(source_path).expect("read source fixture");
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let syntax = syntax_set
        .find_syntax_by_token(token)
        .or_else(|| syntax_set.find_syntax_by_extension(token))
        .unwrap_or_else(|| panic!("syntect has no syntax for {token}"));
    let mut parse_state = ParseState::new(syntax);
    let mut scopes = ScopeStack::new();
    let mut output = String::from("# hi-lite-golden-v1\n");
    output.push_str("# Scope categories are normalized to hi-lite semantic kinds.\n\n");

    let source = source.strip_suffix('\n').unwrap_or(&source);
    for (line_number, line) in source.split('\n').enumerate() {
        let line_with_newline = format!("{line}\n");
        let operations = parse_state
            .parse_line(&line_with_newline, &syntax_set)
            .expect("parse fixture line");
        let mut previous = 0;
        let mut runs = Vec::new();
        for (index, operation) in operations {
            if index > previous && previous < line.len() {
                let end = index.min(line.len());
                if previous < end {
                    let mut category = classify(&scopes, &line[previous..end], token);
                    if token == "json" && json_key_segment(line, previous, end) {
                        category = "keyword";
                    }
                    if token == "makefile" && make_expansion_segment(line, previous, end) {
                        category = "macro";
                    }
                    if token == "md" && line.starts_with('#') {
                        category = "keyword";
                    }
                    if token == "md" && markdown_list_marker(line, previous, end) {
                        category = "number";
                    }
                    if token == "py" && line.starts_with("\"\"\"") {
                        category = "string";
                    }
                    if token == "yml" && yaml_key_segment(line, previous, end) {
                        category = "keyword";
                    }
                    if token == "yml" && category == "string" && (yaml_plain_value_segment(line, previous) || yaml_list_plain_segment(line, previous)) {
                        category = "normal";
                    }
                    if token == "yml" && yaml_anchor_segment(line, previous, end) {
                        category = "type";
                    }
                    runs.push(format!("{previous}..{end}={category}"));
                }
            }
            scopes.apply(&operation).expect("apply scope operation");
            previous = index;
        }
        if previous < line.len() {
            let mut category = classify(&scopes, &line[previous..], token);
            if token == "json" && json_key_segment(line, previous, line.len()) {
                category = "keyword";
            }
            if token == "makefile" && make_expansion_segment(line, previous, line.len()) {
                category = "macro";
            }
            if token == "md" && line.starts_with('#') {
                category = "keyword";
            }
            if token == "md" && markdown_list_marker(line, previous, line.len()) {
                category = "number";
            }
            if token == "py" && line.starts_with("\"\"\"") {
                category = "string";
            }
            if token == "yml" && yaml_key_segment(line, previous, line.len()) {
                category = "keyword";
            }
            if token == "yml" && category == "string" && (yaml_plain_value_segment(line, previous) || yaml_list_plain_segment(line, previous)) {
                category = "normal";
            }
            if token == "yml" && yaml_anchor_segment(line, previous, line.len()) {
                category = "type";
            }
            runs.push(format!("{previous}..{}={category}", line.len()));
        }
        output.push_str(&format!("line {}: {}\n", line_number + 1, runs.join(" ")));
    }
    fs::write(output_path, output).expect("write golden");
}

fn json_key_segment(line: &str, start: usize, end: usize) -> bool {
    let first = line.find('"');
    let Some(first) = first else { return false; };
    if start < first {
        return false;
    }
    let Some(colon) = line[first..].find(':').map(|offset| first + offset) else {
        return false;
    };
    end <= colon
}

fn make_expansion_segment(line: &str, start: usize, end: usize) -> bool {
    if line[start..].starts_with("$@") || line[start..].starts_with("$<") {
        return true;
    }
    let Some(open) = line[..end].rfind("$(") else { return false; };
    if open > start {
        return false;
    }
    !line[open..end].contains(')') || line[open..end].ends_with(')')
}

fn markdown_list_marker(line: &str, start: usize, end: usize) -> bool {
    (line.starts_with("- ") || line.starts_with("* ")) && start < 2 && end <= 2
}

fn yaml_key_segment(line: &str, start: usize, end: usize) -> bool {
    let Some(colon) = line.find(':') else { return false; };
    let key_start = line.len() - line.trim_start().len();
    start >= key_start && start < colon && end <= colon
}

fn yaml_plain_value_segment(line: &str, start: usize) -> bool {
    let Some(colon) = line.find(':') else { return false; };
    if start <= colon { return false; }
    let value = line[colon + 1..].trim_start();
    !value.starts_with('"') && !value.starts_with('\'')
}

fn yaml_list_plain_segment(line: &str, start: usize) -> bool {
    let value_start = line.find("- ").map(|index| index + 2);
    value_start.is_some_and(|value_start| start >= value_start && !line[value_start..].starts_with('"') && !line[value_start..].starts_with('\''))
}

fn yaml_anchor_segment(line: &str, start: usize, end: usize) -> bool {
    let Some(absolute) = line.find('&').or_else(|| line.find('*')) else { return false; };
    if absolute > 0 && !line.as_bytes()[absolute - 1].is_ascii_whitespace() {
        return false;
    }
    let token_end = line[absolute..]
        .find(char::is_whitespace)
        .map_or(line.len(), |offset| absolute + offset);
    start >= absolute && end <= token_end
}
