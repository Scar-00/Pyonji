use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ScrollHandle, Window,
    actions, div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{
    Sizable,
    button::{Button, ButtonVariants},
};

use crate::{Next, Prev, PyTheme, Submit};

// Two extra actions this component needs on top of Next/Prev/Submit.
actions!(file_opener, [Parent, Cancel]);

const CONTEXT: &str = "FileOpener";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Prev, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("alt-up", Parent, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

pub enum FileOpenerEvent {
    /// A file or directory was chosen for a new session.
    Opened(PathBuf),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub _modified: Option<SystemTime>,
}

#[derive(Default)]
struct DirectoryListing {
    generation: u64,
    entries: Option<Vec<Entry>>,
    error: Option<String>,
}

impl DirectoryListing {
    fn begin_load(&mut self) -> u64 {
        self.generation += 1;
        self.entries = None;
        self.error = None;
        self.generation
    }

    fn finish_load(&mut self, generation: u64, result: io::Result<Vec<Entry>>) -> bool {
        if generation != self.generation {
            return false;
        }
        match result {
            Ok(entries) => self.entries = Some(entries),
            Err(error) => {
                self.entries = Some(Vec::new());
                self.error = Some(error.to_string());
            }
        }
        true
    }
}

pub struct FileOpener {
    focus_handle: FocusHandle,
    path_input: Entity<InputState>,
    scroll_handle: ScrollHandle,

    /// Directory whose entries are currently loaded.
    cwd: PathBuf,
    relative_base: PathBuf,
    listing: DirectoryListing,
    selected: Option<usize>,

    show_hidden: bool,
}

impl EventEmitter<FileOpenerEvent> for FileOpener {}

impl FileOpener {
    pub fn new(start_dir: impl Into<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let start_dir: PathBuf = start_dir.into();
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a path…"));

        cx.subscribe(&path_input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.on_input_changed(cx);
            }
        })
        .detach();

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            path_input,
            scroll_handle: ScrollHandle::new(),
            cwd: PathBuf::new(),
            relative_base: start_dir.clone(),
            listing: DirectoryListing::default(),
            selected: None,
            show_hidden: false,
        };
        this.set_input_path(&start_dir, window, cx);
        this
    }

    pub fn show_hidden(mut self, v: bool) -> Self {
        self.show_hidden = v;
        self
    }

    pub fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.selected = None;
        cx.notify();
    }

    // ------------------------------------------------------------- path logic

    /// Native separators are recognized without treating Unix backslashes as separators.
    fn parse_input(&self, cx: &App) -> (PathBuf, String) {
        split_path_input(
            self.path_input.read(cx).value().as_str(),
            &self.cwd,
            &self.relative_base,
            cfg!(windows),
            dirs::home_dir().as_deref(),
        )
    }

    /// Replace the input with `dir/` (which triggers a reload via Change).
    fn set_input_path(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let mut s = dir.to_string_lossy().to_string();
        if !s.ends_with(std::path::MAIN_SEPARATOR) && !s.ends_with('/') {
            s.push(std::path::MAIN_SEPARATOR);
        }
        self.relative_base = dir.to_path_buf();
        self.path_input
            .update(cx, |state, cx| state.set_value(s, window, cx));
        // set_value may or may not emit Change depending on version; be safe.
        self.on_input_changed(cx);
    }

    fn on_input_changed(&mut self, cx: &mut Context<Self>) {
        let (dir, _) = self.parse_input(cx);
        if dir != self.cwd {
            self.cwd = dir.clone();
            self.load(dir, cx);
        }
        self.selected = None;
        cx.notify();
    }

    fn load(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let generation = self.listing.begin_load();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { read_dir_sorted(&dir) })
                .await;

            this.update(cx, |this, cx| {
                if this.listing.finish_load(generation, result) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Indices into `entries` matching the current fragment / flags.
    fn visible(&self, cx: &App) -> Vec<usize> {
        let (_, frag) = self.parse_input(cx);
        let frag = frag.to_lowercase();
        let Some(entries) = &self.listing.entries else {
            return vec![];
        };

        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.show_hidden || !e.name.starts_with('.'))
            .filter(|(_, e)| frag.is_empty() || e.name.to_lowercase().contains(&frag))
            .map(|(ix, _)| ix)
            .collect()
    }

    fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix;
        if let Some(position) =
            ix.and_then(|ix| self.visible(cx).iter().position(|entry| *entry == ix))
        {
            self.scroll_handle.scroll_to_item(position);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self
            .listing
            .entries
            .as_ref()
            .and_then(|e| e.get(ix))
            .cloned()
        else {
            return;
        };
        if entry.is_dir {
            self.set_input_path(&entry.path, window, cx);
        } else {
            cx.emit(FileOpenerEvent::Opened(entry.path));
        }
    }

    // ---------------------------------------------------------------- actions

    fn on_next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        let vis = self.visible(cx);
        if vis.is_empty() {
            return;
        }
        let next = match self.selected.and_then(|s| vis.iter().position(|&i| i == s)) {
            Some(pos) => vis[(pos + 1) % vis.len()],
            None => vis[0],
        };
        self.select(Some(next), cx);
    }

    fn on_prev(&mut self, _: &Prev, _: &mut Window, cx: &mut Context<Self>) {
        let vis = self.visible(cx);
        if vis.is_empty() {
            return;
        }
        let prev = match self.selected.and_then(|s| vis.iter().position(|&i| i == s)) {
            Some(pos) => vis[(pos + vis.len() - 1) % vis.len()],
            None => vis[vis.len() - 1],
        };
        self.select(Some(prev), cx);
    }

    fn on_submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let vis = self.visible(cx);
        let (dir, frag) = self.parse_input(cx);

        // A selected directory is browsed; an unfiltered path opens that directory.
        if let Some(ix) = self.selected {
            self.activate(ix, window, cx);
        } else if frag.is_empty() {
            self.open_directory(cx);
        } else {
            let path = dir.join(&frag);
            if path.is_dir() {
                self.set_input_path(&path, window, cx);
            } else if path.is_file() {
                cx.emit(FileOpenerEvent::Opened(path));
            } else if vis.len() == 1 {
                self.activate(vis[0], window, cx);
            }
        }
    }

    fn open_directory(&self, cx: &mut Context<Self>) {
        if self.cwd.is_dir() {
            cx.emit(FileOpenerEvent::Opened(self.cwd.clone()));
        }
    }

    fn on_parent(&mut self, _: &Parent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
            self.set_input_path(&parent, window, cx);
        }
    }

    fn on_cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FileOpenerEvent::Cancelled);
    }

    // ----------------------------------------------------------------- render

    fn render_breadcrumb(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let mut acc = PathBuf::new();
        let parts: Vec<(String, PathBuf)> = self
            .cwd
            .components()
            .map(|c| {
                acc.push(c);
                let label = match c {
                    std::path::Component::RootDir => "/".to_string(),
                    other => other.as_os_str().to_string_lossy().to_string(),
                };
                (label, acc.clone())
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap_1()
            .text_xs()
            .text_color(t.text_muted)
            .children(
                parts
                    .into_iter()
                    .enumerate()
                    .flat_map(|(i, (label, path))| {
                        let is_root = label == "/";
                        let crumb = div()
                            .id(("crumb", i))
                            .role(gpui::accesskit::Role::Button)
                            .tab_index(0)
                            .focus_visible(|style| style.text_color(t.accent))
                            .aria_label(format!("Go to {}", path.display()))
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hovered).text_color(t.text))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_input_path(&path, window, cx);
                            }))
                            .child(label);
                        let sep =
                            (!is_root && i > 0).then(|| div().text_color(t.text_muted).child("/"));
                        sep.into_iter()
                            .map(IntoElement::into_any_element)
                            .chain(std::iter::once(crumb.into_any_element()))
                    }),
            )
    }

    fn render_input(&self, window: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let focused = self.path_input.read(cx).focus_handle(cx).is_focused(window);
        div()
            .w_full()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .bg(gpui::rgba(0))
            .border_color(if focused { t.focus_ring } else { t.border })
            .text_color(t.text)
            .text_sm()
            .child(
                Input::new(&self.path_input)
                    .appearance(false)
                    .bordered(false)
                    .role(gpui_base::RoleOverride::Presentational),
            )
    }

    fn render_entry(&self, ix: usize, e: &Entry, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let selected = self.selected == Some(ix);

        div()
            .id(("entry", ix))
            .aria_label(format!(
                "{} {}",
                if e.is_dir { "Directory" } else { "File" },
                e.name
            ))
            .role(gpui::accesskit::Role::ListBoxOption)
            .aria_selected(selected)
            .when(selected, |row| row.aria_active_descendant())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .border_l_2()
            .border_color(if selected {
                t.selected_border
            } else {
                gpui::rgba(0)
            })
            .bg(if selected { t.selected } else { gpui::rgba(0) })
            .when(!selected, |d| d.hover(|s| s.bg(t.hovered)))
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.activate(ix, window, cx);
                } else {
                    this.select(Some(ix), cx);
                }
            }))
            // icon
            .child(
                div()
                    .w_4()
                    .text_xs()
                    .text_color(if e.is_dir { t.accent } else { t.text_muted })
                    .child(if e.is_dir { "▸" } else { "·" }),
            )
            // name
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_sm()
                    .text_color(t.text)
                    .when(e.is_dir, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                    .child(if e.is_dir {
                        format!("{}/", e.name)
                    } else {
                        e.name.clone()
                    }),
            )
            // size
            .child(div().text_xs().text_color(t.text_muted).child(if e.is_dir {
                String::new()
            } else {
                human_size(e.size)
            }))
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();

        let Some(entries) = &self.listing.entries else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_muted)
                .child("Loading…")
                .into_any_element();
        };

        if let Some(err) = &self.listing.error {
            return div()
                .flex_1()
                .p_3()
                .text_sm()
                .text_color(t.error)
                .child(err.clone())
                .into_any_element();
        }

        let vis = self.visible(cx);
        if vis.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(t.text_muted)
                .child(if entries.is_empty() {
                    "Empty directory"
                } else {
                    "No matches"
                })
                .into_any_element();
        }

        div()
            .id("file-opener-list")
            .role(gpui::accesskit::Role::ListBox)
            .aria_label("Files and directories")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_px()
            .p_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .children(
                vis.into_iter()
                    .map(|ix| self.render_entry(ix, &entries[ix], cx)),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let count = self.visible(cx).len();
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .flex_shrink_0()
            .justify_between()
            .items_center()
            .px_3()
            .py_1p5()
            .border_t_1()
            .border_color(t.border)
            .text_xs()
            .text_color(t.text_muted)
            .child(format!("{count} item{}", if count == 1 { "" } else { "s" }))
            .child(
                Button::new("open-directory")
                    .ghost()
                    .small()
                    .label("Open directory")
                    .on_click(cx.listener(|this, _, _, cx| this.open_directory(cx))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .id("toggle-hidden")
                            .role(gpui::accesskit::Role::CheckBox)
                            .tab_index(0)
                            .focus_visible(|style| style.text_color(t.accent).underline())
                            .aria_label("Show hidden files")
                            .aria_toggled(if self.show_hidden {
                                gpui::accesskit::Toggled::True
                            } else {
                                gpui::accesskit::Toggled::False
                            })
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text))
                            .text_color(if self.show_hidden {
                                t.accent
                            } else {
                                t.text_muted
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_hidden(cx)))
                            .child("hidden"),
                    )
                    .child("↑↓ navigate")
                    .child("⏎ open")
                    .child("⌥↑ parent")
                    .child("esc cancel"),
            )
    }
}

impl Focusable for FileOpener {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Delegate focus to the input so typing works immediately.
        self.path_input.read(cx).focus_handle(cx)
    }
}

impl Render for FileOpener {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .id("file-opener")
            .key_context(CONTEXT)
            .tab_group()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_parent))
            .on_action(cx.listener(Self::on_cancel))
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgba(0))
            .rounded_lg()
            .overflow_hidden()
            .child(
                crate::ui::combo_box::editable_combo_box(
                    "file-search-results",
                    "Open a path",
                    "Type a path…",
                    &self.path_input,
                    true,
                    cx,
                )
                .size_full()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .gap_2()
                        .p_3()
                        .border_b_1()
                        .border_color(t.border)
                        .child(self.render_breadcrumb(cx))
                        .child(self.render_input(window, cx)),
                )
                .child(self.render_list(cx))
                .child(self.render_footer(cx)),
            )
    }
}

// ------------------------------------------------------------------ helpers

fn split_path_input(
    raw: &str,
    cwd: &Path,
    base: &Path,
    windows: bool,
    home: Option<&Path>,
) -> (PathBuf, String) {
    let separator = |c| c == '/' || (windows && c == '\\');
    let expand = |value: &str| {
        if let Some(rest) = value.strip_prefix('~')
            && (rest.is_empty() || rest.starts_with(separator))
            && let Some(home) = home
        {
            return home.join(rest.trim_start_matches(separator));
        }
        let path = PathBuf::from(value);
        let native_root =
            windows && (value.starts_with('\\') || value.as_bytes().get(1) == Some(&b':'));
        if path.is_absolute() || native_root {
            path
        } else {
            base.join(path)
        }
    };
    if raw == "~" {
        return (expand(raw), String::new());
    }
    match raw.rfind(separator) {
        Some(ix) => {
            let (directory, fragment) = raw.split_at(ix + 1);
            (expand(directory), fragment.to_string())
        }
        None => (cwd.to_path_buf(), raw.to_string()),
    }
}

fn read_dir_sorted(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out: Vec<Entry> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir()) || e.path().is_dir(); // follow symlinks to dirs
            Entry {
                name: e.file_name().to_string_lossy().to_string(),
                path: e.path(),
                is_dir,
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                _modified: meta.and_then(|m| m.modified().ok()),
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1}{}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_drive_unc_and_forward_slash_paths_are_split() {
        let cwd = Path::new("/current");
        let base = Path::new("/base");
        for (raw, directory, fragment) in [
            (r"C:\", r"C:\", ""),
            (
                "C:\\Users\\Public\\Documents\\",
                "C:\\Users\\Public\\Documents\\",
                "",
            ),
            (
                "C:\\Users\\Public\\Documents",
                "C:\\Users\\Public\\",
                "Documents",
            ),
            (
                "C:/Users/Public/Documents/",
                "C:/Users/Public/Documents/",
                "",
            ),
            (
                "\\\\server\\share\\folder\\",
                "\\\\server\\share\\folder\\",
                "",
            ),
            ("\\\\server\\share\\folder", "\\\\server\\share\\", "folder"),
        ] {
            assert_eq!(
                split_path_input(raw, cwd, base, true, None),
                (PathBuf::from(directory), fragment.to_string())
            );
        }
    }

    #[test]
    fn relative_paths_use_a_stable_base_and_tilde_only_expands_home() {
        let cwd = Path::new("/base/sub");
        let base = Path::new("/base");
        let home = Path::new("/home/test");
        assert_eq!(
            split_path_input("sub/file", cwd, base, false, Some(home)),
            (base.join("sub"), "file".into())
        );
        assert_eq!(
            split_path_input("sub/", cwd, base, false, Some(home)),
            (base.join("sub"), "".into())
        );
        assert_eq!(
            split_path_input("~/", cwd, base, false, Some(home)),
            (home.to_path_buf(), "".into())
        );
        assert_eq!(
            split_path_input("~", cwd, base, false, Some(home)),
            (home.to_path_buf(), "".into())
        );
        assert_eq!(
            split_path_input("~someone/file", cwd, base, false, Some(home)),
            (base.join("~someone"), "file".into())
        );
        assert_eq!(
            split_path_input(r"literal\name", cwd, base, false, Some(home)),
            (cwd.to_path_buf(), r"literal\name".into())
        );
    }

    #[test]
    fn late_directory_results_cannot_replace_the_current_listing() {
        let mut listing = DirectoryListing::default();
        let first = listing.begin_load();
        let second = listing.begin_load();
        let entry = Entry {
            name: "current.txt".into(),
            path: PathBuf::from("current/current.txt"),
            is_dir: false,
            size: 0,
            _modified: None,
        };
        assert!(listing.finish_load(second, Ok(vec![entry])));
        assert!(!listing.finish_load(first, Err(io::Error::other("old error"))));
        assert!(!listing.finish_load(first, Ok(Vec::new())));
        assert_eq!(listing.entries.as_ref().unwrap()[0].name, "current.txt");
        assert!(listing.error.is_none());
    }

    #[test]
    fn returning_to_a_directory_still_discards_its_previous_load() {
        let mut listing = DirectoryListing::default();
        let old = listing.begin_load();
        listing.begin_load();
        let current = listing.begin_load();
        assert!(!listing.finish_load(old, Ok(Vec::new())));
        assert!(listing.entries.is_none());
        assert!(listing.finish_load(current, Err(io::Error::other("current error"))));
        assert_eq!(listing.error.as_deref(), Some("current error"));
    }
}
