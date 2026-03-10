#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use e::buffer::GapBuffer;

#[derive(Debug, Arbitrary)]
enum Op {
    Insert { pos_pct: u8, data: Vec<u8> },
    Delete { pos_pct: u8, len_pct: u8 },
}

fuzz_target!(|ops: Vec<Op>| {
    let mut buf = GapBuffer::new();
    let mut reference: Vec<u8> = Vec::new();

    for op in &ops {
        match op {
            Op::Insert { pos_pct, data } => {
                if data.is_empty() || data.len() > 1024 {
                    continue;
                }
                let pos = if reference.is_empty() {
                    0
                } else {
                    (*pos_pct as usize) % (reference.len() + 1)
                };
                buf.insert(pos, data);
                reference.splice(pos..pos, data.iter().copied());
            }
            Op::Delete { pos_pct, len_pct } => {
                if reference.is_empty() {
                    continue;
                }
                let pos = (*pos_pct as usize) % reference.len();
                let max_len = reference.len() - pos;
                if max_len == 0 {
                    continue;
                }
                let count = ((*len_pct as usize) % max_len).max(1);
                buf.delete(pos, count);
                reference.drain(pos..pos + count);
            }
        }
    }

    // Invariant: contents must match reference
    assert_eq!(buf.contents(), reference);

    // Invariant: line count must match
    let expected_lines = reference.iter().filter(|&&b| b == b'\n').count() + 1;
    assert_eq!(buf.line_count(), expected_lines);

    // Invariant: len must match
    assert_eq!(buf.len(), reference.len());

    // Invariant: pos_to_offset/offset_to_pos roundtrip
    for line in 0..buf.line_count() {
        let char_len = buf.line_char_len(line);
        for col in 0..=char_len {
            let offset = buf.pos_to_offset(line, col);
            let (rt_line, rt_col) = buf.offset_to_pos(offset);
            assert_eq!((rt_line, rt_col), (line, col));
        }
    }
});
