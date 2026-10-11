use jayjay_core::diff::{DiffSide, SbsVisualLine};

/// Words per visual line: row index, old start and end, new start and end, full-width source.
const WORDS_PER_LINE: usize = 6;
const SOURCE_NONE: u32 = 0;
const SOURCE_OLD: u32 = 1;
const SOURCE_NEW: u32 = 2;

/// `lines` as little-endian `u32` words, `WORDS_PER_LINE` per line, so Swift copies one buffer instead of lifting a record per line; `SideBySideLayoutLine.decode` in JayJayDiffUI reads it.
pub fn pack_sbs_layout(lines: &[SbsVisualLine]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lines.len() * WORDS_PER_LINE * size_of::<u32>());
    for line in lines {
        let source = match line.full_width_source {
            None => SOURCE_NONE,
            Some(DiffSide::Old) => SOURCE_OLD,
            Some(DiffSide::New) => SOURCE_NEW,
        };
        let words: [u32; WORDS_PER_LINE] = [
            line.row_ix,
            line.old.start,
            line.old.end,
            line.new.start,
            line.new.end,
            source,
        ];
        bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use jayjay_core::diff::Utf16Range;

    use super::*;

    #[test]
    fn packs_six_little_endian_words_per_line() {
        let lines = [
            SbsVisualLine {
                row_ix: 3,
                old: Utf16Range { start: 0, end: 40 },
                new: Utf16Range { start: 0, end: 0 },
                full_width_source: None,
            },
            SbsVisualLine {
                row_ix: 0x0102_0304,
                old: Utf16Range { start: 5, end: 6 },
                new: Utf16Range { start: 6, end: 300 },
                full_width_source: Some(DiffSide::New),
            },
            SbsVisualLine {
                row_ix: 9,
                old: Utf16Range { start: 1, end: 2 },
                new: Utf16Range { start: 2, end: 3 },
                full_width_source: Some(DiffSide::Old),
            },
        ];

        #[rustfmt::skip]
        let expected: Vec<u8> = vec![
            3, 0, 0, 0,  0, 0, 0, 0,  40, 0, 0, 0,  0, 0, 0, 0,  0, 0, 0, 0,  0, 0, 0, 0,
            4, 3, 2, 1,  5, 0, 0, 0,  6, 0, 0, 0,  6, 0, 0, 0,  44, 1, 0, 0,  2, 0, 0, 0,
            9, 0, 0, 0,  1, 0, 0, 0,  2, 0, 0, 0,  2, 0, 0, 0,  3, 0, 0, 0,  1, 0, 0, 0,
        ];
        assert_eq!(pack_sbs_layout(&lines), expected);
    }
}
