use gpui::{App, Context, Entity, IntoElement, Render, Window, prelude::*};
use gpui_component::{WindowExt as _, command::CommandState};

use crate::{Surface, terminal::SessionId};

use super::{SearchDialog, SearchDialogEvent, SearchItem};

/// One row of the sessions dialog: which tab owns it and which session it
/// focuses. Mirrors the ratatui `SessionsView` lines (`[tab]-id  title`).
#[derive(Clone)]
pub struct SessionEntry {
    tab: usize,
    session: SessionId,
    label: String,
}

pub(crate) fn session_entries(workspace: &crate::workspace::Workspace) -> Vec<SessionEntry> {
    let mut entries = Vec::new();
    for (tab_index, tab) in workspace.tabs.iter().enumerate() {
        let Some(tab) = tab.as_ref() else {
            continue;
        };
        for session in tab.sessions() {
            let title = workspace
                .session_manager
                .session(session)
                .map(|s| s.title())
                .unwrap_or("shell");
            entries.push(SessionEntry {
                tab: tab_index,
                session,
                label: format!("[{}]-{session}    {title}", tab_index + 1),
            });
        }
    }
    entries
}

/// Sessions screen: owns its dialog entity plus the entries behind the
/// rows. Constructed once, held by the `Overlay` orchestrator
/// (`ActiveScreen::Sessions`); `show` refreshes the snapshot and presents.
pub struct SessionsView {
    surface: Entity<Surface>,
    dialog: Entity<SearchDialog>,
    entries: Vec<SessionEntry>,
}

impl SessionsView {
    pub(crate) fn new(
        surface: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| CommandState::new(window, cx));
        let dialog = cx.new(|_| {
            SearchDialog::new(
                Some("Sessions".to_string()),
                "Search sessions..".to_string(),
                None,
                query,
            )
        });
        cx.subscribe_in(&dialog, window, Self::on_confirm).detach();
        Self {
            surface,
            dialog,
            entries: Vec::new(),
        }
    }

    /// Refresh the entry snapshot and present the reused dialog.
    pub(crate) fn show(view: Entity<Self>, window: &mut Window, cx: &mut App) {
        view.update(cx, |this, cx| {
            this.entries = session_entries(&this.surface.read(cx).workspace);
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
        window.close_dialog(cx);
        let Some(entry) = entry else {
            self.surface.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
                cx.notify();
            });
            return;
        };
        self.surface.update(cx, |surface, cx| {
            if entry.tab != surface.workspace.current_tab {
                surface.switch_tab(entry.tab);
            }
            surface.workspace.set_active_session(entry.session);
            window.focus(&surface.focus_handle, cx);
            cx.notify();
        });
    }
}

impl Render for SessionsView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.dialog.clone()
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn session_entry_label_format() {
        let label = format!("[{}]-{}    {}", 1, 42, "shell");
        assert_eq!(label, "[1]-42    shell");
    }
}
