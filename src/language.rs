//! Application language detection and comment syntax.
//!
//! `hi-lite::Language` is the canonical lexer selection type. This module is
//! intentionally narrower: filename/shebang matching and comment insertion are
//! editor policy, including entries for languages that have no `hi-lite` lexer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedLanguage {
    pub name: &'static str,
    pub comment: &'static str,
}

impl DetectedLanguage {
    /// Return the reusable lexer language when `hi-lite` has rules for it.
    pub fn syntax(self) -> Option<hi_lite::Language> {
        hi_lite::Language::from_name(self.name)
    }
}

const LANGUAGES: &[(&[&str], DetectedLanguage)] = &[
    (
        &[".rs"],
        DetectedLanguage {
            name: "Rust",
            comment: "//",
        },
    ),
    (
        &[".c", ".h"],
        DetectedLanguage {
            name: "C",
            comment: "//",
        },
    ),
    (
        &[".cpp", ".cc", ".cxx", ".hpp", ".hxx"],
        DetectedLanguage {
            name: "C++",
            comment: "//",
        },
    ),
    (
        &[".go"],
        DetectedLanguage {
            name: "Go",
            comment: "//",
        },
    ),
    (
        &[".js", ".jsx", ".mjs"],
        DetectedLanguage {
            name: "JavaScript",
            comment: "//",
        },
    ),
    (
        &[".ts", ".tsx"],
        DetectedLanguage {
            name: "TypeScript",
            comment: "//",
        },
    ),
    (
        &[".java"],
        DetectedLanguage {
            name: "Java",
            comment: "//",
        },
    ),
    (
        &[".cs"],
        DetectedLanguage {
            name: "C#",
            comment: "//",
        },
    ),
    (
        &[".swift"],
        DetectedLanguage {
            name: "Swift",
            comment: "//",
        },
    ),
    (
        &[".kt", ".kts"],
        DetectedLanguage {
            name: "Kotlin",
            comment: "//",
        },
    ),
    (
        &[".scala"],
        DetectedLanguage {
            name: "Scala",
            comment: "//",
        },
    ),
    (
        &[".py", ".pyi"],
        DetectedLanguage {
            name: "Python",
            comment: "#",
        },
    ),
    (
        &[".rb"],
        DetectedLanguage {
            name: "Ruby",
            comment: "#",
        },
    ),
    (
        &[".sh", ".bash", ".zsh", ".fish"],
        DetectedLanguage {
            name: "Shell",
            comment: "#",
        },
    ),
    (
        &[".pl", ".pm"],
        DetectedLanguage {
            name: "Perl",
            comment: "#",
        },
    ),
    (
        &[".r"],
        DetectedLanguage {
            name: "R",
            comment: "#",
        },
    ),
    (
        &[".json"],
        DetectedLanguage {
            name: "JSON",
            comment: "",
        },
    ),
    (
        &[".yaml", ".yml"],
        DetectedLanguage {
            name: "YAML",
            comment: "#",
        },
    ),
    (
        &[".toml"],
        DetectedLanguage {
            name: "TOML",
            comment: "#",
        },
    ),
    (
        &[".conf", ".cfg", ".ini"],
        DetectedLanguage {
            name: "Config",
            comment: "#",
        },
    ),
    (
        &[".lua"],
        DetectedLanguage {
            name: "Lua",
            comment: "--",
        },
    ),
    (
        &[".sql"],
        DetectedLanguage {
            name: "SQL",
            comment: "--",
        },
    ),
    (
        &[".hs"],
        DetectedLanguage {
            name: "Haskell",
            comment: "--",
        },
    ),
    (
        &[".elm"],
        DetectedLanguage {
            name: "Elm",
            comment: "--",
        },
    ),
    (
        &[".html", ".htm"],
        DetectedLanguage {
            name: "HTML",
            comment: "<!--",
        },
    ),
    (
        &[".xml", ".svg"],
        DetectedLanguage {
            name: "XML",
            comment: "<!--",
        },
    ),
    (
        &[".css"],
        DetectedLanguage {
            name: "CSS",
            comment: "/*",
        },
    ),
    (
        &[".scss", ".sass"],
        DetectedLanguage {
            name: "SCSS",
            comment: "//",
        },
    ),
    (
        &[".less"],
        DetectedLanguage {
            name: "Less",
            comment: "//",
        },
    ),
    (
        &[".php"],
        DetectedLanguage {
            name: "PHP",
            comment: "//",
        },
    ),
    (
        &[".ex", ".exs"],
        DetectedLanguage {
            name: "Elixir",
            comment: "#",
        },
    ),
    (
        &[".erl", ".hrl"],
        DetectedLanguage {
            name: "Erlang",
            comment: "%",
        },
    ),
    (
        &[".clj", ".cljs"],
        DetectedLanguage {
            name: "Clojure",
            comment: ";;",
        },
    ),
    (
        &[".lisp", ".cl", ".el"],
        DetectedLanguage {
            name: "Lisp",
            comment: ";;",
        },
    ),
    (
        &[".vim"],
        DetectedLanguage {
            name: "Vim",
            comment: "\"",
        },
    ),
    (
        &[".zig"],
        DetectedLanguage {
            name: "Zig",
            comment: "//",
        },
    ),
    (
        &[".d"],
        DetectedLanguage {
            name: "D",
            comment: "//",
        },
    ),
    (
        &[".dart"],
        DetectedLanguage {
            name: "Dart",
            comment: "//",
        },
    ),
    (
        &[".m"],
        DetectedLanguage {
            name: "Objective-C",
            comment: "//",
        },
    ),
    (
        &[".v"],
        DetectedLanguage {
            name: "V",
            comment: "//",
        },
    ),
    (
        &[".nim"],
        DetectedLanguage {
            name: "Nim",
            comment: "#",
        },
    ),
    (
        &[".cr"],
        DetectedLanguage {
            name: "Crystal",
            comment: "#",
        },
    ),
    (
        &[".jl"],
        DetectedLanguage {
            name: "Julia",
            comment: "#",
        },
    ),
    (
        &[".tf"],
        DetectedLanguage {
            name: "Terraform",
            comment: "#",
        },
    ),
    (
        &["Makefile", "makefile", "GNUmakefile"],
        DetectedLanguage {
            name: "Makefile",
            comment: "#",
        },
    ),
    (
        &["Dockerfile"],
        DetectedLanguage {
            name: "Dockerfile",
            comment: "#",
        },
    ),
    (
        &[".cmake"],
        DetectedLanguage {
            name: "CMake",
            comment: "#",
        },
    ),
    (
        &[".proto"],
        DetectedLanguage {
            name: "Protobuf",
            comment: "//",
        },
    ),
    (
        &[".graphql", ".gql"],
        DetectedLanguage {
            name: "GraphQL",
            comment: "#",
        },
    ),
    (
        &[".md", ".markdown", ".mkd", ".mdx"],
        DetectedLanguage {
            name: "Markdown",
            comment: "<!--",
        },
    ),
    (
        &[".xsh"],
        DetectedLanguage {
            name: "XSH",
            comment: "#",
        },
    ),
];

/// Detect language from a filename.
pub fn detect(filename: &str) -> Option<DetectedLanguage> {
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    for (patterns, lang) in LANGUAGES {
        for pattern in *patterns {
            if pattern.starts_with('.') {
                if filename.ends_with(pattern) {
                    return Some(*lang);
                }
            } else {
                // Exact basename match or prefix+dot (e.g. "Dockerfile" matches "Dockerfile.release")
                if basename == *pattern
                    || (basename.starts_with(pattern)
                        && basename.as_bytes().get(pattern.len()) == Some(&b'.'))
                {
                    return Some(*lang);
                }
            }
        }
    }
    None
}

/// Map interpreter names (from shebangs) to languages.
/// Only covers languages that have syntax-highlighting rules.
const SHEBANGS: &[(&[&str], DetectedLanguage)] = &[
    (
        &["sh", "bash", "zsh", "fish", "dash", "ash", "ksh"],
        DetectedLanguage {
            name: "Shell",
            comment: "#",
        },
    ),
    (
        &["python", "python3", "python2"],
        DetectedLanguage {
            name: "Python",
            comment: "#",
        },
    ),
    (
        &["node", "nodejs", "deno", "bun"],
        DetectedLanguage {
            name: "JavaScript",
            comment: "//",
        },
    ),
    (
        &["xsh", "xshi", "xsht"],
        DetectedLanguage {
            name: "XSH",
            comment: "#",
        },
    ),
];

/// Detect language from a shebang line (the first line of file content).
pub fn detect_from_shebang(first_line: &[u8]) -> Option<DetectedLanguage> {
    let line = first_line.strip_prefix(b"#!")?;
    // Extract the interpreter: split on whitespace to get the command and args.
    let line = line.trim_ascii();
    let mut parts = line
        .split(|&b| b == b' ' || b == b'\t')
        .filter(|p| !p.is_empty());
    let cmd = parts.next()?;
    // If the command ends with "/env", the interpreter is the next argument.
    let interpreter = if cmd.ends_with(b"/env") {
        // Skip flags like -S
        parts.find(|p| !p.starts_with(b"-"))?
    } else {
        cmd
    };
    // Take the basename of the interpreter path.
    let basename = interpreter
        .rsplit(|&b| b == b'/')
        .next()
        .unwrap_or(interpreter);
    // Strip version suffixes (e.g. "python3.11" -> "python3")
    let name = match basename.iter().position(|&b| b == b'.') {
        Some(i) => &basename[..i],
        None => basename,
    };
    let name = std::str::from_utf8(name).ok()?;
    for (interpreters, lang) in SHEBANGS {
        if interpreters.contains(&name) {
            return Some(*lang);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_extensions_resolve() {
        for (patterns, lang) in LANGUAGES {
            for pattern in *patterns {
                let filename = if pattern.starts_with('.') {
                    format!("test{pattern}")
                } else {
                    pattern.to_string()
                };
                assert_eq!(
                    detect(&filename).map(|l| l.name),
                    Some(lang.name),
                    "{filename} should detect as {}",
                    lang.name
                );
            }
        }
    }

    #[test]
    fn test_unknown_returns_none() {
        assert!(detect("readme.txt").is_none());
        assert!(detect("data.bin").is_none());
        assert!(detect("noext").is_none());
    }

    #[test]
    fn test_detect_with_path() {
        assert_eq!(detect("/some/path/main.rs").unwrap().name, "Rust");
    }

    #[test]
    fn test_detected_language_delegates_syntax_selection() {
        assert_eq!(
            detect("main.rs").unwrap().syntax(),
            Some(hi_lite::Language::Rust)
        );
        assert_eq!(
            detect("main.cpp").unwrap().syntax(),
            None,
            "detection can retain comment support without claiming a hi-lite lexer"
        );
    }

    #[test]
    fn test_detect_dockerfile_prefix() {
        assert_eq!(detect("Dockerfile").unwrap().name, "Dockerfile");
        assert_eq!(detect("Dockerfile.release").unwrap().name, "Dockerfile");
        assert_eq!(detect("Dockerfile.dev").unwrap().name, "Dockerfile");
        assert_eq!(
            detect("/path/to/Dockerfile.prod").unwrap().name,
            "Dockerfile"
        );
    }

    #[test]
    fn test_detect_makefile_prefix() {
        assert_eq!(detect("Makefile").unwrap().name, "Makefile");
        assert_eq!(detect("/path/Makefile").unwrap().name, "Makefile");
    }

    #[test]
    fn test_shebang_direct_path() {
        assert_eq!(detect_from_shebang(b"#!/bin/bash").unwrap().name, "Shell");
        assert_eq!(detect_from_shebang(b"#!/bin/sh").unwrap().name, "Shell");
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/python3").unwrap().name,
            "Python"
        );
    }

    #[test]
    fn test_shebang_env() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env bash").unwrap().name,
            "Shell"
        );
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env python3").unwrap().name,
            "Python"
        );
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env node").unwrap().name,
            "JavaScript"
        );
    }

    #[test]
    fn test_shebang_env_with_flags() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env -S python3")
                .unwrap()
                .name,
            "Python"
        );
    }

    #[test]
    fn test_shebang_version_suffix() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/python3.11").unwrap().name,
            "Python"
        );
    }

    #[test]
    fn test_shebang_not_present() {
        assert!(detect_from_shebang(b"# just a comment").is_none());
        assert!(detect_from_shebang(b"print('hello')").is_none());
        assert!(detect_from_shebang(b"").is_none());
    }

    #[test]
    fn test_shebang_unknown_interpreter() {
        assert!(detect_from_shebang(b"#!/usr/bin/unknown").is_none());
    }

    #[test]
    fn test_xsh_extension() {
        assert_eq!(detect("script.xsh").unwrap().name, "XSH");
        assert_eq!(detect("/path/to/build.xsh").unwrap().name, "XSH");
    }

    #[test]
    fn test_xsh_shebang() {
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env xsh").unwrap().name,
            "XSH"
        );
        assert_eq!(detect_from_shebang(b"#!/usr/bin/xsh").unwrap().name, "XSH");
        assert_eq!(
            detect_from_shebang(b"#!/usr/bin/env xshi").unwrap().name,
            "XSH"
        );
    }
}
