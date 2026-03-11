/// Gap buffer backed by a `Vec<u8>` with an always-valid incremental line-start index.
///
/// Text is stored as UTF-8 bytes. The gap sits between `gap_start` and `gap_end`
/// inside `data`. Insertions at the cursor just fill the gap; deletions widen it.
pub struct GapBuffer {
    data: Vec<u8>,
    gap_start: usize,
    gap_end: usize,
    /// Byte offsets of line starts (entry 0 is always 0). Always valid — updated
    /// incrementally on every insert/delete.
    line_starts: Vec<usize>,
    /// Per-line ASCII flag (parallel to `line_starts`). When true, every byte on
    /// that line is < 0x80, so char_count == byte_len and char/byte offsets are
    /// identical — all UTF-8 walking can be skipped.
    line_ascii: Vec<bool>,
    /// Min line touched since the last `take_dirty_line()` call.
    min_dirty_line: usize,
    /// Monotonically increasing counter, bumped on every insert/delete.
    version: u64,
}

const MIN_GAP: usize = 128;

impl Default for GapBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl GapBuffer {
    pub fn new() -> Self {
        let gap = MIN_GAP;
        Self {
            data: vec![0; gap],
            gap_start: 0,
            gap_end: gap,
            line_starts: vec![0],
            line_ascii: vec![true],
            min_dirty_line: usize::MAX,
            version: 0,
        }
    }

    /// Build a `GapBuffer` from an owned `Vec<u8>`, reusing the allocation.
    ///
    /// This is the fast path for opening files: the caller passes the `Vec`
    /// returned by `read_file` and we extend it in-place for the gap, so the
    /// whole startup path needs only one allocation (the fs::read itself).
    /// Pre-allocates `line_starts` with a heuristic to avoid reallocs.
    pub fn from_vec(mut data: Vec<u8>) -> Self {
        let gap = MIN_GAP;
        // Reserve the gap space now so the resize at the end doesn't reallocate.
        data.reserve(gap);
        let content_len = data.len();
        // Heuristic: typical source lines are ~20 bytes; pre-allocate to avoid
        // the ~20 doublings that Vec would otherwise do for 1M-line files.
        let mut starts = Vec::with_capacity(content_len / 20 + 16);
        let mut ascii = Vec::with_capacity(content_len / 20 + 16);
        starts.push(0usize);
        let mut line_is_ascii = true;
        for (i, &b) in data.iter().enumerate() {
            if b >= 0x80 {
                line_is_ascii = false;
            }
            if b == b'\n' && i + 1 < content_len {
                starts.push(i + 1);
                ascii.push(line_is_ascii);
                line_is_ascii = true;
            }
        }
        ascii.push(line_is_ascii); // last line
        data.resize(content_len + gap, 0);
        Self {
            data,
            gap_start: content_len,
            gap_end: content_len + gap,
            line_starts: starts,
            line_ascii: ascii,
            min_dirty_line: usize::MAX,
            version: 0,
        }
    }

    /// Build a `GapBuffer` from a byte slice. Allocates a new `Vec` internally.
    /// Used in tests; production code should prefer `from_vec`.
    #[cfg(test)]
    pub fn from_text(text: &[u8]) -> Self {
        Self::from_vec(text.to_vec())
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    // -- low level helpers --------------------------------------------------

    fn len_logical(&self) -> usize {
        self.data.len() - self.gap_len()
    }

    fn gap_len(&self) -> usize {
        self.gap_end - self.gap_start
    }

    /// Convert a logical byte offset (ignoring the gap) to a physical index.
    fn logical_to_physical(&self, pos: usize) -> usize {
        if pos < self.gap_start {
            pos
        } else {
            pos + self.gap_len()
        }
    }

    fn move_gap_to(&mut self, pos: usize) {
        if pos == self.gap_start {
            return;
        }
        if pos < self.gap_start {
            let count = self.gap_start - pos;
            self.data
                .copy_within(pos..self.gap_start, self.gap_end - count);
            self.gap_start = pos;
            self.gap_end -= count;
        } else {
            let count = pos - self.gap_start;
            self.data
                .copy_within(self.gap_end..self.gap_end + count, self.gap_start);
            self.gap_start += count;
            self.gap_end += count;
        }
    }

    fn ensure_gap(&mut self, needed: usize) {
        if self.gap_len() >= needed {
            return;
        }
        let extra = needed.max(MIN_GAP);
        let old_gap_end = self.gap_end;
        let tail = self.data.len() - old_gap_end;
        self.data.resize(self.data.len() + extra, 0);
        // shift tail right
        self.data
            .copy_within(old_gap_end..old_gap_end + tail, old_gap_end + extra);
        self.gap_end += extra;
    }

    // -- public editing API -------------------------------------------------

    /// Insert `bytes` at logical byte offset `pos`.
    pub fn insert(&mut self, pos: usize, bytes: &[u8]) {
        assert!(pos <= self.len_logical());
        self.move_gap_to(pos);
        self.ensure_gap(bytes.len());
        self.data[self.gap_start..self.gap_start + bytes.len()].copy_from_slice(bytes);
        self.gap_start += bytes.len();
        self.update_line_index_insert(pos, bytes);
        self.version += 1;
    }

    /// Delete `count` bytes starting at logical byte offset `pos`.
    pub fn delete(&mut self, pos: usize, count: usize) {
        assert!(pos + count <= self.len_logical());
        self.move_gap_to(pos);
        self.gap_end += count;
        self.update_line_index_delete(pos, count);
        self.version += 1;
    }

    // -- read access --------------------------------------------------------

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.len_logical()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len_logical() == 0
    }

    /// Get the byte at logical offset `pos`.
    pub fn byte_at(&self, pos: usize) -> u8 {
        self.data[self.logical_to_physical(pos)]
    }

    /// Copy a logical byte range into `dst`.
    pub fn slice(&self, start: usize, end: usize) -> Vec<u8> {
        assert!(end <= self.len_logical());
        let mut out = Vec::with_capacity(end - start);
        if start < self.gap_start {
            let chunk_end = end.min(self.gap_start);
            out.extend_from_slice(&self.data[start..chunk_end]);
        }
        if end > self.gap_start {
            let phys_start = start.max(self.gap_start) + self.gap_len();
            let phys_end = end + self.gap_len();
            out.extend_from_slice(&self.data[phys_start..phys_end]);
        }
        out
    }

    /// Return all text as a contiguous `Vec<u8>`.
    pub fn contents(&self) -> Vec<u8> {
        self.slice(0, self.len_logical())
    }

    // -- line index ---------------------------------------------------------

    /// Incrementally update the line index after inserting `bytes` at logical offset `pos`.
    /// Must be called after the gap buffer has been updated.
    fn update_line_index_insert(&mut self, pos: usize, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let n = bytes.len();

        // Single binary search: derive both insert_line and shift_from.
        let (insert_line, shift_from) = match self.line_starts.binary_search(&pos) {
            Ok(i) => (i, i + 1), // exact match: line i contains pos, shift after it
            Err(i) => (i.saturating_sub(1), i), // between entries: line before, shift from i
        };
        for j in shift_from..self.line_starts.len() {
            self.line_starts[j] += n;
        }

        // Splice new line starts from newlines in the inserted bytes
        self.line_starts.splice(
            shift_from..shift_from,
            bytes.iter().enumerate().filter_map(|(k, &b)| {
                let s = pos + k + 1;
                (b == b'\n').then_some(s)
            }),
        );

        // Maintain per-line ASCII flags.
        let was_ascii = self.line_ascii[insert_line];
        let inserted_ascii = bytes.is_ascii();
        let has_newlines = bytes.contains(&b'\n');
        if !has_newlines {
            // Common case: no line splits — just taint if non-ASCII
            if !inserted_ascii {
                self.line_ascii[insert_line] = false;
            }
        } else {
            // Build per-segment ASCII flags with a single pass
            let mut new_flags: Vec<bool> = Vec::new();
            let mut seg_ascii = true;
            for &b in bytes {
                if b == b'\n' {
                    new_flags.push(seg_ascii);
                    seg_ascii = true;
                } else if b >= 0x80 {
                    seg_ascii = false;
                }
            }
            // First segment merges with old line prefix
            self.line_ascii[insert_line] = was_ascii && new_flags[0];
            // Middle segments are purely from inserted bytes; last merges with old suffix
            let last = new_flags.len();
            new_flags.push(was_ascii && seg_ascii);
            self.line_ascii
                .splice(shift_from..shift_from, new_flags[1..=last].iter().copied());
        }

        self.min_dirty_line = self.min_dirty_line.min(insert_line);
    }

    /// Incrementally update the line index after deleting `count` bytes at logical offset `pos`.
    /// Must be called after the gap buffer has been updated.
    fn update_line_index_delete(&mut self, pos: usize, count: usize) {
        if count == 0 {
            return;
        }

        let affected_line = match self.line_starts.binary_search(&pos) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };

        // Remove starts whose creating '\n' was in [pos, pos+count):
        // that means starts S where pos < S <= pos+count
        let lo = self.line_starts.partition_point(|&s| s <= pos);
        let hi = self.line_starts.partition_point(|&s| s <= pos + count);

        // Merged line is ASCII only if all merging lines were ASCII
        let merged_ascii =
            self.line_ascii[affected_line] && self.line_ascii[lo..hi].iter().all(|&a| a);
        self.line_ascii.drain(lo..hi);
        self.line_ascii[affected_line] = merged_ascii;

        self.line_starts.drain(lo..hi);

        // Shift surviving entries that are > pos
        for j in lo..self.line_starts.len() {
            self.line_starts[j] -= count;
        }

        self.min_dirty_line = self.min_dirty_line.min(affected_line);
    }

    /// Return and reset the minimum dirty line index. The renderer calls this
    /// to learn how far back it needs to recompute syntax highlighting.
    pub fn take_dirty_line(&mut self) -> usize {
        let v = self.min_dirty_line;
        self.min_dirty_line = usize::MAX;
        v
    }

    /// Returns true if every byte on `line` is ASCII (< 0x80).
    pub fn line_is_ascii(&self, line: usize) -> bool {
        self.line_ascii[line]
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte offset of the start of line `line` (0-indexed).
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line]
    }

    /// Byte offset one past the end of line `line` (exclusive, includes the '\n' if present).
    pub fn line_end(&self, line: usize) -> usize {
        if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1]
        } else {
            self.len_logical()
        }
    }

    /// Get the text of line `line` (0-indexed), without the trailing '\n'.
    pub fn line_text(&self, line: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        self.line_text_into(line, &mut buf);
        buf
    }

    /// Fill `buf` with the text of line `line` (0-indexed), without the trailing '\n'.
    /// Clears `buf` first; reuses its allocation when called repeatedly.
    pub fn line_text_into(&self, line: usize, buf: &mut Vec<u8>) {
        buf.clear();
        let start = self.line_start(line);
        let mut end = self.line_end(line);
        // Strip trailing newline
        if end > start && self.byte_at(end - 1) == b'\n' {
            end -= 1;
        }
        // Copy in (at most) two chunks, avoiding the gap
        if start < self.gap_start {
            let chunk_end = end.min(self.gap_start);
            buf.extend_from_slice(&self.data[start..chunk_end]);
        }
        if end > self.gap_start {
            let phys_start = start.max(self.gap_start) + self.gap_len();
            let phys_end = end + self.gap_len();
            buf.extend_from_slice(&self.data[phys_start..phys_end]);
        }
    }

    /// Compute the display column at char position `char_col` on `line`.
    /// Tabs count as 2 display columns; every other character counts as 1.
    /// Pass `char_col = usize::MAX` to get the total display width of the line.
    /// Does not allocate.
    pub fn display_col_at(&self, line: usize, char_col: usize) -> usize {
        let ls = self.line_start(line);
        let le = self.line_end(line);
        let mut display = 0usize;
        if self.line_ascii[line] {
            let end = ls.saturating_add(char_col).min(le);
            for bi in ls..end {
                let b = self.byte_at(bi);
                if b == b'\n' {
                    break;
                }
                display += if b == b'\t' { 2 } else { 1 };
            }
        } else {
            let mut ci = 0usize;
            let mut bi = ls;
            while ci < char_col && bi < le {
                let b = self.byte_at(bi);
                if b == b'\n' {
                    break;
                }
                display += if b == b'\t' { 2 } else { 1 };
                bi += utf8_char_len(b);
                ci += 1;
            }
        }
        display
    }

    /// Map a display column back to a char column on `line`.
    /// Inverse of `display_col_at`; reads the gap buffer directly.
    /// Does not allocate.
    pub fn char_col_from_display(&self, line: usize, target_display: usize) -> usize {
        let ls = self.line_start(line);
        let le = self.line_end(line);
        let mut display = 0usize;
        let mut ci = 0usize;
        if self.line_ascii[line] {
            for bi in ls..le {
                let b = self.byte_at(bi);
                if b == b'\n' {
                    break;
                }
                let w = if b == b'\t' { 2 } else { 1 };
                if display + w > target_display {
                    break;
                }
                display += w;
                ci += 1;
            }
        } else {
            let mut bi = ls;
            while bi < le {
                let b = self.byte_at(bi);
                if b == b'\n' {
                    break;
                }
                let w = if b == b'\t' { 2 } else { 1 };
                if display + w > target_display {
                    break;
                }
                display += w;
                bi += utf8_char_len(b);
                ci += 1;
            }
        }
        ci
    }

    /// Convert a (line, col) to a byte offset. Col is clamped to line length.
    pub fn pos_to_offset(&self, line: usize, col: usize) -> usize {
        let start = self.line_start(line);
        let end = self.line_end(line);
        // Exclude trailing newline from the clamping boundary (mirrors line_text behaviour).
        let limit = if end > start && self.byte_at(end - 1) == b'\n' {
            end - 1
        } else {
            end
        };
        if self.line_ascii[line] {
            start + col.min(limit - start)
        } else {
            let mut byte_off = 0;
            let mut char_idx = 0;
            while char_idx < col && start + byte_off < limit {
                let advance =
                    utf8_char_len(self.byte_at(start + byte_off)).min(limit - (start + byte_off));
                byte_off += advance;
                char_idx += 1;
            }
            start + byte_off
        }
    }

    /// Convert a byte offset to (line, col). Col is character count.
    pub fn offset_to_pos(&self, offset: usize) -> (usize, usize) {
        let starts = &self.line_starts;
        // binary search for the line
        let line = match starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l.saturating_sub(1),
        };
        let line_start = starts[line];
        if self.line_ascii[line] {
            (line, offset - line_start)
        } else {
            let col = self.char_count_in_range(line_start, offset);
            (line, col)
        }
    }

    fn char_count_in_range(&self, from: usize, to: usize) -> usize {
        let mut count = 0;
        let mut i = from;
        while i < to {
            let b = self.byte_at(i);
            i += utf8_char_len(b).min(to - i);
            count += 1;
        }
        count
    }

    /// Return the character count of a line (0-indexed), not counting the newline.
    pub fn line_char_len(&self, line: usize) -> usize {
        let start = self.line_start(line);
        let end = self.line_end(line);
        let end = if end > start && self.byte_at(end - 1) == b'\n' {
            end - 1
        } else {
            end
        };
        if self.line_ascii[line] {
            end - start
        } else {
            self.char_count_in_range(start, end)
        }
    }
}

pub fn utf8_char_len(first_byte: u8) -> usize {
    if first_byte < 0xC0 {
        // ASCII (0x00-0x7F) or continuation byte (0x80-0xBF).
        // Continuation bytes return 1 as a safe fallback for non-UTF-8 input.
        1
    } else if first_byte < 0xE0 {
        2
    } else if first_byte < 0xF0 {
        3
    } else {
        4
    }
}

pub fn char_count(bytes: &[u8]) -> usize {
    if bytes.is_ascii() {
        return bytes.len();
    }
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        i += utf8_char_len(bytes[i]);
        count += 1;
    }
    count
}

/// Convert a char column index to a byte offset in a UTF-8 byte slice.
/// Returns `bytes.len()` if `char_col` is past the end.
pub fn char_to_byte(bytes: &[u8], char_col: usize) -> usize {
    if bytes.is_ascii() {
        return char_col.min(bytes.len());
    }
    let mut bi = 0;
    let mut ci = 0;
    while ci < char_col && bi < bytes.len() {
        bi += utf8_char_len(bytes[bi]);
        ci += 1;
    }
    bi
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- basic insert/delete ------------------------------------------------

    #[test]
    fn test_insert_and_contents() {
        let mut buf = GapBuffer::new();
        buf.insert(0, b"hello");
        assert_eq!(buf.contents(), b"hello");
        buf.insert(5, b" world");
        assert_eq!(buf.contents(), b"hello world");
    }

    #[test]
    fn test_insert_at_beginning() {
        let mut buf = GapBuffer::from_text(b"world");
        buf.insert(0, b"hello ");
        assert_eq!(buf.contents(), b"hello world");
    }

    #[test]
    fn test_insert_in_middle() {
        let mut buf = GapBuffer::from_text(b"hllo");
        buf.insert(1, b"e");
        assert_eq!(buf.contents(), b"hello");
    }

    #[test]
    fn test_delete() {
        let mut buf = GapBuffer::from_text(b"hello world");
        buf.delete(5, 6);
        assert_eq!(buf.contents(), b"hello");
    }

    #[test]
    fn test_delete_at_beginning() {
        let mut buf = GapBuffer::from_text(b"hello world");
        buf.delete(0, 6);
        assert_eq!(buf.contents(), b"world");
    }

    #[test]
    fn test_delete_at_end() {
        let mut buf = GapBuffer::from_text(b"hello world");
        buf.delete(5, 6);
        assert_eq!(buf.contents(), b"hello");
    }

    #[test]
    fn test_delete_everything() {
        let mut buf = GapBuffer::from_text(b"hello");
        buf.delete(0, 5);
        assert_eq!(buf.contents(), b"");
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_multiple_inserts_and_deletes() {
        let mut buf = GapBuffer::new();
        buf.insert(0, b"abc");
        buf.insert(3, b"ghi");
        buf.insert(3, b"def");
        assert_eq!(buf.contents(), b"abcdefghi");
        buf.delete(3, 3);
        assert_eq!(buf.contents(), b"abcghi");
    }

    // -- len / is_empty -----------------------------------------------------

    #[test]
    fn test_empty_buffer() {
        let buf = GapBuffer::new();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_len_after_operations() {
        let mut buf = GapBuffer::from_text(b"hello");
        assert_eq!(buf.len(), 5);
        buf.insert(5, b" world");
        assert_eq!(buf.len(), 11);
        buf.delete(0, 6);
        assert_eq!(buf.len(), 5);
    }

    // -- byte_at / slice ----------------------------------------------------

    #[test]
    fn test_byte_at() {
        let buf = GapBuffer::from_text(b"abcde");
        assert_eq!(buf.byte_at(0), b'a');
        assert_eq!(buf.byte_at(2), b'c');
        assert_eq!(buf.byte_at(4), b'e');
    }

    #[test]
    fn test_byte_at_after_gap_move() {
        let mut buf = GapBuffer::from_text(b"abcde");
        buf.insert(2, b"XX");
        assert_eq!(buf.byte_at(0), b'a');
        assert_eq!(buf.byte_at(1), b'b');
        assert_eq!(buf.byte_at(2), b'X');
        assert_eq!(buf.byte_at(3), b'X');
        assert_eq!(buf.byte_at(4), b'c');
    }

    #[test]
    fn test_slice_within_one_segment() {
        let buf = GapBuffer::from_text(b"hello world");
        assert_eq!(buf.slice(0, 5), b"hello");
        assert_eq!(buf.slice(6, 11), b"world");
    }

    #[test]
    fn test_slice_across_gap() {
        let mut buf = GapBuffer::from_text(b"abcdefgh");
        // Move gap to position 4
        buf.insert(4, b"");
        assert_eq!(buf.slice(2, 6), b"cdef");
    }

    #[test]
    fn test_slice_empty() {
        let buf = GapBuffer::from_text(b"hello");
        assert_eq!(buf.slice(3, 3), b"");
    }

    // -- line index ---------------------------------------------------------

    #[test]
    fn test_line_index() {
        let buf = GapBuffer::from_text(b"line1\nline2\nline3");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"line1");
        assert_eq!(buf.line_text(1), b"line2");
        assert_eq!(buf.line_text(2), b"line3");
    }

    #[test]
    fn test_single_line_no_newline() {
        let buf = GapBuffer::from_text(b"hello");
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"hello");
    }

    #[test]
    fn test_empty_lines() {
        let buf = GapBuffer::from_text(b"a\n\nb");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"a");
        assert_eq!(buf.line_text(1), b"");
        assert_eq!(buf.line_text(2), b"b");
    }

    #[test]
    fn test_trailing_newline() {
        let buf = GapBuffer::from_text(b"hello\n");
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"hello");
    }

    #[test]
    fn test_only_newlines() {
        let buf = GapBuffer::from_text(b"\n\n\n");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"");
        assert_eq!(buf.line_text(1), b"");
        assert_eq!(buf.line_text(2), b"");
    }

    #[test]
    fn test_line_start_and_end() {
        let buf = GapBuffer::from_text(b"abc\ndef\nghi");
        assert_eq!(buf.line_start(0), 0);
        assert_eq!(buf.line_end(0), 4); // includes \n
        assert_eq!(buf.line_start(1), 4);
        assert_eq!(buf.line_end(1), 8);
        assert_eq!(buf.line_start(2), 8);
        assert_eq!(buf.line_end(2), 11); // end of buffer
    }

    #[test]
    fn test_line_char_len() {
        let buf = GapBuffer::from_text(b"abc\nde\nfghij");
        assert_eq!(buf.line_char_len(0), 3);
        assert_eq!(buf.line_char_len(1), 2);
        assert_eq!(buf.line_char_len(2), 5);
    }

    #[test]
    fn test_line_index_after_insert() {
        let mut buf = GapBuffer::from_text(b"ab\ncd");
        assert_eq!(buf.line_count(), 2);
        buf.insert(2, b"\nXX"); // insert newline+text before the existing \n
        // now: "ab\nXX\ncd"
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"ab");
        assert_eq!(buf.line_text(1), b"XX");
        assert_eq!(buf.line_text(2), b"cd");
    }

    #[test]
    fn test_line_index_after_delete() {
        let mut buf = GapBuffer::from_text(b"ab\ncd\nef");
        assert_eq!(buf.line_count(), 3);
        buf.delete(2, 1); // delete the first \n
        // now: "abcd\nef"
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line_text(0), b"abcd");
        assert_eq!(buf.line_text(1), b"ef");
    }

    // -- pos_to_offset / offset_to_pos --------------------------------------

    #[test]
    fn test_pos_to_offset() {
        let buf = GapBuffer::from_text(b"abc\ndef\nghi");
        assert_eq!(buf.pos_to_offset(0, 0), 0);
        assert_eq!(buf.pos_to_offset(1, 0), 4);
        assert_eq!(buf.pos_to_offset(1, 2), 6);
        assert_eq!(buf.pos_to_offset(2, 3), 11);
    }

    #[test]
    fn test_offset_to_pos() {
        let buf = GapBuffer::from_text(b"abc\ndef\nghi");
        assert_eq!(buf.offset_to_pos(0), (0, 0));
        assert_eq!(buf.offset_to_pos(4), (1, 0));
        assert_eq!(buf.offset_to_pos(6), (1, 2));
    }

    #[test]
    fn test_pos_to_offset_col_clamped() {
        let buf = GapBuffer::from_text(b"ab\ncd");
        // col 10 on a 2-char line should clamp to end
        assert_eq!(buf.pos_to_offset(0, 10), 2);
    }

    #[test]
    fn test_offset_to_pos_at_newline() {
        let buf = GapBuffer::from_text(b"abc\ndef");
        // Offset 3 is the newline itself, which is col 3 of line 0
        assert_eq!(buf.offset_to_pos(3), (0, 3));
    }

    // -- UTF-8 handling -----------------------------------------------------

    #[test]
    fn test_utf8_char_len_function() {
        assert_eq!(utf8_char_len(b'a'), 1);
        assert_eq!(utf8_char_len(0xC3), 2); // start of 2-byte
        assert_eq!(utf8_char_len(0xE4), 3); // start of 3-byte
        assert_eq!(utf8_char_len(0xF0), 4); // start of 4-byte
    }

    #[test]
    fn test_char_count_ascii() {
        assert_eq!(char_count(b"hello"), 5);
        assert_eq!(char_count(b""), 0);
    }

    #[test]
    fn test_char_count_utf8() {
        // "café" = 63 61 66 c3 a9 = 5 bytes, 4 chars
        assert_eq!(char_count("café".as_bytes()), 4);
        // "日本" = 3 bytes each = 6 bytes, 2 chars
        assert_eq!(char_count("日本".as_bytes()), 2);
    }

    #[test]
    fn test_utf8_insert_and_line_char_len() {
        let buf = GapBuffer::from_text("café".as_bytes());
        assert_eq!(buf.line_char_len(0), 4);
        assert_eq!(buf.line_count(), 1);
    }

    #[test]
    fn test_utf8_pos_to_offset() {
        // "aé" = 61 c3 a9 = 3 bytes
        let buf = GapBuffer::from_text("aé".as_bytes());
        assert_eq!(buf.pos_to_offset(0, 0), 0); // 'a' at byte 0
        assert_eq!(buf.pos_to_offset(0, 1), 1); // 'é' at byte 1
        assert_eq!(buf.pos_to_offset(0, 2), 3); // end
    }

    #[test]
    fn test_utf8_offset_to_pos() {
        let buf = GapBuffer::from_text("aé".as_bytes());
        assert_eq!(buf.offset_to_pos(0), (0, 0)); // 'a'
        assert_eq!(buf.offset_to_pos(1), (0, 1)); // 'é'
        assert_eq!(buf.offset_to_pos(3), (0, 2)); // end
    }

    // -- gap buffer stress --------------------------------------------------

    #[test]
    fn test_many_small_inserts() {
        let mut buf = GapBuffer::new();
        for i in 0..1000 {
            let byte = [(i % 26 + 65) as u8]; // A-Z
            buf.insert(buf.len(), &byte);
        }
        assert_eq!(buf.len(), 1000);
    }

    #[test]
    fn test_insert_then_delete_all() {
        let mut buf = GapBuffer::new();
        buf.insert(0, b"hello world");
        buf.delete(0, 11);
        assert!(buf.is_empty());
        assert_eq!(buf.contents(), b"");
    }

    #[test]
    fn test_alternating_insert_delete() {
        let mut buf = GapBuffer::new();
        buf.insert(0, b"abcdef");
        buf.delete(2, 2); // "abef"
        buf.insert(2, b"XX"); // "abXXef"
        buf.delete(0, 2); // "XXef"
        buf.insert(4, b"YY"); // "XXefYY"
        assert_eq!(buf.contents(), b"XXefYY");
    }

    // -- empty buffer edge cases --------------------------------------------

    #[test]
    fn test_empty_buffer_line_count() {
        let buf = GapBuffer::new();
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"");
        assert_eq!(buf.line_char_len(0), 0);
    }

    #[test]
    fn test_contents_matches_slice_full() {
        let buf = GapBuffer::from_text(b"some text here");
        assert_eq!(buf.contents(), buf.slice(0, buf.len()));
    }

    // -- incremental line index edge cases ----------------------------------

    #[test]
    fn test_insert_newline_at_start() {
        let mut buf = GapBuffer::from_text(b"hello");
        buf.insert(0, b"\n");
        // now: "\nhello"
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line_text(0), b"");
        assert_eq!(buf.line_text(1), b"hello");
    }

    #[test]
    fn test_delete_all_content_multiline() {
        let mut buf = GapBuffer::from_text(b"a\nb\nc");
        buf.delete(0, buf.len());
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"");
    }

    #[test]
    fn test_multiple_edits_then_query() {
        let mut buf = GapBuffer::from_text(b"aa\nbb\ncc");
        buf.insert(2, b"\nXX"); // "aa\nXX\nbb\ncc"
        buf.delete(0, 3); // "XX\nbb\ncc"
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"XX");
        assert_eq!(buf.line_text(1), b"bb");
        assert_eq!(buf.line_text(2), b"cc");
    }

    #[test]
    fn test_insert_trailing_newline() {
        // Inserting \n at end of buffer creates a new (empty) line
        let mut buf = GapBuffer::from_text(b"hello");
        buf.insert(5, b"\n");
        // "hello\n" — 2 lines: "hello" and ""
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line_text(0), b"hello");
        assert_eq!(buf.line_text(1), b"");
    }

    #[test]
    fn test_delete_newline_merges_lines() {
        let mut buf = GapBuffer::from_text(b"foo\nbar\nbaz");
        buf.delete(7, 1); // delete '\n' between bar and baz
        // "foo\nbarbaz"
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line_text(0), b"foo");
        assert_eq!(buf.line_text(1), b"barbaz");
    }

    #[test]
    fn test_take_dirty_line_resets() {
        let mut buf = GapBuffer::from_text(b"a\nb\nc");
        buf.insert(2, b"X"); // dirty line 1
        let d1 = buf.take_dirty_line();
        assert!(d1 < usize::MAX);
        // After take, resets to MAX
        let d2 = buf.take_dirty_line();
        assert_eq!(d2, usize::MAX);
    }

    #[test]
    fn test_take_dirty_line_accumulates_min() {
        let mut buf = GapBuffer::from_text(b"a\nb\nc\nd\ne");
        buf.insert(6, b"X"); // line 2 (c line)
        buf.insert(2, b"Y"); // line 1 (b line) — should be the min
        let d = buf.take_dirty_line();
        assert_eq!(d, 1); // min of line 2 and line 1
    }

    // -- line_ascii flag -------------------------------------------------------

    #[test]
    fn test_ascii_flag_pure_ascii() {
        let buf = GapBuffer::from_text(b"hello\nworld\n");
        assert!(buf.line_is_ascii(0));
        assert!(buf.line_is_ascii(1));
    }

    #[test]
    fn test_ascii_flag_utf8_line() {
        let buf = GapBuffer::from_text("hello\ncaf\u{e9}\nworld".as_bytes());
        assert!(buf.line_is_ascii(0));
        assert!(!buf.line_is_ascii(1)); // "café" has non-ASCII
        assert!(buf.line_is_ascii(2));
    }

    #[test]
    fn test_ascii_flag_insert_ascii() {
        let mut buf = GapBuffer::from_text(b"ab\ncd");
        assert!(buf.line_is_ascii(0));
        buf.insert(1, b"X");
        assert!(buf.line_is_ascii(0)); // still ASCII
    }

    #[test]
    fn test_ascii_flag_insert_utf8() {
        let mut buf = GapBuffer::from_text(b"ab\ncd");
        assert!(buf.line_is_ascii(0));
        buf.insert(1, "\u{e9}".as_bytes()); // insert é
        assert!(!buf.line_is_ascii(0)); // now non-ASCII
        assert!(buf.line_is_ascii(1)); // other line unaffected
    }

    #[test]
    fn test_ascii_flag_insert_newline_splits() {
        let mut buf = GapBuffer::from_text(b"abcd");
        assert_eq!(buf.line_count(), 1);
        buf.insert(2, b"\n"); // split into "ab" and "cd"
        assert_eq!(buf.line_count(), 2);
        assert!(buf.line_is_ascii(0));
        assert!(buf.line_is_ascii(1));
    }

    #[test]
    fn test_ascii_flag_insert_newline_with_utf8() {
        let mut buf = GapBuffer::from_text(b"abcd");
        buf.insert(2, "x\u{e9}\ny".as_bytes()); // insert "xé\ny"
        assert_eq!(buf.line_count(), 2);
        assert!(!buf.line_is_ascii(0)); // "abxé" has non-ASCII
        assert!(buf.line_is_ascii(1)); // "ycd" is ASCII
    }

    #[test]
    fn test_ascii_flag_delete_merges_lines() {
        let mut buf = GapBuffer::from_text(b"ab\ncd");
        assert!(buf.line_is_ascii(0));
        assert!(buf.line_is_ascii(1));
        buf.delete(2, 1); // delete the \n, merging into "abcd"
        assert_eq!(buf.line_count(), 1);
        assert!(buf.line_is_ascii(0));
    }

    #[test]
    fn test_ascii_flag_delete_merge_preserves_non_ascii() {
        let mut buf = GapBuffer::from_text("ab\ncaf\u{e9}".as_bytes());
        assert!(buf.line_is_ascii(0));
        assert!(!buf.line_is_ascii(1));
        buf.delete(2, 1); // delete \n, merge into "abcafé"
        assert_eq!(buf.line_count(), 1);
        assert!(!buf.line_is_ascii(0)); // merged line is non-ASCII
    }

    #[test]
    fn test_ascii_fast_path_line_char_len() {
        let buf = GapBuffer::from_text("hello\ncaf\u{e9}".as_bytes());
        assert_eq!(buf.line_char_len(0), 5); // ASCII: fast path
        assert_eq!(buf.line_char_len(1), 4); // "café" = 4 chars, 5 bytes
    }

    #[test]
    fn test_ascii_fast_path_pos_to_offset() {
        let buf = GapBuffer::from_text("hello\ncaf\u{e9}".as_bytes());
        // ASCII line: col == byte offset from line start
        assert_eq!(buf.pos_to_offset(0, 3), 3);
        // UTF-8 line: col 3 → byte offset for 'é' (byte 9)
        assert_eq!(buf.pos_to_offset(1, 3), 9);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// An edit operation for property testing.
    #[derive(Debug, Clone)]
    enum EditOp {
        Insert { pos_frac: f64, data: Vec<u8> },
        Delete { pos_frac: f64, len_frac: f64 },
    }

    fn arb_edit_op() -> impl Strategy<Value = EditOp> {
        prop_oneof![
            (any::<f64>(), prop::collection::vec(any::<u8>(), 0..32))
                .prop_map(|(pos_frac, data)| EditOp::Insert { pos_frac, data }),
            (any::<f64>(), any::<f64>())
                .prop_map(|(pos_frac, len_frac)| EditOp::Delete { pos_frac, len_frac }),
        ]
    }

    /// Apply an edit op to both a GapBuffer and a reference Vec<u8>.
    fn apply_op(buf: &mut GapBuffer, reference: &mut Vec<u8>, op: &EditOp) {
        match op {
            EditOp::Insert { pos_frac, data } => {
                if data.is_empty() {
                    return;
                }
                let pos = if reference.is_empty() {
                    0
                } else {
                    (pos_frac.abs().fract() * reference.len() as f64) as usize
                        % (reference.len() + 1)
                };
                buf.insert(pos, data);
                reference.splice(pos..pos, data.iter().copied());
            }
            EditOp::Delete { pos_frac, len_frac } => {
                if reference.is_empty() {
                    return;
                }
                let pos =
                    (pos_frac.abs().fract() * reference.len() as f64) as usize % reference.len();
                let max_len = reference.len() - pos;
                if max_len == 0 {
                    return;
                }
                let count = ((len_frac.abs().fract() * max_len as f64) as usize)
                    .max(1)
                    .min(max_len);
                buf.delete(pos, count);
                reference.drain(pos..pos + count);
            }
        }
    }

    proptest! {
        /// After any sequence of edits, contents() matches the reference.
        #[test]
        fn contents_match_reference(
            initial in prop::collection::vec(any::<u8>(), 0..256),
            ops in prop::collection::vec(arb_edit_op(), 0..50),
        ) {
            let mut buf = GapBuffer::from_vec(initial.clone());
            let mut reference = initial;
            for op in &ops {
                apply_op(&mut buf, &mut reference, op);
            }
            prop_assert_eq!(buf.contents(), reference);
        }

        /// The line index correctly partitions the buffer: line ranges
        /// are contiguous, cover the entire buffer, and slice() for each
        /// line range reconstructs the original content.
        #[test]
        fn line_index_partitions_buffer(
            initial in prop::collection::vec(any::<u8>(), 0..256),
            ops in prop::collection::vec(arb_edit_op(), 0..50),
        ) {
            let mut buf = GapBuffer::from_vec(initial.clone());
            let mut reference = initial;
            for op in &ops {
                apply_op(&mut buf, &mut reference, op);
            }
            let contents = buf.contents();
            let lc = buf.line_count();
            prop_assert!(lc >= 1, "always at least 1 line");
            prop_assert_eq!(buf.line_start(0), 0, "first line starts at 0");
            prop_assert_eq!(buf.line_end(lc - 1), contents.len(), "last line ends at len");

            // Lines are contiguous and reconstruct the full content.
            let mut reconstructed = Vec::new();
            for i in 0..lc {
                if i + 1 < lc {
                    prop_assert_eq!(buf.line_end(i), buf.line_start(i + 1),
                        "gap between line {} end and line {} start", i, i + 1);
                }
                reconstructed.extend_from_slice(&buf.slice(buf.line_start(i), buf.line_end(i)));
            }
            prop_assert_eq!(reconstructed, contents);
        }

        /// pos_to_offset and offset_to_pos are inverses for valid positions.
        #[test]
        fn pos_offset_roundtrip(
            text in prop::collection::vec(any::<u8>(), 0..256),
        ) {
            let buf = GapBuffer::from_vec(text);
            for line in 0..buf.line_count() {
                let char_len = buf.line_char_len(line);
                for col in 0..=char_len {
                    let offset = buf.pos_to_offset(line, col);
                    let (rt_line, rt_col) = buf.offset_to_pos(offset);
                    prop_assert_eq!((rt_line, rt_col), (line, col),
                        "roundtrip failed for ({}, {}): offset={}, got ({}, {})",
                        line, col, offset, rt_line, rt_col);
                }
            }
        }

        /// line_start(n+1) == line_end(n) for adjacent lines (lines end with '\n').
        #[test]
        fn line_boundaries_contiguous(
            text in prop::collection::vec(any::<u8>(), 1..256),
        ) {
            let buf = GapBuffer::from_vec(text);
            let lc = buf.line_count();
            for line in 0..lc.saturating_sub(1) {
                prop_assert_eq!(
                    buf.line_start(line + 1),
                    buf.line_end(line),
                    "gap between line {} end and line {} start", line, line + 1
                );
            }
        }

        /// len() is always consistent with contents().len().
        #[test]
        fn len_matches_contents(
            initial in prop::collection::vec(any::<u8>(), 0..256),
            ops in prop::collection::vec(arb_edit_op(), 0..50),
        ) {
            let mut buf = GapBuffer::from_vec(initial.clone());
            let mut reference = initial;
            for op in &ops {
                apply_op(&mut buf, &mut reference, op);
            }
            prop_assert_eq!(buf.len(), buf.contents().len());
            prop_assert_eq!(buf.len(), reference.len());
        }

        /// Insert then delete at the same position restores original content.
        #[test]
        fn insert_delete_roundtrip(
            initial in prop::collection::vec(any::<u8>(), 0..128),
            insert_data in prop::collection::vec(any::<u8>(), 1..32),
            pos_frac in 0.0f64..1.0,
        ) {
            let mut buf = GapBuffer::from_vec(initial.clone());
            let pos = if initial.is_empty() {
                0
            } else {
                (pos_frac * initial.len() as f64) as usize % (initial.len() + 1)
            };
            buf.insert(pos, &insert_data);
            buf.delete(pos, insert_data.len());
            prop_assert_eq!(buf.contents(), initial);
        }

        /// display_col_at round-trips through char_col_from_display for ASCII.
        #[test]
        fn display_col_roundtrip_ascii(
            text in "[a-z\t ]{0,80}\n[a-z\t ]{0,80}",
        ) {
            let buf = GapBuffer::from_vec(text.into_bytes());
            for line in 0..buf.line_count() {
                let char_len = buf.line_char_len(line);
                for col in 0..=char_len {
                    let dcol = buf.display_col_at(line, col);
                    let rt_col = buf.char_col_from_display(line, dcol);
                    prop_assert_eq!(rt_col, col,
                        "display roundtrip failed on line {} col {}: dcol={} rt={}",
                        line, col, dcol, rt_col);
                }
            }
        }
    }
}
