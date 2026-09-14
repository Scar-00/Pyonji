use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render, Window, div, prelude::*, px};
use gpui_component::{ActiveTheme as _, WindowExt as _, v_flex};

use crate::Surface;

/// One row of the opener: a directory the user can descend into, or the
/// parent (`..`) to go up. The old explorer was filtered to directories only;
/// this list preserves that.
#[derive(Clone)]
struct DirEntry {
    /// Display name (`..` for the parent).
    name: String,
    /// Absolute path of the entry (parent dir for `..`).
    path: PathBuf,
}

/// Directory picker dialog state.
///
/// Keys mirror the old `OpenerState::handle_events`: Up/Down move, Enter
/// descends, Space opens a session in the current directory, `o` jumps to the
/// filesystem root.
pub struct OpenerView {
    surface: Entity<Surface>,
    focus: FocusHandle,
    cwd: PathBuf,
    entries: Vec<DirEntry>,
    selected: usize,
}

impl OpenerView {
    fn new(surface: Entity<Surface>, cx: &mut Context<Self>) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut this = Self {
            surface,
            focus: cx.focus_handle(),
            cwd,
            entries: Vec::new(),
            selected: 0,
        };
        this.refresh();
        this
    }

    fn refresh(&mut self) {
        let mut entries = Vec::new();
        if let Some(parent) = self.cwd.parent() {
            entries.push(DirEntry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
            });
        }
        if let Ok(read_dir) = std::fs::read_dir(&self.cwd) {
            let mut dirs = read_dir
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect::<Vec<_>>();
            dirs.sort();
            for path in dirs {
                entries.push(DirEntry {
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string()),
                    path,
                });
            }
        }
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        self.entries = entries;
    }

    fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn select_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    fn descend(&mut self) {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return;
        };
        self.cwd = entry.path;
        self.selected = 0;
        self.refresh();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        window.close_dialog(cx);
        self.surface.update(cx, |surface, cx| {
            surface.open_session_in_dir(&cwd);
            window.focus(&surface.focus_handle, cx);
            cx.notify();
        });
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        match key {
            "up" => {
                self.select_prev();
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }
            "down" => {
                self.select_next();
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }
            "enter" => {
                self.descend();
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }
            "space" => {
                window.prevent_default();
                cx.stop_propagation();
                self.confirm(window, cx);
            }
            "o" => {
                self.cwd = Path::new("/").to_path_buf();
                self.selected = 0;
                self.refresh();
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                window.close_dialog(cx);
            }
            _ => {}
        }
    }
}

impl Focusable for OpenerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for OpenerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        v_flex()
            .id("opener")
            .key_context("opener")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .p_1()
            .gap_px()
            .max_h(px(320.0))
            .overflow_hidden()
            .children(self.entries.iter().enumerate().map(|(index, entry)| {
                let is_selected = index == selected;
                div()
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .when(is_selected, |this| {
                        this.bg(cx.theme().accent)
                            .text_color(cx.theme().accent_foreground)
                    })
                    .child(entry.name.clone())
            }))
    }
}

/// Open the directory picker.
///
/// Space opens a session in the highlighted directory (like the old Space
/// confirm of `explorer.cwd()`); the dialog title always shows the cwd, as
/// the old block title did (`Opener - <cwd>`).
pub fn open_opener(surface: Entity<Surface>, window: &mut Window, cx: &mut App) {
    let focus_back = surface.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let view = cx.new(|cx| OpenerView::new(surface.clone(), cx));
        let focus = view.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| {
            focus.focus(window, cx);
        });
        let cwd = view.read(cx).cwd.clone();
        dialog
            .close_button(false)
            .p_0()
            .title(format!("Opener - {}", cwd.display()))
            .on_close(window.listener_for(&focus_back, |surface, _, window, cx| {
                window.focus(&surface.focus_handle, cx);
            }))
            .content(move |content, _, _| content.child(view.clone()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_entry_points_at_parent_dir() {
        let cwd = std::env::temp_dir();
        let parent = cwd.parent().map(Path::to_path_buf).unwrap();
        let entry = DirEntry {
            name: "..".to_string(),
            path: parent.clone(),
        };
        assert_eq!(entry.name, "..");
        assert_eq!(entry.path, parent);
    }

    #[test]
    fn dir_entry_fields() {
        let entry = DirEntry {
            name: "src".to_string(),
            path: PathBuf::from("/tmp/src"),
        };
        assert_eq!(entry.path, PathBuf::from("/tmp/src"));
    }
}
