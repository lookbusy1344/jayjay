use std::mem::size_of;

use jj_diff::{
    ConflictLineKind, ContextRegion, DiffLine, DiffSpan, DiffSpanStyle, FileDiff, MovedRow,
    SyntaxToken,
};

/// A `FileDiff` in three buffers: row metadata, span metadata, and every span's text back to back.
///
/// A `FileDiff` holds one heap block per row and one per span; a cache of loaded diffs holds hundreds of thousands of them.
#[derive(Debug, Clone)]
pub(super) struct PackedDiff {
    path: String,
    language: String,
    whitespace_only_hidden: bool,
    rows: Vec<PackedRow>,
    spans: Vec<PackedSpan>,
    text: String,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PackedRow {
    pub(super) old_line_no: Option<u32>,
    pub(super) new_line_no: Option<u32>,
    pub(super) style: DiffSpanStyle,
    pub(super) conflict_kind: ConflictLineKind,
    pub(super) no_eof_newline: bool,
    pub(super) context_region: Option<ContextRegion>,
    pub(super) moved: Option<MovedRow>,
    /// End of this row's spans in `PackedDiff::spans`; the previous row's end is its start.
    spans_end: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PackedSpan {
    /// End of this span's text in `PackedDiff::text`; the previous span's end is its start.
    text_end: usize,
    style: DiffSpanStyle,
    token: SyntaxToken,
}

impl PackedDiff {
    pub(super) fn pack(diff: FileDiff) -> Self {
        let span_count = diff.lines.iter().map(|line| line.spans.len()).sum();
        let text_len = diff
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.text.len())
            .sum();
        let mut spans = Vec::with_capacity(span_count);
        let mut text = String::with_capacity(text_len);
        let rows = diff
            .lines
            .into_iter()
            .map(|line| {
                for span in line.spans {
                    text.push_str(&span.text);
                    spans.push(PackedSpan {
                        text_end: text.len(),
                        style: span.style,
                        token: span.token,
                    });
                }
                PackedRow {
                    old_line_no: line.old_line_no,
                    new_line_no: line.new_line_no,
                    style: line.style,
                    conflict_kind: line.conflict_kind,
                    no_eof_newline: line.no_eof_newline,
                    context_region: line.context_region,
                    moved: line.moved,
                    spans_end: spans.len(),
                }
            })
            .collect();
        Self {
            path: diff.path,
            language: diff.language,
            whitespace_only_hidden: diff.whitespace_only_hidden,
            rows,
            spans,
            text,
        }
    }

    pub(super) fn unpack(&self) -> FileDiff {
        let mut span_start = 0;
        let mut text_start = 0;
        let lines = self
            .rows
            .iter()
            .map(|row| {
                let spans = self.spans[span_start..row.spans_end]
                    .iter()
                    .map(|span| {
                        let text = self.text[text_start..span.text_end].to_owned();
                        text_start = span.text_end;
                        DiffSpan {
                            text,
                            style: span.style,
                            token: span.token,
                            tooltip: None,
                        }
                    })
                    .collect();
                span_start = row.spans_end;
                DiffLine {
                    old_line_no: row.old_line_no,
                    new_line_no: row.new_line_no,
                    style: row.style,
                    spans,
                    conflict_kind: row.conflict_kind,
                    no_eof_newline: row.no_eof_newline,
                    context_region: row.context_region,
                    moved: row.moved,
                }
            })
            .collect();
        FileDiff {
            path: self.path.clone(),
            language: self.language.clone(),
            lines,
            whitespace_only_hidden: self.whitespace_only_hidden,
        }
    }

    pub(super) fn rows(&self) -> &[PackedRow] {
        &self.rows
    }

    pub(super) fn whitespace_only_hidden(&self) -> bool {
        self.whitespace_only_hidden
    }

    /// Span text plus row and span metadata.
    pub(super) fn retained_bytes(&self) -> usize {
        self.text.len()
            + self.rows.len() * size_of::<PackedRow>()
            + self.spans.len() * size_of::<PackedSpan>()
    }
}
