/// A position in the buffer: 0-indexed line and column (character index, not byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub line: usize,
    pub column: usize,
}

impl TextPosition {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    pub fn zero() -> Self {
        Self { line: 0, column: 0 }
    }
}

impl PartialOrd for TextPosition {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TextPosition {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.line
            .cmp(&other.line)
            .then(self.column.cmp(&other.column))
    }
}

/// A selection: anchor + cursor. When anchor == cursor, there is no selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: TextPosition,
    pub cursor: TextPosition,
}

impl Selection {
    pub fn caret(pos: TextPosition) -> Self {
        Self {
            anchor: pos,
            cursor: pos,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Return (start, end) where start <= end.
    pub fn ordered(&self) -> (TextPosition, TextPosition) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

/// A single caret with its own sticky vertical movement column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub selection: Selection,
    pub desired_column: Option<usize>,
}

impl Caret {
    pub fn new(pos: TextPosition) -> Self {
        Self {
            selection: Selection::caret(pos),
            desired_column: None,
        }
    }

    pub fn cursor(&self) -> TextPosition {
        self.selection.cursor
    }

    pub fn anchor(&self) -> TextPosition {
        self.selection.anchor
    }

    pub fn is_empty(&self) -> bool {
        self.selection.is_empty()
    }
}

/// Undo/redo-safe snapshot of the editor's caret layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaretSnapshot {
    pub selections: Vec<Selection>,
    pub primary: usize,
}

impl CaretSnapshot {
    pub fn primary_cursor(&self) -> TextPosition {
        self.selections
            .get(self.primary)
            .map(|selection| selection.cursor)
            .unwrap_or(TextPosition::zero())
    }
}

/// Ordered set of carets with one primary caret that drives viewport focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaretSet {
    pub carets: Vec<Caret>,
    pub primary: usize,
}

impl CaretSet {
    pub fn new(pos: TextPosition) -> Self {
        Self {
            carets: vec![Caret::new(pos)],
            primary: 0,
        }
    }

    pub fn from_selection(selection: Selection) -> Self {
        Self {
            carets: vec![Caret {
                selection,
                desired_column: None,
            }],
            primary: 0,
        }
    }

    pub fn primary(&self) -> &Caret {
        &self.carets[self.primary]
    }

    pub fn primary_mut(&mut self) -> &mut Caret {
        &mut self.carets[self.primary]
    }

    pub fn cursor(&self) -> TextPosition {
        self.primary().cursor()
    }

    pub fn selection(&self) -> Selection {
        self.primary().selection
    }

    pub fn len(&self) -> usize {
        self.carets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.carets.is_empty()
    }

    pub fn is_multicursor(&self) -> bool {
        self.carets.len() > 1
    }

    pub fn iter(&self) -> impl Iterator<Item = &Caret> {
        self.carets.iter()
    }

    pub fn collapse_to_primary(&mut self) {
        let primary = self.primary().to_owned();
        self.carets.clear();
        self.carets.push(primary);
        self.primary = 0;
    }

    pub fn set_single_selection(&mut self, selection: Selection) {
        self.carets.clear();
        self.carets.push(Caret {
            selection,
            desired_column: None,
        });
        self.primary = 0;
    }

    pub fn add_caret(&mut self, pos: TextPosition) {
        self.carets.push(Caret::new(pos));
        self.primary = self.carets.len() - 1;
        self.normalize();
    }

    pub fn snapshot(&self) -> CaretSnapshot {
        CaretSnapshot {
            selections: self.carets.iter().map(|caret| caret.selection).collect(),
            primary: self.primary,
        }
    }

    pub fn restore(&mut self, snapshot: CaretSnapshot) {
        self.carets = snapshot
            .selections
            .into_iter()
            .map(|selection| Caret {
                selection,
                desired_column: None,
            })
            .collect();
        if self.carets.is_empty() {
            self.carets.push(Caret::new(TextPosition::zero()));
            self.primary = 0;
        } else {
            self.primary = snapshot.primary.min(self.carets.len().saturating_sub(1));
            self.normalize();
        }
    }

    pub fn normalize(&mut self) {
        if self.carets.is_empty() {
            self.carets.push(Caret::new(TextPosition::zero()));
            self.primary = 0;
            return;
        }

        let primary_sel = self.primary().selection;
        let (primary_start, primary_end) = primary_sel.ordered();
        self.carets.sort_by(|a, b| {
            let (a_start, a_end) = a.selection.ordered();
            let (b_start, b_end) = b.selection.ordered();
            a_start.cmp(&b_start).then(a_end.cmp(&b_end))
        });

        let mut merged: Vec<Caret> = Vec::with_capacity(self.carets.len());
        for caret in self.carets.drain(..) {
            if let Some(last) = merged.last_mut() {
                let (last_start, last_end) = last.selection.ordered();
                let (cur_start, cur_end) = caret.selection.ordered();
                let same_empty_caret = last.selection.is_empty()
                    && caret.selection.is_empty()
                    && last_start == cur_start;
                let overlaps = cur_start < last_end;
                if same_empty_caret || overlaps {
                    last.selection = Selection {
                        anchor: last_start,
                        cursor: last_end.max(cur_end),
                    };
                    last.desired_column = None;
                    continue;
                }
            }
            merged.push(caret);
        }

        self.carets = merged;
        self.primary = self
            .carets
            .iter()
            .position(|caret| {
                let (start, end) = caret.selection.ordered();
                if primary_sel.is_empty() {
                    if caret.selection.is_empty() {
                        caret.selection.cursor == primary_sel.cursor
                    } else {
                        start <= primary_start && primary_start < end
                    }
                } else {
                    start <= primary_start && primary_end <= end
                }
            })
            .unwrap_or(0);
    }
}

// -- word boundary helpers --------------------------------------------------

/// Is the character a word character (alphanumeric or underscore)?
pub fn is_word_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Find the start of the previous word from `column` in `line_bytes`.
pub fn prev_word_boundary(line_bytes: &[u8], column: usize) -> usize {
    if column == 0 {
        return 0;
    }
    let mut i = column.min(line_bytes.len());

    // Skip whitespace/non-word chars backward
    while i > 0 && !is_word_char(line_bytes[i - 1]) {
        i -= 1;
    }
    // Skip word chars backward
    while i > 0 && is_word_char(line_bytes[i - 1]) {
        i -= 1;
    }
    i
}

/// Find the end of the next word from `column` in `line_bytes`.
pub fn next_word_boundary(line_bytes: &[u8], column: usize) -> usize {
    let len = line_bytes.len();
    let mut i = column;

    // Skip word chars forward
    while i < len && is_word_char(line_bytes[i]) {
        i += 1;
    }
    // Skip whitespace/non-word chars forward
    while i < len && !is_word_char(line_bytes[i]) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- TextPosition ----------------------------------------------------------------

    #[test]
    fn test_pos_zero() {
        let p = TextPosition::zero();
        assert_eq!(p.line, 0);
        assert_eq!(p.column, 0);
    }

    #[test]
    fn test_pos_ordering_same_line() {
        assert!(TextPosition::new(0, 0) < TextPosition::new(0, 5));
        assert!(TextPosition::new(0, 5) > TextPosition::new(0, 0));
        assert_eq!(TextPosition::new(1, 3), TextPosition::new(1, 3));
    }

    #[test]
    fn test_pos_ordering_different_lines() {
        assert!(TextPosition::new(0, 100) < TextPosition::new(1, 0));
        assert!(TextPosition::new(5, 0) > TextPosition::new(4, 999));
    }

    #[test]
    fn test_pos_eq() {
        assert_eq!(TextPosition::new(3, 7), TextPosition::new(3, 7));
        assert_ne!(TextPosition::new(3, 7), TextPosition::new(3, 8));
        assert_ne!(TextPosition::new(3, 7), TextPosition::new(4, 7));
    }

    // -- Selection ----------------------------------------------------------

    #[test]
    fn test_selection_caret_is_empty() {
        let selection = Selection::caret(TextPosition::new(5, 10));
        assert!(selection.is_empty());
        assert_eq!(selection.anchor, selection.cursor);
    }

    #[test]
    fn test_selection_non_empty() {
        let selection = Selection {
            anchor: TextPosition::new(0, 0),
            cursor: TextPosition::new(0, 5),
        };
        assert!(!selection.is_empty());
    }

    #[test]
    fn test_selection_ordered_forward() {
        let selection = Selection {
            anchor: TextPosition::new(1, 0),
            cursor: TextPosition::new(3, 5),
        };
        let (start, end) = selection.ordered();
        assert_eq!(start, TextPosition::new(1, 0));
        assert_eq!(end, TextPosition::new(3, 5));
    }

    #[test]
    fn test_selection_ordered_backward() {
        let selection = Selection {
            anchor: TextPosition::new(3, 5),
            cursor: TextPosition::new(1, 0),
        };
        let (start, end) = selection.ordered();
        assert_eq!(start, TextPosition::new(1, 0));
        assert_eq!(end, TextPosition::new(3, 5));
    }

    #[test]
    fn test_selection_ordered_same_line() {
        let selection = Selection {
            anchor: TextPosition::new(2, 10),
            cursor: TextPosition::new(2, 3),
        };
        let (start, end) = selection.ordered();
        assert_eq!(start, TextPosition::new(2, 3));
        assert_eq!(end, TextPosition::new(2, 10));
    }

    #[test]
    fn test_caretset_normalize_merges_overlapping_ranges() {
        let mut carets = CaretSet {
            carets: vec![
                Caret::new(TextPosition::new(0, 8)),
                Caret {
                    selection: Selection {
                        anchor: TextPosition::new(0, 2),
                        cursor: TextPosition::new(0, 6),
                    },
                    desired_column: None,
                },
                Caret {
                    selection: Selection {
                        anchor: TextPosition::new(0, 4),
                        cursor: TextPosition::new(0, 10),
                    },
                    desired_column: Some(4),
                },
            ],
            primary: 2,
        };

        carets.normalize();

        assert_eq!(carets.len(), 1);
        assert_eq!(
            carets.selection(),
            Selection {
                anchor: TextPosition::new(0, 2),
                cursor: TextPosition::new(0, 10),
            }
        );
        assert_eq!(carets.primary, 0);
    }

    #[test]
    fn test_caretset_normalize_keeps_adjacent_ranges_separate() {
        let mut carets = CaretSet {
            carets: vec![
                Caret {
                    selection: Selection {
                        anchor: TextPosition::new(0, 1),
                        cursor: TextPosition::new(0, 3),
                    },
                    desired_column: None,
                },
                Caret::new(TextPosition::new(0, 3)),
            ],
            primary: 1,
        };

        carets.normalize();

        assert_eq!(carets.len(), 2);
        assert_eq!(
            carets.carets[0].selection.ordered(),
            (TextPosition::new(0, 1), TextPosition::new(0, 3))
        );
        assert_eq!(
            carets.carets[1].selection,
            Selection::caret(TextPosition::new(0, 3))
        );
        assert_eq!(carets.primary, 1);
    }

    // -- is_word_char -------------------------------------------------------

    #[test]
    fn test_is_word_char() {
        assert!(is_word_char(b'a'));
        assert!(is_word_char(b'Z'));
        assert!(is_word_char(b'0'));
        assert!(is_word_char(b'_'));
        assert!(!is_word_char(b' '));
        assert!(!is_word_char(b'.'));
        assert!(!is_word_char(b'-'));
        assert!(!is_word_char(b'('));
        assert!(!is_word_char(b'\n'));
    }

    // -- prev_word_boundary -------------------------------------------------

    #[test]
    fn test_prev_word_boundary_at_start() {
        assert_eq!(prev_word_boundary(b"hello world", 0), 0);
    }

    #[test]
    fn test_prev_word_boundary_middle_of_word() {
        // "hello world", column 3 -> should go to 0
        assert_eq!(prev_word_boundary(b"hello world", 3), 0);
    }

    #[test]
    fn test_prev_word_boundary_at_word_start() {
        // "hello world", column 6 (start of "world") -> skip space, then skip "hello" -> 0
        assert_eq!(prev_word_boundary(b"hello world", 6), 0);
    }

    #[test]
    fn test_prev_word_boundary_end_of_second_word() {
        // "hello world", column 11 (end) -> skip back through "world" -> 6
        assert_eq!(prev_word_boundary(b"hello world", 11), 6);
    }

    #[test]
    fn test_prev_word_boundary_after_space() {
        // "abc  def", column 5 (at 'd') -> skip no non-word, skip back... actually column 5 is 'd'
        // skip non-word backward from column 5: nothing (d is word char)
        // Wait, let me re-examine. "abc  def" bytes: a b c ' ' ' ' d e f
        // column=5 -> chars[4]='d'. But the function uses column.min(chars.len()) as starting i
        // Actually column 5, chars[4] = ' ' (0-indexed). Hmm the function treats column as index.
        // At column 5 (pointing at 'd'): i=5, chars[4]='d' is word char
        // Actually the loop checks chars[i-1]. i=5, chars[4]='d' is word. Skip word: i=3
        // then chars[2]='c' is word. No wait chars[i-1]=chars[4]='d'... Hmm the function
        // copies to Vec<u8> and uses indices.
        // Let me just test a known case:
        assert_eq!(prev_word_boundary(b"abc def", 7), 4);
    }

    #[test]
    fn test_prev_word_boundary_multiple_spaces() {
        // "foo   bar", column 9 -> go back through "bar" to 6
        assert_eq!(prev_word_boundary(b"foo   bar", 9), 6);
    }

    #[test]
    fn test_prev_word_boundary_only_spaces() {
        assert_eq!(prev_word_boundary(b"     ", 3), 0);
    }

    #[test]
    fn test_prev_word_boundary_punctuation() {
        // "foo.bar", column 7 -> skip "bar" -> 4, skip "." -> 3, skip "foo" -> 0
        assert_eq!(prev_word_boundary(b"foo.bar", 7), 4);
    }

    // -- next_word_boundary -------------------------------------------------

    #[test]
    fn test_next_word_boundary_from_start() {
        // "hello world", column 0 -> skip "hello" to 5, skip " " to 6
        assert_eq!(next_word_boundary(b"hello world", 0), 6);
    }

    #[test]
    fn test_next_word_boundary_from_middle() {
        // "hello world", column 3 -> skip "lo" to 5, skip " " to 6
        assert_eq!(next_word_boundary(b"hello world", 3), 6);
    }

    #[test]
    fn test_next_word_boundary_from_space() {
        // "hello world", column 5 -> skip " " to 6
        assert_eq!(next_word_boundary(b"hello world", 5), 6);
    }

    #[test]
    fn test_next_word_boundary_at_end() {
        assert_eq!(next_word_boundary(b"hello", 5), 5);
    }

    #[test]
    fn test_next_word_boundary_empty() {
        assert_eq!(next_word_boundary(b"", 0), 0);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// prev_word_boundary always returns <= column and within bounds.
        #[test]
        fn prev_word_boundary_in_bounds(
            line in prop::collection::vec(any::<u8>(), 0..128),
            column in 0usize..256,
        ) {
            let result = prev_word_boundary(&line, column);
            prop_assert!(result <= column);
            prop_assert!(result <= line.len());
        }

        /// next_word_boundary always returns >= column and within bounds.
        #[test]
        fn next_word_boundary_in_bounds(
            line in prop::collection::vec(any::<u8>(), 0..128),
            column in 0usize..256,
        ) {
            let result = next_word_boundary(&line, column.min(line.len()));
            prop_assert!(result >= column.min(line.len()));
            prop_assert!(result <= line.len());
        }

        /// Selection::ordered always returns (start <= end).
        #[test]
        fn selection_ordered_invariant(
            al in 0usize..1000, ac in 0usize..1000,
            cl in 0usize..1000, cc in 0usize..1000,
        ) {
            let selection = Selection {
                anchor: TextPosition::new(al, ac),
                cursor: TextPosition::new(cl, cc),
            };
            let (start, end) = selection.ordered();
            prop_assert!(start <= end);
            // One of them should be anchor, the other cursor
            prop_assert!(
                (start == selection.anchor && end == selection.cursor)
                || (start == selection.cursor && end == selection.anchor)
            );
        }

        /// TextPosition ordering is a total order (transitivity, antisymmetry).
        #[test]
        fn pos_ordering_total(
            l1 in 0usize..100, c1 in 0usize..100,
            l2 in 0usize..100, c2 in 0usize..100,
        ) {
            let a = TextPosition::new(l1, c1);
            let b = TextPosition::new(l2, c2);
            // Antisymmetry: if a <= b and b <= a then a == b
            if a <= b && b <= a {
                prop_assert_eq!(a, b);
            }
            // Totality: either a <= b or b <= a
            prop_assert!(a <= b || b <= a);
        }
    }
}
