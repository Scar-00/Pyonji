use std::{
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, ScrollHandle, Window,
    actions, div, prelude::*, px,
};
use gpui_base::h_flex;
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonRounded, ButtonVariants},
};

use crate::{
    Next, Prev, Submit,
    ui::{
        PyTheme as _,
        components::{
            EmptyState, PyInteractive as _, PyStyled as _, SearchField, counter,
            editable_combo_box, footer_status, hint, panel_footer, panel_header, rule,
        },
    },
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

    fn selected_entry(&self) -> Option<&Entry> {
        self.listing.entries.as_ref()?.get(self.selected?)
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
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "space" | "enter") {
                    cx.stop_propagation();
                    this.toggle_hidden(cx);
                }
            }))
            // A box big enough to read as one, ticked when on.
            .child(
                h_flex()
                    .size(px(13.))
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if self.show_hidden {
                        t.accent
                    } else {
                        t.text_muted
                    })
                    .when(self.show_hidden, |mark| {
                        mark.bg(t.accent).child(
                            Icon::new(IconName::Check)
                                .size(px(10.))
                                .text_color(t.background),
                        )
                    }),
            )
            .child("Hidden files")
    }

    /// The path line, ruled like the session search.
    fn render_input(&self, cx: &App) -> impl IntoElement {
        SearchField::new(&self.path_input)
            .icon(IconName::FolderOpen)
            .mono(cx)
            .text_size(px(14.))
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
            .list_row(selected, cx)
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                let is_dir = this
                    .listing
                    .entries
                    .as_ref()
                    .and_then(|entries| entries.get(ix))
                    .is_some_and(|entry| entry.is_dir);
                if is_dir || ev.click_count() >= 2 {
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
                    .mono(cx)
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
            // size, or for a directory a chevron that browses into it with a
            // single click, matching the directory row.
            .map(|row| {
                if e.is_dir {
                    row.child(
                        div()
                            .id(("browse", ix))
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .text_base()
                            .text_color(if selected { t.text } else { t.text_muted })
                            .hover(|s| s.bg(t.hovered).text_color(t.text))
                            .cursor_pointer()
                            .tooltip(|window, cx| {
                                gpui_component::tooltip::Tooltip::new("Browse directory")
                                    .build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.activate(ix, window, cx);
                            }))
                            .child("›"),
                    )
                } else {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .mono(cx)
                            .text_size(px(11.))
                            .text_color(t.text_muted)
                            .child(human_size(e.size)),
                    )
                }
            })
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let t = cx.theme();

        let Some(entries) = &self.listing.entries else {
            return EmptyState::new("Reading directory…")
                .muted(cx)
                .into_any_element();
        };

        if let Some(err) = &self.listing.error {
            return EmptyState::new("Can't read this directory.")
                .detail(format!(
                    "{err}. Choose another place from the path above, or press alt ↑."
                ))
                .tone(t.error)
                .into_any_element();
        }

        let vis = self.visible(cx);
        if vis.is_empty() {
            let (_, fragment) = self.parse_input(cx);
            return if entries.is_empty() || fragment.is_empty() {
                EmptyState::new("This directory is empty.")
                    .nothing(gpui_component::IconName::FolderOpen, cx)
                    .detail("Press ⏎ to open a session here.")
            } else {
                EmptyState::new(format!("Nothing here matches \u{201c}{fragment}\u{201d}."))
                    .nothing(gpui_component::IconName::Search, cx)
                    .detail("Keep typing a path, or press ⏎ to open it if it exists.")
            }
            .into_any_element();
        }

        let list = div()
            .id("file-opener-list")
            .role(gpui::accesskit::Role::ListBox)
            .aria_label("Files and directories")
            .size_full()
            .flex()
            .flex_col()
            .py_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .children(
                vis.into_iter()
                    .map(|ix| self.render_entry(ix, &entries[ix], cx)),
            );
        // The viewport fades at overflowing edges; the scrollbar appears on interaction.
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(crate::ui::components::scroll_viewport(
                list,
                &self.scroll_handle,
            ))
            .child(
                div().absolute().inset_0().child(
                    gpui_base::Scrollbar::new(&self.scroll_handle)
                        .id("file-opener-scrollbar")
                        .axis(gpui_base::ScrollbarAxis::Vertical)
                        .mode(gpui_base::ScrollbarMode::Hover)
                        .viewport_from_layout(),
                ),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        // Enter browses into a selected directory, opens a selected file, and
        // with nothing selected opens the directory on the line. The button
        // is that same press, named for what it will do, as in Releases.
        let selected = self.selected_entry();
        let enter = match selected {
            Some(entry) if entry.is_dir => "⏎ Browse",
            Some(_) => "⏎ Open file",
            None => "⏎ Open here",
        };
        // The path the press acts on, home written as `~`. With nothing
        // selected the field above already shows it, so say what happens.
        let target = match selected {
            Some(entry) => {
                let mut path = tilde(&entry.path);
                if entry.is_dir {
                    path.push(std::path::MAIN_SEPARATOR);
                }
                path
            }
            None => format!("Opens a session in {}", tilde(&self.cwd)),
        };
        panel_footer(cx)
            .child(footer_status(
                h_flex()
                    .gap_1()
                    .items_center()
                    .when_some(selected, |this, entry| {
                        this.child(if entry.is_dir { "Browse" } else { "Open" })
                    })
                    .child(
                        div()
                            .truncate()
                            .when(selected.is_some(), |this| this.mono(cx))
                            .child(target),
                    ),
                cx.theme().text_muted,
            ))
            .when(self.visible(cx).len() > 1, |this| {
                this.child(hint("↑↓", "choose", cx))
            })
            .child(hint("alt ↑", "parent", cx))
            .child(hint("esc", "close", cx))
            // With a row selected, enter no longer opens the directory
            // itself; keep that one click away.
            .when(selected.is_some(), |this| {
                this.child(
                    Button::new("open-directory")
                        .outline()
                        .small()
                        .text_size(px(12.))
                        .rounded(ButtonRounded::Small)
                        .label(format!("Session in {}", tilde(&self.cwd)))
                        .tooltip(format!("Start a session in {}", self.cwd.display()))
                        .on_click(cx.listener(|this, _, _, cx| this.open_directory(cx))),
                )
            })
            .child(
                Button::new("open-enter")
                    .primary()
                    .small()
                    .text_size(px(12.))
                    .rounded(ButtonRounded::Small)
                    .label(enter)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.on_submit(&Submit, window, cx)),
                    ),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.visible(cx).len();
        let total = self.listing.entries.as_ref().map_or(0, |entries| {
            entries
                .iter()
                .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
                .count()
        });
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
            .overflow_hidden()
            .child(
                editable_combo_box(
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
                    panel_header()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(16.))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(cx.theme().text)
                                        .child("Open in a new session"),
                                )
                                .child(self.render_hidden_toggle(cx))
                                .child(rule(cx))
                                .child(counter(shown, total, "item", cx)),
                        )
                        .child(self.render_input(cx)),
                )
                .child(self.render_list(cx))
                .child(self.render_footer(cx)),
            )
    }
}

// ------------------------------------------------------------------ helpers

/// Everything in the dialog but its list: header, footer, the list's own
/// padding, and the dialog's padding and border.
const CHROME_HEIGHT: f32 = 143.0;

/// The tallest dialog under `cap` whose list holds a whole number of rows.
pub(super) fn snap_height(cap: gpui::Pixels) -> gpui::Pixels {
    let rows = ((f32::from(cap) - CHROME_HEIGHT) / ROW_HEIGHT)
        .floor()
        .max(1.0);
    px(CHROME_HEIGHT + rows * ROW_HEIGHT)
}

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

/// `path` with the home directory written as `~`.
fn tilde(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
    }
    path.display().to_string()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
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
