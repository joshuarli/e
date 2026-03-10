#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use e::document::Document;
use e::selection::Pos;

#[derive(Debug, Arbitrary)]
enum Op {
    Insert { line_pct: u8, col_pct: u8, data: Vec<u8> },
    Delete { l1_pct: u8, c1_pct: u8, l2_pct: u8, c2_pct: u8 },
    Undo,
    Redo,
    Seal,
}

fn clamp_pos(doc: &Document, line_pct: u8, col_pct: u8) -> Pos {
    let lc = doc.buf.line_count();
    let line = (line_pct as usize) % lc;
    let char_len = doc.buf.line_char_len(line);
    let col = if char_len == 0 { 0 } else { (col_pct as usize) % (char_len + 1) };
    Pos::new(line, col)
}

fuzz_target!(|ops: Vec<Op>| {
    let mut doc = Document::new(Vec::new(), None);

    for op in &ops {
        match op {
            Op::Insert { line_pct, col_pct, data } => {
                if data.is_empty() || data.len() > 512 {
                    continue;
                }
                let pos = clamp_pos(&doc, *line_pct, *col_pct);
                doc.insert(pos.line, pos.col, data);
            }
            Op::Delete { l1_pct, c1_pct, l2_pct, c2_pct } => {
                let start = clamp_pos(&doc, *l1_pct, *c1_pct);
                let end = clamp_pos(&doc, *l2_pct, *c2_pct);
                if start < end {
                    doc.delete_range(start, end);
                }
            }
            Op::Undo => { doc.undo(); }
            Op::Redo => { doc.redo(); }
            Op::Seal => { doc.seal_undo(); }
        }

        // Invariant: buffer must be internally consistent after every op
        let lc = doc.buf.line_count();
        assert!(lc >= 1);
        assert_eq!(doc.buf.line_start(0), 0);
        assert_eq!(doc.buf.line_end(lc - 1), doc.buf.len());
    }
});
