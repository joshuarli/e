use hi_lite::{
    Highlighter, Kind, Language, Run, State, byte_kinds_to_char_kinds, highlight_line_into, runs,
};

#[test]
fn public_api_streams_opaque_state_and_reuses_output() {
    let mut highlighter = Highlighter::new(Language::Rust);
    let mut output = Vec::new();

    let first = highlighter.highlight_into(b"/* start", &mut output);
    assert!(first.iter().all(|&kind| kind == Kind::Comment));
    assert!(!highlighter.state().is_normal());

    let second = highlighter.highlight_into(b"end */ let", &mut output);
    assert_eq!(second[8], Kind::Keyword);
    assert!(highlighter.state().is_normal());

    highlighter.reset();
    assert_eq!(highlighter.state(), State::default());
}

#[test]
fn public_api_clears_reused_slots_between_equal_length_lines() {
    let mut highlighter = Highlighter::new(Language::Rust);
    let mut output = Vec::new();

    let first = highlighter.highlight_into(b"//x", &mut output);
    assert!(first.iter().all(|&kind| kind == Kind::Comment));

    let second = highlighter.highlight_into(b"let", &mut output);
    assert!(second.iter().all(|&kind| kind == Kind::Keyword));
}

#[test]
fn public_api_supports_stateless_lines_runs_and_char_mapping() {
    let line = b"let x = 1";
    let mut kinds = vec![Kind::Normal; line.len()];
    assert!(highlight_line_into(Language::Rust, State::default(), line, &mut kinds).is_normal());
    assert_eq!(kinds[0], Kind::Keyword);

    let grouped: Vec<_> = runs(&[Kind::Keyword, Kind::Keyword, Kind::Normal]).collect();
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
        ]
    );

    let raw = "a\té".as_bytes();
    let byte_kinds = vec![Kind::Comment; raw.len()];
    let chars = byte_kinds_to_char_kinds(raw, &byte_kinds);
    assert_eq!(chars.len(), 4);
}

#[test]
fn language_aliases_are_small_and_explicit() {
    assert_eq!(Language::from_name("rs"), Some(Language::Rust));
    assert_eq!(Language::from_name("shell"), Some(Language::Bash));
    assert_eq!(Language::from_extension(".md"), Some(Language::Markdown));
    assert_eq!(Language::from_name("unknown"), None);
}

#[test]
fn language_detection_owns_filename_and_shebang_mapping() {
    assert_eq!(Language::from_filename("src/main.rs"), Some(Language::Rust));
    assert_eq!(
        Language::from_filename("templates/page.css.erb"),
        Some(Language::Css)
    );
    assert_eq!(
        Language::from_filename("Dockerfile.release"),
        Some(Language::Dockerfile)
    );
    assert_eq!(Language::from_filename("GNUmakefile"), Some(Language::Makefile));
    assert_eq!(Language::from_filename("build.xsh"), Some(Language::Xsh));
    assert_eq!(Language::from_filename("README.txt"), None);

    assert_eq!(
        Language::from_shebang(b"#!/usr/bin/python3.11"),
        Some(Language::Python)
    );
    assert_eq!(
        Language::from_shebang(b"#!/usr/bin/env -S python3 -O"),
        Some(Language::Python)
    );
    assert_eq!(
        Language::from_shebang(b"#!/usr/bin/node"),
        Some(Language::JavaScript)
    );
    assert_eq!(
        Language::from_shebang(b"#!/usr/bin/env xshi"),
        Some(Language::Xsh)
    );
    assert_eq!(Language::from_shebang(b"not a shebang"), None);
    assert_eq!(
        Language::detect(Some("unknown.file"), b"#!/bin/bash"),
        Some(Language::Bash)
    );
    assert_eq!(
        Language::detect(Some("main.rs"), b"#!/bin/bash"),
        Some(Language::Rust)
    );
}

#[test]
fn language_comment_delimiters_are_canonical() {
    assert_eq!(Language::Rust.comment(), "//");
    assert_eq!(Language::Python.comment(), "#");
    assert_eq!(Language::Html.comment(), "<!--");
    assert_eq!(Language::Markdown.comment(), "<!--");
    assert_eq!(Language::Css.comment(), "/*");
    assert_eq!(Language::PlainText.comment(), "");
}

#[test]
fn syntect_programming_syntax_names_have_dependency_free_lexers() {
    // Keep this list aligned with the programming grammars in syntect's
    // default SyntaxSet. Embedded/template syntaxes intentionally resolve to
    // their reusable host lexer (for example JSP to HTML and SQL (Rails) to
    // SQL).
    let names = [
        "Plain Text", "ASP", "HTML (ASP)", "ActionScript", "AppleScript", "Batch File",
        "NAnt Build File", "C#", "C++", "C", "Clojure", "D", "Diff", "Erlang",
        "HTML (Erlang)", "Go", "Graphviz (DOT)", "Groovy", "HTML", "Haskell",
        "Literate Haskell", "Java Server Page (JSP)", "Java", "JavaDoc", "JSON",
        "Regular Expressions (Javascript)", "BibTeX", "LaTeX Log", "LaTeX", "TeX",
        "Lisp", "Lua", "Make Output", "Makefile", "Markdown", "MultiMarkdown", "MATLAB",
        "OCaml", "OCamllex", "OCamlyacc", "camlp4", "Objective-C++", "Objective-C",
        "PHP Source", "PHP", "Pascal", "Perl", "Python", "Regular Expressions (Python)",
        "R Console", "R", "Rd (R Documentation)", "HTML (Rails)", "JavaScript (Rails)",
        "Ruby Haml", "Ruby on Rails", "SQL (Rails)", "Regular Expression",
        "reStructuredText", "Ruby", "Cargo Build Results", "Rust", "SQL", "Scala",
        "Bourne Again Shell (bash)", "Shell-Unix-Generic", "commands-builtin-shell-bash",
        "HTML (Tcl)", "Tcl", "Textile", "XML", "YAML",
    ];
    for name in names {
        assert!(Language::from_name(name).is_some(), "missing syntax: {name}");
    }
}

#[test]
fn generic_language_families_share_the_common_scanner() {
    let cases = [
        (Language::Java, b"public int answer() { return 42; }".as_slice(), 0, Kind::Keyword),
        (Language::Ruby, b"value = 42 # comment".as_slice(), 8, Kind::Number),
        (Language::Haskell, b"value = 42 -- comment".as_slice(), 8, Kind::Number),
        (Language::Sql, b"select count from users where id = 42".as_slice(), 0, Kind::Keyword),
    ];
    for (language, line, offset, expected) in cases {
        let mut highlighter = Highlighter::new(language);
        let mut scratch = Vec::new();
        let kinds = highlighter.highlight_into(line, &mut scratch);
        assert_eq!(kinds[offset], expected, "{}: {line:?}", language.name());
    }
}

#[test]
fn every_public_language_round_trips_its_canonical_name() {
    let languages = [
        Language::Rust, Language::Python, Language::Go, Language::JavaScript,
        Language::TypeScript, Language::Bash, Language::C, Language::Cpp,
        Language::CSharp, Language::Json, Language::Yaml, Language::Toml,
        Language::Ini, Language::Makefile, Language::Html, Language::Css,
        Language::Scss, Language::Less, Language::Dockerfile, Language::Markdown,
        Language::Xml, Language::ActionScript, Language::AppleScript, Language::Batch,
        Language::Clojure, Language::D, Language::Erlang, Language::Graphviz,
        Language::Groovy, Language::Haskell, Language::Java, Language::LaTeX,
        Language::Lisp, Language::Lua, Language::Matlab, Language::Ocaml,
        Language::ObjectiveC, Language::ObjectiveCpp, Language::Pascal, Language::Perl,
        Language::Php, Language::R, Language::Ruby, Language::Scala, Language::Sql,
        Language::Swift, Language::Tcl, Language::Kotlin, Language::Elm, Language::Regex,
        Language::PlainText, Language::Xsh,
    ];
    for language in languages {
        assert_eq!(Language::from_name(language.name()), Some(language));
    }
}

#[test]
fn syntect_extensions_resolve_to_host_or_generic_lexers() {
    let cases = [
        (".cpp", Language::Cpp),
        (".csx", Language::CSharp),
        ("GNUmakefile", Language::Makefile),
        (".txt", Language::PlainText),
        (".asa", Language::Html),
        (".gradle", Language::Groovy),
        (".lhs", Language::Haskell),
        (".jsp", Language::Html),
        (".mli", Language::Ocaml),
        (".mm", Language::ObjectiveCpp),
        (".phtml", Language::Php),
        (".rst", Language::Markdown),
        (".sql.erb", Language::Sql),
        (".svg", Language::Xml),
        (".textile", Language::Markdown),
    ];
    for (extension, language) in cases {
        assert_eq!(Language::from_extension(extension), Some(language), "{extension}");
    }
}
