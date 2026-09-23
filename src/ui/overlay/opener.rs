use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui::{
    actions, div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle,
    Focusable, KeyBinding, ScrollHandle, Window,
};
use gpui_base::input::{Input, InputEvent, InputState};

use crate::{
    Next, Prev, Submit, PyTheme
};

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
    /// A file (or, when `directories_only`, a directory) was chosen.
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

pub struct FileOpener {
    focus_handle: FocusHandle,
    path_input: Entity<InputState>,
    scroll_handle: ScrollHandle,

    /// Directory whose entries are currently loaded.
    cwd: PathBuf,
    entries: Option<Vec<Entry>>,
    error: Option<String>,
    selected: Option<usize>,

    show_hidden: bool,
    directories_only: bool,
}

impl EventEmitter<FileOpenerEvent> for FileOpener {}

impl FileOpener {
    pub fn new(
        start_dir: impl Into<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let start_dir: PathBuf = start_dir.into();
        let path_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Type a path…")
        });

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
            entries: None,
            error: None,
            selected: None,
            show_hidden: false,
            directories_only: false,
        };
        this.set_input_path(&start_dir, window, cx);
        this
    }

    /*pub fn directories_only(mut self, v: bool) -> Self {
        self.directories_only = v;
        self
    }*/

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

    fn expand_tilde(p: &str) -> PathBuf {
        if let Some(rest) = p.strip_prefix('~')
        && let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest.trim_start_matches('/'));
        }
        PathBuf::from(p)
    }

    /// Split the input into (directory, filter fragment).
    fn parse_input(&self, cx: &App) -> (PathBuf, String) {
        let raw = self.path_input.read(cx).value().to_string();
        match raw.rfind('/') {
            Some(ix) => {
                let (dir, frag) = raw.split_at(ix + 1);
                let dir = if dir == "/" {
                    PathBuf::from("/")
                } else {
                    Self::expand_tilde(dir)
                };
                (dir, frag.to_string())
            }
            None => (self.cwd.clone(), raw),
        }
    }

    /// Replace the input with `dir/` (which triggers a reload via Change).
    fn set_input_path(&mut self, dir: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let mut s = dir.to_string_lossy().to_string();
        if !s.ends_with('/') {
            s.push('/');
        }
        self.path_input.update(cx, |state, cx| state.set_value(s, window, cx));
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
        self.entries = None;
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { read_dir_sorted(&dir) })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(entries) => this.entries = Some(entries),
                    Err(e) => {
                        this.entries = Some(Vec::new());
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Indices into `entries` matching the current fragment / flags.
    fn visible(&self, cx: &App) -> Vec<usize> {
        let (_, frag) = self.parse_input(cx);
        let frag = frag.to_lowercase();
        let Some(entries) = &self.entries else { return vec![] };

        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.show_hidden || !e.name.starts_with('.'))
            .filter(|(_, e)| !self.directories_only || e.is_dir)
            .filter(|(_, e)| {
                frag.is_empty() || e.name.to_lowercase().contains(&frag)
            })
            .map(|(ix, _)| ix)
            .collect()
    }

    fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix;
        if let Some(ix) = ix {
            self.scroll_handle.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.as_ref().and_then(|e| e.get(ix)).cloned()
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
        if vis.is_empty() { return; }
        let next = match self.selected.and_then(|s| vis.iter().position(|&i| i == s)) {
            Some(pos) => vis[(pos + 1) % vis.len()],
            None => vis[0],
        };
        self.select(Some(next), cx);
    }

    fn on_prev(&mut self, _: &Prev, _: &mut Window, cx: &mut Context<Self>) {
        let vis = self.visible(cx);
        if vis.is_empty() { return; }
        let prev = match self.selected.and_then(|s| vis.iter().position(|&i| i == s)) {
            Some(pos) => vis[(pos + vis.len() - 1) % vis.len()],
            None => vis[vis.len() - 1],
        };
        self.select(Some(prev), cx);
    }

    fn on_submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let vis = self.visible(cx);
        let (dir, frag) = self.parse_input(cx);

        // Prefer selection, then the single match, then literal typed path.
        if let Some(ix) = self.selected {
            self.activate(ix, window, cx);
        } else if vis.len() == 1 {
            self.activate(vis[0], window, cx);
        } else if frag.is_empty() && self.directories_only {
            cx.emit(FileOpenerEvent::Opened(dir));
        } else if !frag.is_empty() {
            let path = dir.join(&frag);
            if path.is_dir() {
                self.set_input_path(&path, window, cx);
            } else if path.is_file() || !self.directories_only {
                cx.emit(FileOpenerEvent::Opened(path));
            }
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
            .children(parts.into_iter().enumerate().flat_map(|(i, (label, path))| {
                let is_root = label == "/";
                let crumb = div()
                    .id(("crumb", i))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hovered).text_color(t.text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_input_path(&path, window, cx);
                    }))
                    .child(label);
                let sep = (!is_root && i > 0)
                    .then(|| div().text_color(t.text_disabled).child("/"));
                sep.into_iter().map(IntoElement::into_any_element)
                    .chain(std::iter::once(crumb.into_any_element()))
            }))
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
            .bg(t.surface)
            .border_color(if focused { t.focus_ring } else { t.border })
            .text_color(t.text)
            .text_sm()
            .child(Input::new(&self.path_input))
    }

    fn render_entry(&self, ix: usize, e: &Entry, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let selected = self.selected == Some(ix);

        div()
            .id(("entry", ix))
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
            .border_color(if selected { t.selected_border } else { t.unselected_border })
            .bg(if selected { t.selected } else { t.unselected })
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
                    .child(if e.is_dir { format!("{}/", e.name) } else { e.name.clone() }),
            )
            // size
            .child(
                div()
                    .text_xs()
                    .text_color(t.text_disabled)
                    .child(if e.is_dir { String::new() } else { human_size(e.size) }),
            )
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();

        let Some(entries) = &self.entries else {
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

        if let Some(err) = &self.error {
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
                .child(if entries.is_empty() { "Empty directory" } else { "No matches" })
                .into_any_element();
        }

        div()
            .id("file-opener-list")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_px()
            .p_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .children(vis.into_iter().map(|ix| self.render_entry(ix, &entries[ix], cx)))
            .into_any_element()
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let count = self.visible(cx).len();
        div()
            .flex()
            .flex_row()
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
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(
                        div()
                            .id("toggle-hidden")
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text))
                            .text_color(if self.show_hidden { t.accent } else { t.text_muted })
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
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_parent))
            .on_action(cx.listener(Self::on_cancel))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.surface_elevated)
            .border_1()
            .border_color(t.border)
            .rounded_lg()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(self.render_breadcrumb(cx))
                    .child(self.render_input(window, cx)),
            )
            .child(self.render_list(cx))
            .child(self.render_footer(cx))
    }
}

// ------------------------------------------------------------------ helpers

fn read_dir_sorted(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out: Vec<Entry> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let meta = e.metadata().ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir())
                || e.path().is_dir(); // follow symlinks to dirs
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
    if i == 0 { format!("{bytes} B") } else { format!("{v:.1}{}", UNITS[i]) }
}
