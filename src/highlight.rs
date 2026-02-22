/// Stub for future syntax highlighting.
///
/// When implemented, the highlighter will provide per-character style information
/// for each line, which the renderer will use to apply ANSI color codes.
///
/// A style applied to a range of characters on a line.
#[allow(dead_code)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// Minimal color/style enum.
#[allow(dead_code)]
pub enum Style {
    Default,
    Keyword,
    String,
    Comment,
    Number,
    Type,
}

/// Trait for syntax highlighters.
#[allow(unused_variables)]
pub trait Highlighter {
    /// Return styled spans for a single line of text.
    fn highlight_line(&self, line: &[u8], line_number: usize) -> Vec<Span>;

    /// Detect the language from a filename extension.
    fn detect(filename: &str) -> Option<Box<dyn Highlighter>>
    where
        Self: Sized,
    {
        None
    }
}
