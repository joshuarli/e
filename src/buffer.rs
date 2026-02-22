/// Gap buffer backed by a `Vec<u8>` with a lazy line-start index.
///
/// Text is stored as UTF-8 bytes. The gap sits between `gap_start` and `gap_end`
/// inside `data`. Insertions at the cursor just fill the gap; deletions widen it.
pub struct GapBuffer {
    data: Vec<u8>,
    gap_start: usize,
    gap_end: usize,
    /// Cached byte offsets of line starts (each entry is the byte offset of the
    /// first character on that line, with line 0 always == 0).  `None` means the
    /// cache is fully invalidated and must be rebuilt.
    line_starts: Option<Vec<usize>>,
}

const MIN_GAP: usize = 128;

impl GapBuffer {
    pub fn new() -> Self {
        let gap = MIN_GAP;
        Self {
            data: vec![0; gap],
            gap_start: 0,
            gap_end: gap,
            line_starts: None,
        }
    }

    pub fn from_text(text: &[u8]) -> Self {
        let gap = MIN_GAP;
        let mut data = Vec::with_capacity(text.len() + gap);
        data.extend_from_slice(text);
        data.resize(text.len() + gap, 0);
        Self {
            data,
            gap_start: text.len(),
            gap_end: text.len() + gap,
            line_starts: None,
        }
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
        self.invalidate_lines_from(pos);
    }

    /// Delete `count` bytes starting at logical byte offset `pos`.
    pub fn delete(&mut self, pos: usize, count: usize) {
        assert!(pos + count <= self.len_logical());
        self.move_gap_to(pos);
        self.gap_end += count;
        self.invalidate_lines_from(pos);
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

    fn invalidate_lines_from(&mut self, _byte_pos: usize) {
        // Simple approach: full invalidation. A smarter version would only
        // invalidate from `_byte_pos` downward.
        self.line_starts = None;
    }

    fn rebuild_line_index(&mut self) {
        let mut starts = vec![0usize];
        let len = self.len_logical();
        for i in 0..len {
            if self.byte_at(i) == b'\n' && i + 1 < len {
                starts.push(i + 1);
            }
        }
        self.line_starts = Some(starts);
    }

    fn ensure_line_index(&mut self) {
        if self.line_starts.is_none() {
            self.rebuild_line_index();
        }
    }

    pub fn line_count(&mut self) -> usize {
        self.ensure_line_index();
        self.line_starts.as_ref().unwrap().len()
    }

    /// Byte offset of the start of line `line` (0-indexed).
    pub fn line_start(&mut self, line: usize) -> usize {
        self.ensure_line_index();
        self.line_starts.as_ref().unwrap()[line]
    }

    /// Byte offset one past the end of line `line` (exclusive, includes the '\n' if present).
    pub fn line_end(&mut self, line: usize) -> usize {
        self.ensure_line_index();
        let starts = self.line_starts.as_ref().unwrap();
        if line + 1 < starts.len() {
            starts[line + 1]
        } else {
            self.len_logical()
        }
    }

    /// Get the text of line `line` (0-indexed), without the trailing '\n'.
    pub fn line_text(&mut self, line: usize) -> Vec<u8> {
        let start = self.line_start(line);
        let end = self.line_end(line);
        let raw = self.slice(start, end);
        if raw.last() == Some(&b'\n') {
            raw[..raw.len() - 1].to_vec()
        } else {
            raw
        }
    }

    /// Convert a (line, col) to a byte offset. Col is clamped to line length.
    pub fn pos_to_offset(&mut self, line: usize, col: usize) -> usize {
        let start = self.line_start(line);
        let text = self.line_text(line);
        // Walk UTF-8 chars to find byte offset of the col-th character
        let mut byte_off = 0;
        let mut char_idx = 0;
        while char_idx < col && byte_off < text.len() {
            let b = text[byte_off];
            let char_len = utf8_char_len(b);
            byte_off += char_len;
            char_idx += 1;
        }
        start + byte_off
    }

    /// Convert a byte offset to (line, col). Col is character count.
    pub fn offset_to_pos(&mut self, offset: usize) -> (usize, usize) {
        self.ensure_line_index();
        let starts = self.line_starts.as_ref().unwrap();
        // binary search for the line
        let line = match starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l.saturating_sub(1),
        };
        let line_start = starts[line];
        // count chars from line_start to offset
        let col = self.char_count_in_range(line_start, offset);
        (line, col)
    }

    fn char_count_in_range(&self, from: usize, to: usize) -> usize {
        let mut count = 0;
        let mut i = from;
        while i < to {
            let b = self.byte_at(i);
            i += utf8_char_len(b);
            count += 1;
        }
        count
    }

    /// Return the character count of a line (0-indexed), not counting the newline.
    pub fn line_char_len(&mut self, line: usize) -> usize {
        let text = self.line_text(line);
        char_count(&text)
    }
}

pub fn utf8_char_len(first_byte: u8) -> usize {
    if first_byte < 0x80 {
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
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        i += utf8_char_len(bytes[i]);
        count += 1;
    }
    count
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
        let mut buf = GapBuffer::from_text(b"line1\nline2\nline3");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"line1");
        assert_eq!(buf.line_text(1), b"line2");
        assert_eq!(buf.line_text(2), b"line3");
    }

    #[test]
    fn test_single_line_no_newline() {
        let mut buf = GapBuffer::from_text(b"hello");
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"hello");
    }

    #[test]
    fn test_empty_lines() {
        let mut buf = GapBuffer::from_text(b"a\n\nb");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"a");
        assert_eq!(buf.line_text(1), b"");
        assert_eq!(buf.line_text(2), b"b");
    }

    #[test]
    fn test_trailing_newline() {
        let mut buf = GapBuffer::from_text(b"hello\n");
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"hello");
    }

    #[test]
    fn test_only_newlines() {
        let mut buf = GapBuffer::from_text(b"\n\n\n");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_text(0), b"");
        assert_eq!(buf.line_text(1), b"");
        assert_eq!(buf.line_text(2), b"");
    }

    #[test]
    fn test_line_start_and_end() {
        let mut buf = GapBuffer::from_text(b"abc\ndef\nghi");
        assert_eq!(buf.line_start(0), 0);
        assert_eq!(buf.line_end(0), 4); // includes \n
        assert_eq!(buf.line_start(1), 4);
        assert_eq!(buf.line_end(1), 8);
        assert_eq!(buf.line_start(2), 8);
        assert_eq!(buf.line_end(2), 11); // end of buffer
    }

    #[test]
    fn test_line_char_len() {
        let mut buf = GapBuffer::from_text(b"abc\nde\nfghij");
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
        let mut buf = GapBuffer::from_text(b"abc\ndef\nghi");
        assert_eq!(buf.pos_to_offset(0, 0), 0);
        assert_eq!(buf.pos_to_offset(1, 0), 4);
        assert_eq!(buf.pos_to_offset(1, 2), 6);
        assert_eq!(buf.pos_to_offset(2, 3), 11);
    }

    #[test]
    fn test_offset_to_pos() {
        let mut buf = GapBuffer::from_text(b"abc\ndef\nghi");
        assert_eq!(buf.offset_to_pos(0), (0, 0));
        assert_eq!(buf.offset_to_pos(4), (1, 0));
        assert_eq!(buf.offset_to_pos(6), (1, 2));
    }

    #[test]
    fn test_pos_to_offset_col_clamped() {
        let mut buf = GapBuffer::from_text(b"ab\ncd");
        // col 10 on a 2-char line should clamp to end
        assert_eq!(buf.pos_to_offset(0, 10), 2);
    }

    #[test]
    fn test_offset_to_pos_at_newline() {
        let mut buf = GapBuffer::from_text(b"abc\ndef");
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
        let mut buf = GapBuffer::from_text("café".as_bytes());
        assert_eq!(buf.line_char_len(0), 4);
        assert_eq!(buf.line_count(), 1);
    }

    #[test]
    fn test_utf8_pos_to_offset() {
        // "aé" = 61 c3 a9 = 3 bytes
        let mut buf = GapBuffer::from_text("aé".as_bytes());
        assert_eq!(buf.pos_to_offset(0, 0), 0); // 'a' at byte 0
        assert_eq!(buf.pos_to_offset(0, 1), 1); // 'é' at byte 1
        assert_eq!(buf.pos_to_offset(0, 2), 3); // end
    }

    #[test]
    fn test_utf8_offset_to_pos() {
        let mut buf = GapBuffer::from_text("aé".as_bytes());
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
        let mut buf = GapBuffer::new();
        assert_eq!(buf.line_count(), 1);
        assert_eq!(buf.line_text(0), b"");
        assert_eq!(buf.line_char_len(0), 0);
    }

    #[test]
    fn test_contents_matches_slice_full() {
        let buf = GapBuffer::from_text(b"some text here");
        assert_eq!(buf.contents(), buf.slice(0, buf.len()));
    }
}
