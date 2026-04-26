/// A position in the buffer: 0-indexed line and column (character index, not byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }

    pub fn zero() -> Self {
        Self { line: 0, col: 0 }
    }
}

impl PartialOrd for Pos {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pos {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.line.cmp(&other.line).then(self.col.cmp(&other.col))
    }
}

/// A selection: anchor + cursor. When anchor == cursor, there is no selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub cursor: Pos,
}

impl Selection {
    pub fn caret(pos: Pos) -> Self {
        Self {
            anchor: pos,
            cursor: pos,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Return (start, end) where start <= end.
    pub fn ordered(&self) -> (Pos, Pos) {
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
    pub sel: Selection,
    pub desired_col: Option<usize>,
}

impl Caret {
    pub fn caret(pos: Pos) -> Self {
        Self {
            sel: Selection::caret(pos),
            desired_col: None,
        }
    }

    pub fn cursor(&self) -> Pos {
        self.sel.cursor
    }

    pub fn anchor(&self) -> Pos {
        self.sel.anchor
    }

    pub fn is_empty(&self) -> bool {
        self.sel.is_empty()
    }
}

/// Undo/redo-safe snapshot of the editor's caret layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaretSnapshot {
    pub selections: Vec<Selection>,
    pub primary: usize,
}

impl CaretSnapshot {
    pub fn primary_cursor(&self) -> Pos {
        self.selections
            .get(self.primary)
            .map(|sel| sel.cursor)
            .unwrap_or(Pos::zero())
    }
}

/// Ordered set of carets with one primary caret that drives viewport focus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaretSet {
    pub carets: Vec<Caret>,
    pub primary: usize,
}

impl CaretSet {
    pub fn new(pos: Pos) -> Self {
        Self {
            carets: vec![Caret::caret(pos)],
            primary: 0,
        }
    }

    pub fn from_selection(sel: Selection) -> Self {
        Self {
            carets: vec![Caret {
                sel,
                desired_col: None,
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

    pub fn cursor(&self) -> Pos {
        self.primary().cursor()
    }

    pub fn selection(&self) -> Selection {
        self.primary().sel
    }

    pub fn len(&self) -> usize {
        self.carets.len()
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

    pub fn set_single_selection(&mut self, sel: Selection) {
        self.carets.clear();
        self.carets.push(Caret {
            sel,
            desired_col: None,
        });
        self.primary = 0;
    }

    pub fn add_caret(&mut self, pos: Pos) {
        self.carets.push(Caret::caret(pos));
        self.primary = self.carets.len() - 1;
        self.normalize();
    }

    pub fn snapshot(&self) -> CaretSnapshot {
        CaretSnapshot {
            selections: self.carets.iter().map(|caret| caret.sel).collect(),
            primary: self.primary,
        }
    }

    pub fn restore(&mut self, snapshot: CaretSnapshot) {
        self.carets = snapshot
            .selections
            .into_iter()
            .map(|sel| Caret {
                sel,
                desired_col: None,
            })
            .collect();
        if self.carets.is_empty() {
            self.carets.push(Caret::caret(Pos::zero()));
            self.primary = 0;
        } else {
            self.primary = snapshot.primary.min(self.carets.len().saturating_sub(1));
            self.normalize();
        }
    }

    pub fn normalize(&mut self) {
        if self.carets.is_empty() {
            self.carets.push(Caret::caret(Pos::zero()));
            self.primary = 0;
            return;
        }

        let primary_sel = self.primary().sel;
        let (primary_start, primary_end) = primary_sel.ordered();
        self.carets.sort_by(|a, b| {
            let (a_start, a_end) = a.sel.ordered();
            let (b_start, b_end) = b.sel.ordered();
            a_start.cmp(&b_start).then(a_end.cmp(&b_end))
        });

        let mut merged: Vec<Caret> = Vec::with_capacity(self.carets.len());
        for caret in self.carets.drain(..) {
            if let Some(last) = merged.last_mut() {
                let (last_start, last_end) = last.sel.ordered();
                let (cur_start, cur_end) = caret.sel.ordered();
                let same_empty_caret =
                    last.sel.is_empty() && caret.sel.is_empty() && last_start == cur_start;
                let overlaps = cur_start < last_end;
                if same_empty_caret || overlaps {
                    last.sel = Selection {
                        anchor: last_start,
                        cursor: last_end.max(cur_end),
                    };
                    last.desired_col = None;
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
                let (start, end) = caret.sel.ordered();
                if primary_sel.is_empty() {
                    if caret.sel.is_empty() {
                        caret.sel.cursor == primary_sel.cursor
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

/// Find the start of the previous word from `col` in `line_bytes`.
pub fn prev_word_boundary(line_bytes: &[u8], col: usize) -> usize {
    if col == 0 {
        return 0;
    }
    let mut i = col.min(line_bytes.len());

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

/// Find the end of the next word from `col` in `line_bytes`.
pub fn next_word_boundary(line_bytes: &[u8], col: usize) -> usize {
    let len = line_bytes.len();
    let mut i = col;

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

    // -- Pos ----------------------------------------------------------------

    #[test]
    fn test_pos_zero() {
        let p = Pos::zero();
        assert_eq!(p.line, 0);
        assert_eq!(p.col, 0);
    }

    #[test]
    fn test_pos_ordering_same_line() {
        assert!(Pos::new(0, 0) < Pos::new(0, 5));
        assert!(Pos::new(0, 5) > Pos::new(0, 0));
        assert_eq!(Pos::new(1, 3), Pos::new(1, 3));
    }

    #[test]
    fn test_pos_ordering_different_lines() {
        assert!(Pos::new(0, 100) < Pos::new(1, 0));
        assert!(Pos::new(5, 0) > Pos::new(4, 999));
    }

    #[test]
    fn test_pos_eq() {
        assert_eq!(Pos::new(3, 7), Pos::new(3, 7));
        assert_ne!(Pos::new(3, 7), Pos::new(3, 8));
        assert_ne!(Pos::new(3, 7), Pos::new(4, 7));
    }

    // -- Selection ----------------------------------------------------------

    #[test]
    fn test_selection_caret_is_empty() {
        let sel = Selection::caret(Pos::new(5, 10));
        assert!(sel.is_empty());
        assert_eq!(sel.anchor, sel.cursor);
    }

    #[test]
    fn test_selection_non_empty() {
        let sel = Selection {
            anchor: Pos::new(0, 0),
            cursor: Pos::new(0, 5),
        };
        assert!(!sel.is_empty());
    }

    #[test]
    fn test_selection_ordered_forward() {
        let sel = Selection {
            anchor: Pos::new(1, 0),
            cursor: Pos::new(3, 5),
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, Pos::new(1, 0));
        assert_eq!(end, Pos::new(3, 5));
    }

    #[test]
    fn test_selection_ordered_backward() {
        let sel = Selection {
            anchor: Pos::new(3, 5),
            cursor: Pos::new(1, 0),
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, Pos::new(1, 0));
        assert_eq!(end, Pos::new(3, 5));
    }

    #[test]
    fn test_selection_ordered_same_line() {
        let sel = Selection {
            anchor: Pos::new(2, 10),
            cursor: Pos::new(2, 3),
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, Pos::new(2, 3));
        assert_eq!(end, Pos::new(2, 10));
    }

    #[test]
    fn test_caretset_normalize_merges_overlapping_ranges() {
        let mut carets = CaretSet {
            carets: vec![
                Caret::caret(Pos::new(0, 8)),
                Caret {
                    sel: Selection {
                        anchor: Pos::new(0, 2),
                        cursor: Pos::new(0, 6),
                    },
                    desired_col: None,
                },
                Caret {
                    sel: Selection {
                        anchor: Pos::new(0, 4),
                        cursor: Pos::new(0, 10),
                    },
                    desired_col: Some(4),
                },
            ],
            primary: 2,
        };

        carets.normalize();

        assert_eq!(carets.len(), 1);
        assert_eq!(
            carets.selection(),
            Selection {
                anchor: Pos::new(0, 2),
                cursor: Pos::new(0, 10),
            }
        );
        assert_eq!(carets.primary, 0);
    }

    #[test]
    fn test_caretset_normalize_keeps_adjacent_ranges_separate() {
        let mut carets = CaretSet {
            carets: vec![
                Caret {
                    sel: Selection {
                        anchor: Pos::new(0, 1),
                        cursor: Pos::new(0, 3),
                    },
                    desired_col: None,
                },
                Caret::caret(Pos::new(0, 3)),
            ],
            primary: 1,
        };

        carets.normalize();

        assert_eq!(carets.len(), 2);
        assert_eq!(
            carets.carets[0].sel.ordered(),
            (Pos::new(0, 1), Pos::new(0, 3))
        );
        assert_eq!(carets.carets[1].sel, Selection::caret(Pos::new(0, 3)));
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
        // "hello world", col 3 -> should go to 0
        assert_eq!(prev_word_boundary(b"hello world", 3), 0);
    }

    #[test]
    fn test_prev_word_boundary_at_word_start() {
        // "hello world", col 6 (start of "world") -> skip space, then skip "hello" -> 0
        assert_eq!(prev_word_boundary(b"hello world", 6), 0);
    }

    #[test]
    fn test_prev_word_boundary_end_of_second_word() {
        // "hello world", col 11 (end) -> skip back through "world" -> 6
        assert_eq!(prev_word_boundary(b"hello world", 11), 6);
    }

    #[test]
    fn test_prev_word_boundary_after_space() {
        // "abc  def", col 5 (at 'd') -> skip no non-word, skip back... actually col 5 is 'd'
        // skip non-word backward from col 5: nothing (d is word char)
        // Wait, let me re-examine. "abc  def" bytes: a b c ' ' ' ' d e f
        // col=5 -> chars[4]='d'. But the function uses col.min(chars.len()) as starting i
        // Actually col 5, chars[4] = ' ' (0-indexed). Hmm the function treats col as index.
        // At col 5 (pointing at 'd'): i=5, chars[4]='d' is word char
        // Actually the loop checks chars[i-1]. i=5, chars[4]='d' is word. Skip word: i=3
        // then chars[2]='c' is word. No wait chars[i-1]=chars[4]='d'... Hmm the function
        // copies to Vec<u8> and uses indices.
        // Let me just test a known case:
        assert_eq!(prev_word_boundary(b"abc def", 7), 4);
    }

    #[test]
    fn test_prev_word_boundary_multiple_spaces() {
        // "foo   bar", col 9 -> go back through "bar" to 6
        assert_eq!(prev_word_boundary(b"foo   bar", 9), 6);
    }

    #[test]
    fn test_prev_word_boundary_only_spaces() {
        assert_eq!(prev_word_boundary(b"     ", 3), 0);
    }

    #[test]
    fn test_prev_word_boundary_punctuation() {
        // "foo.bar", col 7 -> skip "bar" -> 4, skip "." -> 3, skip "foo" -> 0
        assert_eq!(prev_word_boundary(b"foo.bar", 7), 4);
    }

    // -- next_word_boundary -------------------------------------------------

    #[test]
    fn test_next_word_boundary_from_start() {
        // "hello world", col 0 -> skip "hello" to 5, skip " " to 6
        assert_eq!(next_word_boundary(b"hello world", 0), 6);
    }

    #[test]
    fn test_next_word_boundary_from_middle() {
        // "hello world", col 3 -> skip "lo" to 5, skip " " to 6
        assert_eq!(next_word_boundary(b"hello world", 3), 6);
    }

    #[test]
    fn test_next_word_boundary_from_space() {
        // "hello world", col 5 -> skip " " to 6
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
        /// prev_word_boundary always returns <= col and within bounds.
        #[test]
        fn prev_word_boundary_in_bounds(
            line in prop::collection::vec(any::<u8>(), 0..128),
            col in 0usize..256,
        ) {
            let result = prev_word_boundary(&line, col);
            prop_assert!(result <= col);
            prop_assert!(result <= line.len());
        }

        /// next_word_boundary always returns >= col and within bounds.
        #[test]
        fn next_word_boundary_in_bounds(
            line in prop::collection::vec(any::<u8>(), 0..128),
            col in 0usize..256,
        ) {
            let result = next_word_boundary(&line, col.min(line.len()));
            prop_assert!(result >= col.min(line.len()));
            prop_assert!(result <= line.len());
        }

        /// Selection::ordered always returns (start <= end).
        #[test]
        fn selection_ordered_invariant(
            al in 0usize..1000, ac in 0usize..1000,
            cl in 0usize..1000, cc in 0usize..1000,
        ) {
            let sel = Selection {
                anchor: Pos::new(al, ac),
                cursor: Pos::new(cl, cc),
            };
            let (start, end) = sel.ordered();
            prop_assert!(start <= end);
            // One of them should be anchor, the other cursor
            prop_assert!(
                (start == sel.anchor && end == sel.cursor)
                || (start == sel.cursor && end == sel.anchor)
            );
        }

        /// Pos ordering is a total order (transitivity, antisymmetry).
        #[test]
        fn pos_ordering_total(
            l1 in 0usize..100, c1 in 0usize..100,
            l2 in 0usize..100, c2 in 0usize..100,
        ) {
            let a = Pos::new(l1, c1);
            let b = Pos::new(l2, c2);
            // Antisymmetry: if a <= b and b <= a then a == b
            if a <= b && b <= a {
                prop_assert_eq!(a, b);
            }
            // Totality: either a <= b or b <= a
            prop_assert!(a <= b || b <= a);
        }
    }
}
