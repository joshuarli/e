//! Regenerate the XSH vocabulary region inside `src/languages/xsh.rs`, the
//! syntax-highlighting vocabulary used by the editor's byte-oriented
//! highlighter.
//!
//! The XSH language keeps its public surface in the registry at `$XSH_REPO`
//! (default `~/d/laputa-systems/xsh`).  The vocabulary is read from the
//! registry's own source with `syn`, so no compiled binary or JSON side
//! channel is involved:
//!
//!   * `src/syntax/token.rs` -- keyword match arms in `Keyword::from_ident`;
//!   * `crates/xsh-registry/src/types.rs` -- the `builtin_type_names!` table;
//!   * `crates/xsh-registry/src/signature/modules.rs` -- `build_api_spec()`
//!     module names;
//!   * `crates/xsh-registry/src/records.rs` -- `record_schemas()` record names;
//!   * `crates/xsh-registry/src/reference.rs` -- effect names and stream
//!     stages.
//!
//! It emits the sorted `XSH_KEYWORDS`, `XSH_TYPES`, and `XSH_MACROS` arrays
//! and splices them in place between the `BEGIN/END GENERATED XSH VOCABULARY`
//! markers in `xsh.rs`, so the hand-maintained rules and scanner around them
//! are untouched.  Regenerate after the XSH language changes keywords, types,
//! modules, or stage names:
//!
//!     make gen-xsh
//!
//! Exit status is non-zero on any parse failure so CI can treat a stale or
//! broken registry as an error.
//!
//! `tests/xsh_lang.rs` includes this file and re-splices the same vocabulary
//! during `cargo test` to catch drift when the registry is present.

use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process;

use proc_macro2::{Delimiter, TokenTree};
use syn::visit::Visit;
use syn::{Item, MacroDelimiter};

// Containers and constructor names used in signatures but not listed in the
// registry's `builtin_type_names!` table.
const EXTRA_TYPES: &[&str] = &["List", "Stream", "Ok", "Err"];

// Global XSH builtins that are not module functions.
const CORE_BUILTINS: &[&str] = &["abort", "print"];

// Run-form vocabulary (`run text`, `run bytes`, `run.status`, `run.capture`).
const RUN_WORDS: &[&str] = &["text", "bytes", "status", "capture"];

/// A small error type so failures format as a single line.
#[derive(Debug)]
struct GenError(String);

impl Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for GenError {}

fn err<T>(message: impl Into<String>) -> Result<T, Box<dyn Error>> {
    Err(Box::new(GenError(message.into())))
}

// -- syn-based grammar extraction -------------------------------------------

/// Collect the string patterns from every `"keyword" => Self::Variant` arm,
/// which is the lexer's authoritative spelling of each keyword.
struct KeywordCollector {
    keywords: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for KeywordCollector {
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        for arm in &node.arms {
            if let syn::Pat::Lit(pattern) = &arm.pat
                && let syn::Lit::Str(s) = &pattern.lit
            {
                self.keywords.insert(s.value());
            }
        }
    }
}

fn keywords_from_token_rs(repo: &Path) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let source = std::fs::read_to_string(repo.join("src").join("syntax").join("token.rs"))?;
    let file = syn::parse_file(&source)?;
    for item in file.items {
        let Item::Impl(impl_block) = item else {
            continue;
        };
        for member in impl_block.items {
            let syn::ImplItem::Fn(function) = member else {
                continue;
            };
            if function.sig.ident == "from_ident" {
                let mut collector = KeywordCollector {
                    keywords: BTreeSet::new(),
                };
                collector.visit_block(&function.block);
                if collector.keywords.is_empty() {
                    return err("found `from_ident` but no string-pattern match arms");
                }
                return Ok(collector.keywords);
            }
        }
    }
    err("no `Keyword::from_ident` function found in src/syntax/token.rs")
}

/// Collect the display-name string of every `(Variant, "name")` tuple in a
/// `builtin_type_names!` invocation.  `<unknown>` is a lexer internal, not a
/// language type, and is excluded by variant name.
fn names_from_macro_tokens(tokens: &proc_macro2::TokenStream) -> Vec<String> {
    let mut names = Vec::new();
    for token in tokens.clone().into_iter() {
        let TokenTree::Group(group) = token else {
            continue;
        };
        if group.delimiter() != Delimiter::Parenthesis {
            continue;
        }
        let inner: Vec<TokenTree> = group.stream().into_iter().collect();
        if inner.len() != 3 {
            continue;
        }
        let (TokenTree::Ident(variant), TokenTree::Punct(_), TokenTree::Literal(lit)) =
            (&inner[0], &inner[1], &inner[2])
        else {
            continue;
        };
        if variant == "Unknown" {
            continue;
        }
        if let Some(text) = strip_literal(&lit.to_string()) {
            names.push(text.to_string());
        }
    }
    names
}

fn strip_literal(literal: &str) -> Option<&str> {
    literal.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
}

fn builtin_types_from_types_rs(src: &Path) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let source = std::fs::read_to_string(
        src.join("crates")
            .join("xsh-registry")
            .join("src")
            .join("types.rs"),
    )?;
    let file = syn::parse_file(&source)?;
    for item in file.items {
        if let Item::Macro(mac) = &item
            && mac.mac.path.is_ident("builtin_type_names")
            && matches!(mac.mac.delimiter, MacroDelimiter::Paren(_))
        {
            return Ok(BTreeSet::from_iter(names_from_macro_tokens(
                &mac.mac.tokens,
            )));
        }
    }
    err("no `builtin_type_names!` invocation found in crates/xsh-registry/src/types.rs")
}

/// Collect the `name` field of every `ModuleEntry` struct literal inside
/// `build_api_spec()`, which holds the standard module list.
/// Collect the `name` field of every `ModuleEntry` struct literal in
/// `build_api_spec()`, which holds the standard module list.
///
/// syn keeps the bodies of `vec![...]` invocations opaque, so the struct
/// literals are read from the raw token stream rather than by visiting.
struct ModuleNameCollector {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for ModuleNameCollector {
    fn visit_expr_macro(&mut self, node: &'ast syn::ExprMacro) {
        if node.mac.path.is_ident("vec") {
            for name in names_from_struct_literal_groups(&node.mac.tokens) {
                self.names.insert(name);
            }
        }
    }
}

/// Read the `name: "..."` field out of every `{ ... }` group in a token
/// stream, matching `ModuleEntry { name: "applet", ... }` literals.
fn names_from_struct_literal_groups(tokens: &proc_macro2::TokenStream) -> Vec<String> {
    let mut names = Vec::new();
    for token in tokens.clone().into_iter() {
        let TokenTree::Group(group) = token else {
            continue;
        };
        if group.delimiter() != Delimiter::Brace {
            continue;
        }
        let inner: Vec<TokenTree> = group.stream().into_iter().collect();
        for i in 0..inner.len().saturating_sub(2) {
            match (&inner[i], &inner[i + 1], &inner[i + 2]) {
                (TokenTree::Ident(ident), TokenTree::Punct(colon), TokenTree::Literal(lit))
                    if ident == "name" && colon.as_char() == ':' =>
                {
                    if let Some(text) = strip_literal(&lit.to_string()) {
                        names.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    names
}

fn modules_from_modules_rs(src: &Path) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let source = std::fs::read_to_string(
        src.join("crates")
            .join("xsh-registry")
            .join("src")
            .join("signature")
            .join("modules.rs"),
    )?;
    let file = syn::parse_file(&source)?;
    for item in file.items {
        if let Item::Fn(function) = item
            && function.sig.ident == "build_api_spec"
        {
            let mut collector = ModuleNameCollector {
                names: BTreeSet::new(),
            };
            collector.visit_block(&function.block);
            if collector.names.is_empty() {
                return err("found `build_api_spec` but no `ModuleEntry` literals");
            }
            return Ok(collector.names);
        }
    }
    err("no `build_api_spec` function found in signature/modules.rs")
}

/// Collect the leading string of every `("Record", ...)` pair in a
/// `btree_map(vec![...])` call, which is the record schema table.  As with
/// `ModuleNameCollector`, the tuples are read from the `vec!` tokens.
struct RecordNameCollector {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for RecordNameCollector {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref()
            && path.path.is_ident("btree_map")
            && let Some(syn::Expr::Macro(mac)) = node.args.first()
            && mac.mac.path.is_ident("vec")
        {
            for name in names_from_tuple_literal_groups(&mac.mac.tokens) {
                self.names.insert(name);
            }
        }
    }
}

/// Read the first token of every `( ... )` group in a token stream, i.e. the
/// `"Record"` half of each `("Record", type_fn())` tuple.
fn names_from_tuple_literal_groups(tokens: &proc_macro2::TokenStream) -> Vec<String> {
    let mut names = Vec::new();
    for token in tokens.clone().into_iter() {
        let TokenTree::Group(group) = token else {
            continue;
        };
        if group.delimiter() != Delimiter::Parenthesis {
            continue;
        }
        let inner: Vec<TokenTree> = group.stream().into_iter().collect();
        if let Some(TokenTree::Literal(lit)) = inner.first()
            && let Some(text) = strip_literal(&lit.to_string())
        {
            names.push(text.to_string());
        }
    }
    names
}

fn records_from_records_rs(src: &Path) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let source = std::fs::read_to_string(
        src.join("crates")
            .join("xsh-registry")
            .join("src")
            .join("records.rs"),
    )?;
    let file = syn::parse_file(&source)?;
    let mut collector = RecordNameCollector {
        names: BTreeSet::new(),
    };
    for item in file.items {
        if let Item::Fn(function) = item {
            collector.visit_block(&function.block);
        }
    }
    if collector.names.is_empty() {
        return err("no `btree_map(vec![...])` record table found in records.rs");
    }
    Ok(collector.names)
}

/// Collect the `name` field of every `EffectReference` struct literal in the
/// `EFFECT_REFERENCES` const.
struct EffectReferenceCollector {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for EffectReferenceCollector {
    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if node.path.is_ident("EffectReference") {
            for field in &node.fields {
                if let syn::Member::Named(ident) = &field.member
                    && ident == "name"
                    && let syn::Expr::Lit(lit) = &field.expr
                    && let syn::Lit::Str(s) = &lit.lit
                {
                    self.names.insert(s.value());
                }
            }
        }
    }
}

/// Collect the string literals from the `STREAM_STAGES` const array.
struct StreamStageCollector {
    stages: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for StreamStageCollector {
    fn visit_expr_array(&mut self, node: &'ast syn::ExprArray) {
        for entry in &node.elems {
            if let syn::Expr::Lit(lit) = entry
                && let syn::Lit::Str(s) = &lit.lit
            {
                self.stages.insert(s.value());
            }
        }
    }
}

fn reference_vocabulary(
    src: &Path,
) -> Result<(BTreeSet<String>, BTreeSet<String>), Box<dyn Error>> {
    let source = std::fs::read_to_string(
        src.join("crates")
            .join("xsh-registry")
            .join("src")
            .join("reference.rs"),
    )?;
    let file = syn::parse_file(&source)?;
    let mut effects = EffectReferenceCollector {
        names: BTreeSet::new(),
    };
    let mut stages = StreamStageCollector {
        stages: BTreeSet::new(),
    };
    for item in file.items {
        match item {
            Item::Const(c) if c.ident == "EFFECT_REFERENCES" => {
                effects.visit_expr(&c.expr);
            }
            Item::Const(c) if c.ident == "STREAM_STAGES" => {
                stages.visit_expr(&c.expr);
            }
            _ => {}
        }
    }
    if effects.names.is_empty() {
        return err("no `EFFECT_REFERENCES` found in reference.rs");
    }
    if stages.stages.is_empty() {
        return err("no `STREAM_STAGES` found in reference.rs");
    }
    Ok((effects.names, stages.stages))
}

// -- vocabulary construction ---------------------------------------------------

/// Keep only identifiers the byte highlighter can scan as one token.
fn single_token(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn build_macros(
    modules: BTreeSet<String>,
    effects: BTreeSet<String>,
    stages: BTreeSet<String>,
    keywords: &BTreeSet<String>,
) -> Vec<String> {
    let mut macros = BTreeSet::new();
    macros.extend(modules);
    macros.extend(effects);
    macros.extend(stages.into_iter().filter(|stage| single_token(stage)));
    macros.extend(RUN_WORDS.iter().map(|s| (*s).to_owned()));
    macros.extend(CORE_BUILTINS.iter().map(|s| (*s).to_owned()));
    macros.retain(|word| !keywords.contains(word));
    macros.into_iter().collect()
}

fn build_types(builtin: BTreeSet<String>, records: BTreeSet<String>) -> Vec<String> {
    let mut types = builtin;
    types.extend(EXTRA_TYPES.iter().map(|s| (*s).to_owned()));
    types.extend(records);
    types.into_iter().collect()
}

// -- output ---------------------------------------------------------------------

const START_MARKER: &str = "// BEGIN GENERATED XSH VOCABULARY";
const END_MARKER: &str = "// END GENERATED XSH VOCABULARY";

fn emit_array(name: &str, items: &[String]) -> String {
    if items.is_empty() {
        return format!("#[rustfmt::skip]\npub static {name}: &[&str] = &[];\n");
    }
    let body = items
        .iter()
        .map(|item| format!("\"{item}\""))
        .collect::<Vec<_>>()
        .join(",\n    ");
    format!("#[rustfmt::skip]\npub static {name}: &[&str] = &[\n    {body},\n];\n")
}

/// Regenerated vocabulary: content plus a per-array line count for the log
/// line, since the emitted text is compared verbatim elsewhere.
pub struct Vocab {
    pub content: String,
    pub keywords: usize,
    pub types: usize,
    pub macros: usize,
}

/// Replace the generated region in `xsh.rs` with a freshly generated block.
///
/// `block` is a complete region including its `BEGIN`/`END` marker lines.
/// Both markers must already be present in `source`, which keeps this from
/// silently rewriting a file the generator does not own.
pub fn splice_into(source: &str, block: &str) -> Result<String, Box<dyn Error>> {
    let lines = source.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.trim_end() == START_MARKER);
    let end = lines.iter().rposition(|line| line.trim_end() == END_MARKER);
    match (start, end) {
        (Some(s), Some(e)) if s <= e => {
            let block_lines = block.lines().collect::<Vec<_>>();
            let mut out = lines[..s].to_vec();
            out.extend_from_slice(&block_lines);
            out.extend_from_slice(&lines[e + 1..]);
            let mut joined = out.join("\n");
            if source.ends_with('\n') {
                joined.push('\n');
            }
            Ok(joined)
        }
        _ => err(format!(
            "{START_MARKER}/{END_MARKER} markers not found; refusing to rewrite the file"
        )),
    }
}

/// Recompute the vocabulary from the registry source at `repo`.
pub fn regenerate(repo: &Path) -> Result<Vocab, Box<dyn Error>> {
    let keywords = keywords_from_token_rs(repo)?;
    let builtin_types = builtin_types_from_types_rs(repo)?;
    let modules = modules_from_modules_rs(repo)?;
    let records = records_from_records_rs(repo)?;
    let (effects, stages) = reference_vocabulary(repo)?;
    let types = build_types(builtin_types, records);
    let macros = build_macros(modules, effects, stages, &keywords);
    let keyword_list: Vec<String> = keywords.into_iter().collect();

    let body = emit_array("XSH_KEYWORDS", &keyword_list)
        + "\n"
        + &emit_array("XSH_TYPES", &types)
        + "\n"
        + &emit_array("XSH_MACROS", &macros);
    let block = format!(
        "{START_MARKER}\n\
         // Generated from {}\n\
         // XSH syntax-highlighting vocabulary, generated from the XSH registry source.\n\
         // DO NOT EDIT BY HAND.  Regenerate after the XSH language or standard library\n\
         // changes keywords, types, modules, or stream stages: `make gen-xsh`.\n\
         {body}\
         {END_MARKER}\n",
        repo.display()
    );

    Ok(Vocab {
        keywords: keyword_list.len(),
        types: types.len(),
        macros: macros.len(),
        content: block,
    })
}

// -- command-line entry point -------------------------------------------------

/// Path to the XSH registry checkout, from `XSH_REPO` or the default location.
pub fn default_repo() -> PathBuf {
    if let Some(repo) = env::var_os("XSH_REPO") {
        return PathBuf::from(repo);
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("d")
        .join("laputa-systems")
        .join("xsh")
}

fn run() -> Result<bool, Box<dyn Error>> {
    let mut repo: PathBuf = default_repo();
    let mut out: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("languages")
        .join("xsh.rs");
    let mut check = false;

    let mut iter = env::args_os().skip(1);
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--repo") => {
                repo = PathBuf::from(iter.next().expect("--repo needs a path"));
            }
            Some("--out") => {
                out = PathBuf::from(iter.next().expect("--out needs a path"));
            }
            Some("--check") => check = true,
            Some("--help" | "-h") => {
                println!(
                    "usage: gen-xsh [--repo PATH] [--out PATH] [--check]\n\
                     reads the vocabulary from the XSH registry source with `syn`\n\
                     and rewrites (or checks) the generated region of src/languages/xsh.rs."
                );
                process::exit(0);
            }
            _ => eprintln!("ignoring unknown argument: {:?}", arg),
        }
    }
    if !repo.join("Cargo.toml").exists() {
        return err(format!(
            "{} is not an xsh checkout (no Cargo.toml); set XSH_REPO",
            repo.display()
        ));
    }

    let vocab = regenerate(&repo)?;
    let existing = std::fs::read_to_string(&out).unwrap_or_default();
    let spliced = splice_into(&existing, &vocab.content)?;

    if check {
        if spliced == existing {
            println!("src/languages/xsh.rs is up to date");
            return Ok(true);
        }
        eprintln!("src/languages/xsh.rs is out of date; run `make gen-xsh`");
        return Ok(false);
    }

    std::fs::write(&out, &spliced)?;
    println!(
        "wrote {} ({} keywords, {} types, {} macros)",
        out.display(),
        vocab.keywords,
        vocab.types,
        vocab.macros
    );
    Ok(true)
}

fn main() -> Result<(), Box<dyn Error>> {
    if run()? { Ok(()) } else { process::exit(1) }
}
