use gpui::Context;
use jayjay_core::{DEFAULT_REVSET_DEPTH, build_default_revset};

use super::view_model::RepoViewModel;
use crate::app::config;
use crate::diff::{DetailMode, DiffViewMode};

impl RepoViewModel {
    pub(crate) fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.view_mode = match self.view_mode {
            DiffViewMode::Unified => DiffViewMode::SideBySide,
            DiffViewMode::SideBySide => DiffViewMode::Unified,
        };
        cx.notify();
    }

    pub(crate) fn sync_ignore_whitespace(&mut self, cx: &mut Context<Self>) {
        let ignore_whitespace = config::current(cx).diff.ignore_whitespace;
        if self.ignore_whitespace == ignore_whitespace {
            return;
        }
        self.ignore_whitespace = ignore_whitespace;
        let rev = self
            .compare
            .as_ref()
            .map(|compare| compare.to_rev.clone())
            .or_else(|| self.selected_revision());
        let hunk = self.selected_hunk().cloned();
        if let (Some(rev), Some(hunk)) = (rev, hunk) {
            self.load_diff_async(rev, hunk, cx);
        } else {
            cx.notify();
        }
    }

    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.can_load_more || self.focused_revision.is_some() {
            return;
        }
        let depth = self.revset_depth().unwrap_or(DEFAULT_REVSET_DEPTH);
        let new_depth = depth + DEFAULT_REVSET_DEPTH;
        let new_revset = build_default_revset(new_depth);
        self.loading.more = true;
        self.can_load_more = false;
        self.revset = new_revset.into();
        self.refresh(false, cx);
    }

    pub(crate) fn toggle_annotate(&mut self, cx: &mut Context<Self>) {
        self.detail_mode = match self.detail_mode {
            DetailMode::Annotate => DetailMode::Diff,
            DetailMode::Diff => DetailMode::Annotate,
        };
        if matches!(self.detail_mode, DetailMode::Annotate) {
            self.load_annotate(cx);
        }
        cx.notify();
    }
}
