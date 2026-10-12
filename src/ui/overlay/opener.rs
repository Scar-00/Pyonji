use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ScrollHandle,
    SharedString, Window, actions, div, prelude::*, px,
};
use gpui_base::{h_flex, v_flex};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonRounded, ButtonVariants},
};

use crate::{
    Next, Prev, PyTheme, Submit,
    ui::{hint, mono},
};

// Two extra actions this component needs on top of Next/Prev/Submit.
actions!(file_opener, [Parent, Cancel]);

const CONTEXT: &str = "FileOpener";
const ROW_HEIGHT: f32 = 34.0;

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
            Some(pos) => vis[(pos + 1).min(vis.len() - 1)],
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
            Some(pos) => vis[pos.saturating_sub(1)],
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

    /// The directory as a row of places to jump back to. A drive or share
    /// prefix and the root after it are one place, `C:\`, not two.
    fn crumbs(&self) -> Vec<(String, PathBuf)> {
        use std::path::Component;
        let mut acc = PathBuf::new();
        let mut parts: Vec<(String, PathBuf)> = Vec::new();
        for component in self.cwd.components() {
            acc.push(component);
            match component {
                Component::RootDir if parts.len() == 1 => {
                    let (label, path) = &mut parts[0];
                    label.push(std::path::MAIN_SEPARATOR);
                    *path = acc.clone();
                }
                Component::RootDir => {
                    parts.push((std::path::MAIN_SEPARATOR.to_string(), acc.clone()))
                }
                other => parts.push((other.as_os_str().to_string_lossy().to_string(), acc.clone())),
            }
        }
        parts
    }

    fn render_breadcrumb(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let parts = self.crumbs();
        let last = parts.len().saturating_sub(1);

        h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .flex_wrap()
            .gap_0p5()
            .font_family(mono(cx))
            .text_size(px(12.))
            .text_color(t.text_muted)
            .children(parts.into_iter().enumerate().flat_map(|(i, (label, path))| {
                // The root crumb ends in its own separator; every later crumb
                // after the second needs one drawn in front of it.
                let sep = (i > 1).then(|| {
                    div()
                        .text_color(t.text_disabled)
                        .child(std::path::MAIN_SEPARATOR.to_string())
                });
                let crumb = div()
                    .id(("crumb", i))
                    .role(gpui::accesskit::Role::Button)
                    .tab_index(0)
                    .focus_visible(|style| style.text_color(t.accent))
                    .aria_label(format!("Go to {}", path.display()))
                    .px_1()
                    .cursor_pointer()
                    .when(i == last, |crumb| crumb.text_color(t.text))
                    .hover(|s| s.bg(t.hovered).text_color(t.text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_input_path(&path, window, cx);
                    }))
                    .child(label);
                sep.into_iter()
                    .map(IntoElement::into_any_element)
                    .chain(std::iter::once(crumb.into_any_element()))
            }))
    }

    fn render_hidden_toggle(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        h_flex()
            .id("toggle-hidden")
            .role(gpui::accesskit::Role::CheckBox)
            .tab_index(0)
            .focus_visible(|style| style.text_color(t.accent))
            .aria_label("Show hidden files")
            .aria_toggled(if self.show_hidden {
                gpui::accesskit::Toggled::True
            } else {
                gpui::accesskit::Toggled::False
            })
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .text_size(px(11.))
            .cursor_pointer()
            .text_color(if self.show_hidden {
                t.text
            } else {
                t.text_muted
            })
            .hover(|s| s.text_color(t.text))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_hidden(cx)))
            // A square that fills, the same mark the release rail uses for the
            // running build: on or off, nothing in between.
            .child(
                div()
                    .size(px(7.))
                    .border_1()
                    .border_color(if self.show_hidden {
                        t.accent
                    } else {
                        t.text_muted
                    })
                    .when(self.show_hidden, |mark| mark.bg(t.accent)),
            )
            .child("Hidden files")
    }

    /// The path line, ruled like the session search: the rule lights while
    /// the line has focus.
    fn render_input(&self, window: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let focused = self.path_input.read(cx).focus_handle(cx).is_focused(window);
        h_flex()
            .w_full()
            .h(px(36.))
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(if focused { t.focus_ring } else { t.border })
            .font_family(mono(cx))
            .text_size(px(14.))
            .text_color(t.text)
            .child(
                Icon::new(IconName::FolderOpen)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(t.text_muted),
            )
            .child(
                Input::new(&self.path_input)
                    .flex_1()
                    .appearance(false)
                    .bordered(false)
                    .role(gpui_base::RoleOverride::Presentational),
            )
    }

    fn render_entry(&self, ix: usize, e: &Entry, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let selected = self.selected == Some(ix);

        h_flex()
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
            .h(px(ROW_HEIGHT))
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_4()
            .cursor_pointer()
            // A rule and a raised fill, so selection cannot be mistaken for
            // hover; the same mark as every other list.
            .border_l_2()
            .border_color(if selected {
                t.selected_border
            } else {
                gpui::rgba(0)
            })
            .when(selected, |d| d.bg(t.surface_elevated))
            .when(!selected, |d| d.hover(|s| s.bg(t.hovered)))
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.activate(ix, window, cx);
                } else {
                    this.select(Some(ix), cx);
                }
            }))
            .child(
                Icon::new(if e.is_dir {
                    IconName::Folder
                } else {
                    IconName::File
                })
                .size_4()
                .flex_shrink_0()
                .text_color(if e.is_dir && selected {
                    t.accent
                } else {
                    t.text_muted
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono(cx))
                    .text_size(px(13.))
                    .text_color(if selected || e.is_dir {
                        t.text
                    } else {
                        t.text_muted
                    })
                    .child(if e.is_dir {
                        format!("{}/", e.name)
                    } else {
                        e.name.clone()
                    }),
            )
            .when(!e.is_dir, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .font_family(mono(cx))
                        .text_size(px(11.))
                        .text_color(t.text_muted)
                        .child(human_size(e.size)),
                )
            })
    }

    /// A message where the list would be: what happened, then what to do.
    fn render_message(
        &self,
        heading: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        heading_color: gpui::Rgba,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let t = cx.theme();
        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(heading_color)
                    .child(heading.into()),
            )
            .child(
                div()
                    .max_w(px(440.))
                    .text_size(px(12.))
                    .text_color(t.text_muted)
                    .child(detail.into()),
            )
            .into_any_element()
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
                .child("Reading directory…")
                .into_any_element();
        };

        if let Some(err) = &self.listing.error {
            return self.render_message(
                "Can't read this directory.",
                format!("{err}. Choose another place from the path above, or press alt ↑."),
                t.error,
                cx,
            );
        }

        let vis = self.visible(cx);
        if vis.is_empty() {
            let (_, fragment) = self.parse_input(cx);
            return if entries.is_empty() || fragment.is_empty() {
                self.render_message(
                    "This directory is empty.",
                    "Press ⏎ to open a session here.",
                    t.text,
                    cx,
                )
            } else {
                self.render_message(
                    format!("Nothing here matches \u{201c}{fragment}\u{201d}."),
                    "Keep typing a path, or press ⏎ to open it if it exists.",
                    t.text,
                    cx,
                )
            };
        }

        div()
            .id("file-opener-list")
            .role(gpui::accesskit::Role::ListBox)
            .aria_label("Files and directories")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .py_1()
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
        // Enter browses into a selected directory; with nothing selected it
        // opens the directory on the line. Say which, before it happens.
        let enter = match self
            .selected
            .and_then(|ix| self.listing.entries.as_ref()?.get(ix))
        {
            Some(entry) if entry.is_dir => "browse",
            Some(_) => "open file",
            None => "open here",
        };
        h_flex()
            .min_h(px(40.))
            .py_2()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_4()
            .px_4()
            .border_t_1()
            .border_color(t.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(t.text_muted)
                    .child(format!("{count} item{}", if count == 1 { "" } else { "s" })),
            )
            .child(hint("↑↓", "choose", cx))
            .child(hint("⏎", enter, cx))
            .child(hint("alt ↑", "parent", cx))
            .child(hint("esc", "close", cx))
            .child(
                Button::new("open-directory")
                    .primary()
                    .small()
                    .rounded(ButtonRounded::None)
                    .label("Open here")
                    .tooltip("Start a session in this directory")
                    .on_click(cx.listener(|this, _, _, cx| this.open_directory(cx))),
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
                    v_flex()
                        .flex_shrink_0()
                        .gap_2()
                        .px_4()
                        .pt_3()
                        .pb_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_3()
                                .child(self.render_breadcrumb(cx))
                                .child(self.render_hidden_toggle(cx)),
                        )
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
