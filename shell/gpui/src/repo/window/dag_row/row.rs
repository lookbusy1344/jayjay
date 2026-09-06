use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, ClickEvent, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Rgba, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px, rgb,
};
use jayjay_core::{BookmarkInfo, ChangeInfo, GraphEntry};

use super::chips::tags_row;
use super::text::{compact_id, first_line, format_relative, id_cell};
use crate::app::theme::{FONT_BODY, FONT_ID, FONT_META, FONT_TAG, Theme, ui_font_size};
use crate::repo::window::dag::ELISION_BAND_HEIGHT;
use crate::repo::window::dag_drag::{DagDrag, DagDragGhost, DagDragSelection};
use crate::ui::primitives::text_tooltip;

const DAG_ROW_GAP: f32 = 3.;
const TEXT_LINE_HEIGHT: f32 = 1.618;

/// The row's resting background, matching the graph column's overflow-fade target so the fade
/// dissolves clipped lanes into the same colour the row paints behind them. Opaque: a translucent
/// selection is composited over the sidebar so the fade hides the lanes it covers.
pub(crate) fn row_background(
    t: &Theme,
    is_selected: bool,
    is_compare_source: bool,
    is_pane_active: bool,
) -> Rgba {
    if is_selected {
        rgb(t.sidebar_bg).blend(t.selection_bg(is_pane_active))
    } else if is_compare_source {
        rgb(t.tag_divergent_bg)
    } else {
        rgb(t.sidebar_bg)
    }
}

pub(crate) type ChipRightClick =
    Arc<dyn Fn(&str, &MouseDownEvent, &mut Window, &mut App) + Send + Sync + 'static>;

/// Invoked when a dragged DAG reference or change is dropped onto this row.
pub(crate) type DagDrop = Arc<dyn Fn(&DagDrag, &mut Window, &mut App) + 'static>;

/// Pure-data inputs for one DAG row in the sidebar.
pub(crate) struct DagRow<'a> {
    pub change: &'a ChangeInfo,
    pub is_selected: bool,
    pub is_compare_source: bool,
    pub is_pane_active: bool,
    pub ix: usize,
    pub theme: &'a Theme,
    pub dag_col: Option<AnyElement>,
    pub refs_budget: f32,
    pub bookmarks: &'a [BookmarkInfo],
    pub entries: &'a Arc<Vec<GraphEntry>>,
    pub drag_selection: Option<Arc<DagDragSelection>>,
    /// Synthetic `(elided revisions)` bands this row owns, and the widest band stack anywhere in
    /// the layout — every row reserves the latter so `uniform_list` can keep one height.
    pub elision_bands: usize,
    pub widest_elision_bands: usize,
}

pub(crate) fn dag_row<F, FR>(
    row: DagRow<'_>,
    on_click: F,
    on_right_click: FR,
    on_bookmark_right_click: ChipRightClick,
    on_workspace_right_click: ChipRightClick,
    on_drop: DagDrop,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    FR: Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
{
    let DagRow {
        change,
        is_selected,
        is_compare_source,
        is_pane_active,
        ix,
        theme: t,
        dag_col,
        refs_budget,
        bookmarks,
        entries,
        drag_selection,
        elision_bands,
        widest_elision_bands,
    } = row;
    let summary = first_line(&change.description);

    let row_bg = row_background(t, is_selected, is_compare_source, is_pane_active);
    let hover_bg = if is_selected || is_compare_source {
        row_bg
    } else {
        rgb(t.row_alt_bg)
    };

    let row_selector = format!("dag-change-{}", change.commit_id.id);
    let drop_ring = t.toggle_active_bg;
    let refused_drop = t.tag_conflict_fg;
    let drop_target = change.clone();
    let mut row_div = div()
        .id(("change", ix))
        .debug_selector(move || row_selector.clone())
        .flex()
        .flex_row()
        .w_full()
        .h(px(dag_row_height(t, widest_elision_bands)))
        .bg(row_bg)
        .hover(|s| s.bg(hover_bg))
        .cursor_pointer()
        .on_click(on_click)
        .on_mouse_down(MouseButton::Right, on_right_click)
        .drag_over::<DagDrag>(move |style, drag, _, _| {
            if !drag.can_drop_on(&drop_target) {
                return style;
            }
            let color = if matches!(drag, DagDrag::WorkingCopy) && drop_target.is_immutable {
                refused_drop
            } else {
                drop_ring
            };
            style.bg(gpui::rgba(((color as u64) << 8) as u32 | 0x44))
        })
        .on_drop(move |drag: &DagDrag, w, cx| {
            on_drop(drag, w, cx);
        });
    if let Some(drag) = DagDrag::for_change(ix, entries, drag_selection) {
        row_div = row_div.on_drag(drag, move |drag: &DagDrag, _offset, _window, cx| {
            cx.new(|_| DagDragGhost::new(drag.clone()))
        });
    }
    if let Some(col) = dag_col {
        row_div = row_div.child(col);
    }
    let mut text_column = div().flex().flex_col().flex_1().min_w_0().child(
        div()
            .flex()
            .flex_col()
            .gap(px(DAG_ROW_GAP))
            .pr_3()
            .py_2()
            .flex_1()
            .min_w_0()
            .child(tags_row(
                change,
                ix,
                t,
                bookmarks,
                refs_budget,
                on_bookmark_right_click,
                on_workspace_right_click,
            ))
            .child(summary_line(change, &summary, ix, t))
            .child(meta_row(change, t)),
    );
    if elision_bands > 0 {
        text_column = text_column.child(elision_labels(elision_bands, t));
    }
    row_div.child(text_column).into_any_element()
}

/// One `(elided revisions)` label per synthetic band, aligned with the bands the graph column
/// paints in the same bottom strip of the row. Never interactive — the whole row stays the real
/// change's hit target.
fn elision_labels(band_count: usize, t: &Theme) -> impl IntoElement {
    let mut labels = div().flex().flex_col().pr_3().min_w_0();
    for _ in 0..band_count {
        labels = labels.child(
            div()
                .h(px(ELISION_BAND_HEIGHT))
                .flex()
                .items_center()
                .text_size(ui_font_size(FONT_META))
                .text_color(rgb(t.fg_faint))
                .truncate()
                .child("(elided revisions)"),
        );
    }
    labels
}

fn summary_line(change: &ChangeInfo, summary: &str, row_ix: usize, t: &Theme) -> impl IntoElement {
    if summary.is_empty() {
        div()
            .text_size(ui_font_size(FONT_BODY))
            .text_color(rgb(t.fg_faint))
            .truncate()
            .child("(no description)")
            .into_any_element()
    } else {
        div()
            .id(("summary", row_ix))
            .text_size(ui_font_size(FONT_BODY))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(rgb(t.fg))
            .line_clamp(2)
            .tooltip(text_tooltip(change.description.clone()))
            .child(SharedString::from(summary.to_owned()))
            .into_any_element()
    }
}

fn meta_row(change: &ChangeInfo, t: &Theme) -> impl IntoElement {
    let commit_id = compact_id(&change.commit_id);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .text_size(ui_font_size(FONT_ID))
        .text_color(rgb(t.fg_dim))
        .child(id_cell(
            &commit_id,
            change.commit_id.short_len,
            t.commit_id_prefix,
            FONT_ID,
            t,
        ))
        .child(crate::ui::avatar::with_name(
            &change.author.email,
            &change.author.name,
            FONT_ID,
            t.fg_dim,
        ))
        .child(
            div()
                .text_color(rgb(t.fg_faint))
                .child(SharedString::from(format_relative(
                    change.author.timestamp_millis,
                ))),
        )
}

pub(crate) fn text_line_height(t: &Theme, base: f32) -> f32 {
    t.scaled_font_size(base) * TEXT_LINE_HEIGHT
}

pub(crate) fn refs_row_height(t: &Theme) -> f32 {
    text_line_height(t, FONT_ID).max(text_line_height(t, FONT_TAG) + 2.)
}

/// Must match the `py_2` on the row's text column.
pub(crate) fn row_vertical_padding(t: &Theme) -> f32 {
    t.scaled_font_size(8.)
}

pub(crate) fn node_center_offset(t: &Theme) -> f32 {
    row_vertical_padding(t) + refs_row_height(t) / 2.
}

/// Rows share one height (the sidebar uses a uniform list), so every row reserves two summary lines
/// and the widest elision band stack in the layout rather than its own. A row with fewer bands
/// leaves the surplus above them, so its graph lanes still run to the row's bottom edge and meet
/// the next row.
pub(crate) fn dag_row_height(t: &Theme, widest_elision_bands: usize) -> f32 {
    2. * row_vertical_padding(t)
        + refs_row_height(t)
        + DAG_ROW_GAP
        + 2. * text_line_height(t, FONT_BODY)
        + DAG_ROW_GAP
        + text_line_height(t, FONT_ID)
        + ELISION_BAND_HEIGHT * widest_elision_bands as f32
}

#[cfg(test)]
mod tests {
    use super::{Theme, dag_row_height};
    use crate::repo::window::dag::main_row_bottom;

    /// Reserving the widest band stack for every row is what keeps a row's own content above its
    /// bands. Sizing each row to its own band count instead would push a multi-band row's content
    /// off the top of its fixed `uniform_list` slot.
    #[test]
    fn a_row_keeps_its_whole_content_above_its_own_elision_bands() {
        let theme = Theme::light();
        let widest = 5;
        let height = dag_row_height(&theme, widest);
        let bandless = dag_row_height(&theme, 0);

        for bands in 0..=widest {
            assert!(
                main_row_bottom(height, bands) >= bandless,
                "{bands} bands left less than a bandless row's content height"
            );
        }
    }
}
