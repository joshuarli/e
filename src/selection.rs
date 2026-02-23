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

/// Sort selections by start position and merge any that overlap or are adjacent.
pub fn merge_overlapping(sels: &mut Vec<Selection>) {
    if sels.len() <= 1 {
        return;
    }
    sels.sort_by(|a, b| a.ordered().0.cmp(&b.ordered().0));
    let mut i = 0;
    while i + 1 < sels.len() {
        let (_, end_a) = sels[i].ordered();
        let (start_b, end_b) = sels[i + 1].ordered();
        if start_b <= end_a {
            // Merge: keep the one with the larger extent
            let (start_a, _) = sels[i].ordered();
            let new_end = if end_b > end_a { end_b } else { end_a };
            // Preserve cursor direction of the later selection
            if sels[i + 1].cursor <= sels[i + 1].anchor {
                sels[i] = Selection {
                    anchor: new_end,
                    cursor: start_a,
                };
            } else {
                sels[i] = Selection {
                    anchor: start_a,
                    cursor: new_end,
                };
            }
            sels.remove(i + 1);
        } else {
            i += 1;
        }
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
    let chars: Vec<u8> = line_bytes.to_vec();
    let mut i = col.min(chars.len());

    // Skip whitespace/non-word chars backward
    while i > 0 && !is_word_char(chars[i - 1]) {
        i -= 1;
    }
    // Skip word chars backward
    while i > 0 && is_word_char(chars[i - 1]) {
        i -= 1;
    }
    i
}

/// Find the end of the next word from `col` in `line_bytes`.
#[allow(dead_code)]
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

    // -- merge_overlapping --------------------------------------------------

    #[test]
    fn test_merge_overlapping_no_overlap() {
        let mut sels = vec![
            Selection::caret(Pos::new(0, 0)),
            Selection::caret(Pos::new(1, 0)),
        ];
        merge_overlapping(&mut sels);
        assert_eq!(sels.len(), 2);
    }

    #[test]
    fn test_merge_overlapping_adjacent() {
        let mut sels = vec![
            Selection {
                anchor: Pos::new(0, 0),
                cursor: Pos::new(0, 5),
            },
            Selection {
                anchor: Pos::new(0, 5),
                cursor: Pos::new(0, 10),
            },
        ];
        merge_overlapping(&mut sels);
        assert_eq!(sels.len(), 1);
        let (s, e) = sels[0].ordered();
        assert_eq!(s, Pos::new(0, 0));
        assert_eq!(e, Pos::new(0, 10));
    }

    #[test]
    fn test_merge_overlapping_overlap() {
        let mut sels = vec![
            Selection {
                anchor: Pos::new(0, 0),
                cursor: Pos::new(0, 7),
            },
            Selection {
                anchor: Pos::new(0, 3),
                cursor: Pos::new(0, 10),
            },
        ];
        merge_overlapping(&mut sels);
        assert_eq!(sels.len(), 1);
        let (s, e) = sels[0].ordered();
        assert_eq!(s, Pos::new(0, 0));
        assert_eq!(e, Pos::new(0, 10));
    }

    #[test]
    fn test_merge_overlapping_carets_same_pos() {
        let mut sels = vec![
            Selection::caret(Pos::new(0, 5)),
            Selection::caret(Pos::new(0, 5)),
        ];
        merge_overlapping(&mut sels);
        assert_eq!(sels.len(), 1);
    }

    #[test]
    fn test_merge_overlapping_unsorted() {
        let mut sels = vec![
            Selection::caret(Pos::new(2, 0)),
            Selection::caret(Pos::new(0, 0)),
            Selection::caret(Pos::new(1, 0)),
        ];
        merge_overlapping(&mut sels);
        assert_eq!(sels.len(), 3);
        assert_eq!(sels[0].cursor, Pos::new(0, 0));
        assert_eq!(sels[1].cursor, Pos::new(1, 0));
        assert_eq!(sels[2].cursor, Pos::new(2, 0));
    }

    #[test]
    fn test_merge_overlapping_single() {
        let mut sels = vec![Selection::caret(Pos::new(0, 0))];
        merge_overlapping(&mut sels);
        assert_eq!(sels.len(), 1);
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
