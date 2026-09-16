use std::path::{Path, PathBuf};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render, Window,
    prelude::*,
};
use gpui_component::WindowExt as _;

use crate::{
    Surface,
    theme,
    ui::list_row::list_row,
};

use super::host::Overlay;

/// One row of the opener: a directory the user can descend into, or the
/// parent (`..`) to go up.
#[derive(Clone)]
struct DirEntry {
    name: String,
    path: PathBuf,
}

/// Directory picker dialog content component.
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
        gpui_component::v_flex()
            .id("opener")
            .key_context("opener")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .p(theme::space::_1)
            .gap(theme::space::PX)
            .max_h(theme::size::OVERLAY_LIST_MAX_H)
            .overflow_hidden()
            .children(
                self.entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| list_row(entry.name.clone(), index == selected)),
            )
    }
}

/// Open the directory picker.
pub fn open(surface: Entity<Surface>, window: &mut Window, cx: &mut App) {
    Overlay::present(surface.clone(), window, cx, move |dialog, window, cx| {
        let view = cx.new(|cx| OpenerView::new(surface.clone(), cx));
        let focus = view.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| {
            focus.focus(window, cx);
        });
        let cwd = view.read(cx).cwd.clone();
        dialog
            .title(format!("Opener - {}", cwd.display()))
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
