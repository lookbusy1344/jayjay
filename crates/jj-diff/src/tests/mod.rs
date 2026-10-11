use super::*;

mod basic;
mod collapse;
mod deep_recursion;
mod eof;
mod expand;
mod expand_highlights;
mod expand_session;
mod fixtures;
mod performance;
mod review_fingerprint;
mod stats;
mod syntax;
mod word;

fn span_info(line: &DiffLine) -> Vec<(&str, DiffSpanStyle)> {
    line.spans
        .iter()
        .map(|s| (s.text.as_str(), s.style))
        .collect()
}

fn styled_text(spans: &[(&str, DiffSpanStyle)], style: DiffSpanStyle) -> String {
    spans
        .iter()
        .filter_map(|(text, span_style)| (*span_style == style).then_some(*text))
        .collect()
}
