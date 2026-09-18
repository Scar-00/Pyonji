use gpui::{App, Context, Entity, IntoElement, Render, Window, prelude::*};
use gpui_component::{WindowExt as _, command::CommandState};

use crate::{Surface, terminal::SessionId};

use super::{SearchDialog, SearchDialogEvent, SearchItem};

/// One row of the attach dialog: `id  title`, like the old `DetachedView`.
#[derive(Clone)]
pub struct DetachedEntry {
    session: SessionId,
    label: String,
}

pub(crate) fn detached_entries(workspace: &crate::workspace::Workspace) -> Vec<DetachedEntry> {
    workspace
        .live_detached_sessions()
        .into_iter()
        .filter_map(|id| {
            let session = workspace.session_manager.session(id)?;
            Some(DetachedEntry {
                session: id,
                label: format!("{id}    {}", session.title()),
            })
        })
        .collect()
}

/// Detached-sessions screen: owns its dialog entity plus the entries behind
/// the rows. Constructed once, held by the `Overlay` orchestrator
/// (`ActiveScreen::Detached`); `show` refreshes the snapshot and presents.
///
/// Enter attaches the highlighted session to the current tab.
pub struct DetachedView {
    surface: Entity<Surface>,
    dialog: Entity<SearchDialog>,
    entries: Vec<DetachedEntry>,
    current_tab: usize,
}

impl DetachedView {
    pub(crate) fn new(
        surface: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| CommandState::new(window, cx));
        let dialog = cx.new(|_| {
            SearchDialog::new(
                Some("Attach Session".to_string()),
                "Search detached sessions..".to_string(),
                Some("Enter attaches to the current tab".to_string()),
                query,
            )
        });
        cx.subscribe_in(&dialog, window, Self::on_confirm).detach();
        Self {
            surface,
            dialog,
            entries: Vec::new(),
            current_tab: 0,
        }
    }

    /// Refresh the entry snapshot and present the reused dialog.
    /// `current_tab` is per-open context: attach lands there.
    pub(crate) fn show(
        view: Entity<Self>,
        current_tab: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        view.update(cx, |this, cx| {
            this.entries = detached_entries(&this.surface.read(cx).workspace);
            this.current_tab = current_tab;
            let items: Vec<SearchItem> = this
                .entries
                .iter()
                .map(|entry| SearchItem::new(entry.label.clone()))
                .collect();
            SearchDialog::show(this.dialog.clone(), items, this.surface.clone(), window, cx);
        });
    }

    fn on_confirm(
        &mut self,
        _: &Entity<SearchDialog>,
        event: &SearchDialogEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SearchDialogEvent::Confirm { row, .. } = event;
        let entry = self.entries.get(*row).cloned();
        let current_tab = self.current_tab;
        window.close_dialog(cx);
        let Some(entry) = entry else {
            self.surface.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
                cx.notify();
            });
            return;
        };
        self.surface.update(cx, |surface, cx| {
            surface.reattach_session(entry.session, current_tab);
            window.focus(&surface.focus_handle, cx);
            cx.notify();
        });
    }
}

impl Render for DetachedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.dialog.clone()
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn detached_label_format() {
        let label = format!("{}    {}", 7, "nvim");
        assert_eq!(label, "7    nvim");
    }
}
