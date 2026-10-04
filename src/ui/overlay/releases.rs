//! The release screen: a rail of versions beside the selected release's builds.

use std::{collections::HashMap, io::Write as _, path::Path};

use crate::{Next, Prev, PushError, PyTheme as _, Pyonji, Submit};
use anyhow::Context as _;
use async_compat::CompatExt;
use gpui::{prelude::FluentBuilder as _, *};
use gpui_base::{Disableable, ScrollbarAxis, actions::Cancel, h_flex, v_flex};
use gpui_component::{
    Icon, IconName, Sizable, WindowExt,
    button::{Button, ButtonRounded, ButtonVariants},
    input::{Input, InputEvent, InputState},
    progress::Progress,
    scroll::ScrollableElement,
    spinner::Spinner,
    text::TextView,
};
use reqwest::Client;
use self_update::{Release, backends::github};
use smol::stream::StreamExt as _;

const RAIL: f32 = 232.0;

const ROW: f32 = 62.0;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(ReleasesView::CONTEXT)),
        KeyBinding::new("up", Prev, Some(ReleasesView::CONTEXT)),
        KeyBinding::new("enter", Submit, Some(ReleasesView::CONTEXT)),
    ]);
}

/// The version this binary was built from.
///
/// The crate version, not `git_version!`, which answers with a bare commit hash
/// here because the tags are lightweight. A release tag carries the crate
/// version, so that is the number to compare against.
const RUNNING: &str = env!("CARGO_PKG_VERSION");

fn running_binary() -> String {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pyonji".to_string())
}

/// One downloadable binary in a release.
struct Build {
    /// `Linux x64`, `macOS ARM64`, `Windows x86`.
    label: String,
    /// The target triple, as it appears in the asset name.
    triple: String,
}

/// Everything the screen shows about one release, derived once per render.
struct Entry {
    /// Index into the fetched list, so a selection survives a refetch.
    ix: usize,
    version: String,
    date: String,
    /// A release name that says more than its own tag.
    title: Option<String>,
    notes: Option<String>,
    changelog: Option<String>,
    /// The build for this machine, when the release has one.
    mine: Option<Build>,
    others: Vec<Build>,
}

impl Entry {
    fn installable(&self) -> bool {
        self.mine.is_some()
    }

    /// Every build, this machine's included.
    fn build_count(&self) -> usize {
        self.others.len() + usize::from(self.mine.is_some())
    }
}

/// GitHub sends `created_at` as RFC 3339; the screen only shows the day.
fn release_date(release: &Release) -> String {
    let raw = release.date();
    let is_day = raw.len() > 10
        && raw.is_char_boundary(10)
        && raw[..10].bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        });
    if is_day {
        raw[..10].to_string()
    } else {
        raw.to_string()
    }
}

/// The target triple in a release asset name.
///
/// Assets are `<crate>-v<version>-<triple>`, so only the version comes off.
fn target_triple<'a>(asset: &'a str, version: &str) -> &'a str {
    let stem = asset.strip_suffix(".exe").unwrap_or(asset);
    let Some(rest) = stem.strip_prefix("pyonji-") else {
        return stem;
    };
    let rest = rest
        .strip_prefix(&format!("v{version}"))
        .or_else(|| rest.strip_prefix(version))
        .unwrap_or(rest);
    rest.strip_prefix('-').unwrap_or(rest)
}

/// The platform and CPU a target triple names, spelled for a person.
fn describe(triple: &str) -> String {
    let platform = if triple.contains("windows") {
        "Windows"
    } else if triple.contains("darwin") {
        "macOS"
    } else if triple.contains("linux") {
        "Linux"
    } else {
        "Other"
    };
    let cpu = if triple.starts_with("aarch64") {
        "ARM64"
    } else if triple.starts_with("x86_64") {
        "x64"
    } else if triple.starts_with("x86") {
        "x86"
    } else {
        ""
    };
    if cpu.is_empty() {
        platform.to_string()
    } else {
        format!("{platform} {cpu}")
    }
}

fn bare_url(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest)
}

/// The URL in a line that is only GitHub's own changelog link.
///
/// With no written notes, GitHub fills a release body with
/// `**Full Changelog**: <url>`, once per closed issue.
fn changelog_url(line: &str) -> Option<&str> {
    let url = match line.rsplit_once(": ") {
        Some((label, url))
            if label
                .trim_matches(['*', ' ', '`'])
                .eq_ignore_ascii_case("full changelog") =>
        {
            url.trim()
        }
        _ => line.trim(),
    };
    let url = url.trim_matches(|c| matches!(c, '*' | '`' | ' ' | '<' | '>'));
    (url.starts_with("https://") && (url.contains("/compare/") || url.contains("/commits/")))
        .then_some(url)
}

/// Split a release body into written notes and the one changelog link.
///
/// Every Pyonji release body is entirely the second kind, which would otherwise
/// render as a paragraph of the same link three times over.
fn split_notes(body: Option<&str>) -> (Option<String>, Option<String>) {
    let mut notes: Vec<&str> = Vec::new();
    let mut changelog = None;

    for line in body.unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(url) = changelog_url(line) {
            changelog.get_or_insert_with(|| url.to_string());
            continue;
        }
        if !notes.contains(&line) {
            notes.push(line);
        }
    }

    let notes = (!notes.is_empty()).then(|| notes.join("\n\n"));
    (notes, changelog)
}

#[derive(Clone, Debug, PartialEq)]
struct UpdateTarget {
    version: String,
    asset_url: String,
}

#[derive(Clone, PartialEq)]
enum UpdateState {
    Idle,
    Updating { target: UpdateTarget, progress: f32 },
    Complete { target: UpdateTarget },
}

pub struct ReleasesView {
    _pyonji: WeakEntity<Pyonji>,

    releases: Option<Vec<Release>>,
    fetch_error: Option<String>,
    update_state: UpdateState,

    binary: String,

    focus_handle: FocusHandle,
    rail_scroll: ScrollHandle,
    body_scroll: ScrollHandle,
    selected: Option<usize>,
    search_input: Entity<InputState>,
}

impl ReleasesView {
    pub const CONTEXT: &str = "ReleasesView";

    pub fn new(pyonji: &WeakEntity<Pyonji>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::fetch(window, cx);

        // Not `clean_on_escape()`: that swallows escape without propagating
        // it, and the dialog binds escape on its own key context, so the field
        // would leave no way off the screen. See `on_cancel`.
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter versions"));

        let focus_handle = cx.focus_handle();
        cx.on_focus_in(&focus_handle, window, |this, window, cx| {
            // The filter only earns its place if it takes keystrokes. The
            // screen's own Next/Prev/Submit bindings are context-scoped and
            // survive that: different actions from the ones the input handles.
            let focus = this.search_input.focus_handle(cx).clone();
            window.focus(&focus, cx);
        })
        .detach();

        cx.subscribe(&search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                // Keep the selection where the filter left it, if it is still
                // on screen.
                let entries = this.entries(cx);
                let selected = entries
                    .iter()
                    .find(|e| Some(e.ix) == this.selected)
                    .or_else(|| entries.first())
                    .map(|e| e.ix);
                this.select(selected, cx);
            }
        })
        .detach();

        Self {
            _pyonji: pyonji.clone(),

            releases: None,
            fetch_error: None,
            update_state: UpdateState::Idle,

            binary: running_binary(),

            focus_handle,
            rail_scroll: ScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            selected: None,
            search_input,
        }
    }

    fn fetch(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async |this, cx| -> Result<()> {
            let list = github::ReleaseList::configure()
                .repo_owner("Scar-00")
                .repo_name("Pyonji")
                .build()
                .inspect_err(|e| {
                    _ = cx.update(|window, cx| {
                        window.dispatch_action(
                            Box::new(PushError::new(format!("failed to build release list: {e}"))),
                            cx,
                        );
                    });
                })?;
            let releases = list.fetch_async().compat().await.inspect_err(|e| {
                _ = cx.update(|window, cx| {
                    window.dispatch_action(
                        Box::new(PushError::new(format!("failed to fetch release list: {e}"))),
                        cx,
                    );
                });
            });

            _ = this.update(cx, |this, cx| {
                match releases {
                    Ok(releases) => {
                        let version = this
                            .selected
                            .and_then(|ix| this.releases.as_ref()?.get(ix))
                            .map(|release| release.version().to_string());
                        this.releases = Some(releases.into_vec());
                        this.fetch_error = None;
                        // A refetch must not leave the selection past the end
                        // of the list; a first load opens on the newest.
                        let entries = this.entries(cx);
                        let selected = entries
                            .iter()
                            .find(|entry| Some(&entry.version) == version.as_ref())
                            .or_else(|| entries.first())
                            .map(|entry| entry.ix);
                        this.select(selected, cx);
                    }
                    Err(e) => {
                        this.fetch_error = Some(e.to_string());
                    }
                }
                cx.notify();
            });
            Ok(())
        })
        .detach();
    }

    /// The releases the filter admits, newest first.
    fn entries(&self, cx: &App) -> Vec<Entry> {
        let Some(releases) = &self.releases else {
            return Vec::new();
        };
        let query = self.search_input.read(cx).value().to_lowercase();

        releases
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                query.is_empty()
                    || r.version().to_lowercase().contains(&query)
                    || r.name().to_lowercase().contains(&query)
                    || r.body().is_some_and(|b| b.to_lowercase().contains(&query))
            })
            .map(|(ix, r)| Self::entry(ix, r))
            .collect()
    }

    fn entry(ix: usize, release: &Release) -> Entry {
        let version = release.version().to_string();
        let (notes, changelog) = split_notes(release.body());

        // GitHub's default release name is the tag, which the version already
        // says.
        let title = (release.name() != format!("v{version}")).then(|| release.name().to_string());

        let build = |name: &str| {
            let triple = target_triple(name, &version);
            Build {
                label: describe(triple),
                triple: triple.to_string(),
            }
        };

        // `asset_for` already matches the running target, so the installable
        // build is found the way the install will find it. It leads the list.
        let (mine, others): (Option<Build>, Vec<Build>) =
            match release.asset_for(self_update::get_target(), None) {
                Some(asset) => {
                    let mine = asset.name().to_string();
                    let others = release
                        .assets()
                        .iter()
                        .map(|a| a.name())
                        .filter(|name| *name != mine.as_str())
                        .map(build)
                        .collect();
                    (Some(build(&mine)), others)
                }
                None => (
                    None,
                    release.assets().iter().map(|a| build(a.name())).collect(),
                ),
            };

        Entry {
            ix,
            version,
            date: release_date(release),
            title,
            notes,
            changelog,
            mine,
            others,
        }
    }

    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        // `set_value` does not emit a change, so the bookkeeping is redone.
        let entries = self.entries(cx);
        let selected = entries
            .iter()
            .find(|e| Some(e.ix) == self.selected)
            .or_else(|| entries.first())
            .map(|e| e.ix);
        self.select(selected, cx);
    }

    /// Escape peels one layer: it drops a filter that is narrowing the list,
    /// and otherwise falls through to the dialog, which closes.
    ///
    /// The dialog raises `Cancel` from its own binding and actions bubble
    /// upwards, so this runs first. Stopping the event is what keeps a cleared
    /// filter from also closing the screen.
    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.read(cx).value().is_empty() {
            return cx.propagate();
        }
        self.clear_filter(window, cx);
        cx.stop_propagation();
    }

    fn selected_entry<'a>(&self, entries: &'a [Entry]) -> Option<&'a Entry> {
        entries.iter().find(|e| Some(e.ix) == self.selected)
    }

    fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != ix {
            self.body_scroll.set_offset(point(px(0.0), px(0.0)));
        }
        self.selected = ix;
        if let Some(position) = self
            .entries(cx)
            .iter()
            .position(|entry| Some(entry.ix) == ix)
        {
            self.rail_scroll.scroll_to_item(position);
        }
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let entries = self.entries(cx);
        if entries.is_empty() {
            return;
        }
        let len = entries.len() as isize;
        let at = entries
            .iter()
            .position(|e| Some(e.ix) == self.selected)
            .map(|at| (at as isize + delta).rem_euclid(len))
            .unwrap_or(0);
        self.select(Some(entries[at as usize].ix), cx);
    }

    fn on_select_next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn on_select_prev(&mut self, _: &Prev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn on_confirm(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.update_state != UpdateState::Idle {
            return;
        }

        let Some(ix) = self.selected else { return };
        let Some(release) = self.releases.as_ref().and_then(|r| r.get(ix)).cloned() else {
            return;
        };

        let Some(asset) = release.asset_for(self_update::get_target(), None) else {
            window.dispatch_action(
                Box::new(PushError::new(format!(
                    "no build for {} in v{}",
                    self_update::get_target(),
                    release.version()
                ))),
                cx,
            );
            return;
        };

        let target = UpdateTarget {
            version: release.version().to_string(),
            asset_url: asset.download_url().to_string(),
        };
        self.update_state = UpdateState::Updating {
            target: target.clone(),
            progress: 0.0,
        };
        cx.notify();

        cx.spawn_in_with_priority(Priority::RealtimeAudio, window, async move |this, cx| {
            if let Err(e) = Self::download_self(&this, target.clone(), cx).await {
                _ = this.update(cx, |this, cx| {
                    this.update_state = UpdateState::Idle;
                    cx.notify();
                });
                _ = cx.update(|window, cx| {
                    window.dispatch_action(
                        Box::new(PushError::new(format!(
                            "Could not install v{}: {e}",
                            target.version
                        ))),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    async fn download_self(
        this: &WeakEntity<Self>,
        target: UpdateTarget,
        cx: &mut AsyncWindowContext,
    ) -> Result<()> {
        let client = Client::new();
        let res = client
            .get(&target.asset_url)
            .header(reqwest::header::USER_AGENT, "Pyonji")
            .send()
            .compat()
            .await?
            .error_for_status()?;
        let body = res.json::<AssetResult>().compat().await?;
        let res = client
            .get(body.browser_download_url)
            .header(reqwest::header::USER_AGENT, "Pyonji")
            .send()
            .compat()
            .await?
            .error_for_status()?;
        let header = res
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .context("no content-length in download")?;
        let length = header.to_str().map(str::parse::<usize>)??;
        if length == 0 {
            return Err(anyhow::anyhow!("update download is empty"));
        }
        let mut stream = res.bytes_stream();
        let mut file = tempfile::NamedTempFile::new()?;
        let mut downloaded = 0;
        while let Some(chunk) = stream.next().compat().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            downloaded += chunk.len();
            let progress = ((downloaded as f64 / length as f64) * 100.0).clamp(0.0, 100.0) as f32;
            _ = this.update(cx, |this, cx| {
                if let UpdateState::Updating {
                    progress: current, ..
                } = &mut this.update_state
                {
                    *current = progress;
                }
                cx.notify();
            });
        }
        _ = this.update(cx, |this, cx| {
            if let UpdateState::Updating { progress, .. } = &mut this.update_state {
                *progress = 100.0;
            }
            cx.notify();
        });
        anyhow::ensure!(
            downloaded == length,
            "incomplete update download: received {downloaded} of {length} bytes"
        );
        let path = file.path();
        self_replace::self_replace(path)?;
        _ = this.update(cx, |this, cx| {
            this.update_state = UpdateState::Complete { target };
            cx.notify();
        });
        Ok(())
    }

    fn status(&self, cx: &Context<Self>) -> Option<(String, Rgba)> {
        let theme = cx.theme();
        let releases = self.releases.as_ref()?;
        let newest = releases.first()?;
        if newest.version() == RUNNING {
            return Some(("the newest release".to_string(), theme.success));
        }
        Some(if releases.iter().any(|r| r.version() == RUNNING) {
            (format!("v{} is available", newest.version()), theme.warning)
        } else {
            ("running an untagged build".to_string(), theme.text_muted)
        })
    }

    // ---------------------------------------------------------------- render

    fn render_header(
        &self,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let filtering = self
            .search_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);

        h_flex()
            .min_h(px(56.0))
            .py_2()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_3()
                    .font_family(mono(cx))
                    .text_size(px(14.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(theme.text_muted)
                            .child("Running"),
                    )
                    .child(div().flex_shrink_0().text_color(theme.text).child(RUNNING))
                    .when_some(
                        (!compact).then(|| self.status(cx)).flatten(),
                        |this, (status, color)| {
                            this.child(rule(theme))
                                .child(div().min_w_0().truncate().text_color(color).child(status))
                        },
                    ),
            )
            .child(div().flex_1())
            .child(
                // A ruled field to match the screen, lit only while focused so
                // the caret is not the only focus cue.
                h_flex()
                    .flex_1()
                    .min_w(px(120.0))
                    .max_w(px(240.0))
                    .h(px(32.0))
                    .items_center()
                    .gap_2()
                    .px_3()
                    .font_family(mono(cx))
                    .border_1()
                    .border_color(if filtering {
                        theme.focus_ring
                    } else {
                        theme.border
                    })
                    .child(
                        Icon::new(IconName::Search)
                            .small()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        Input::new(&self.search_input)
                            .role(gpui_base::RoleOverride::Presentational)
                            .flex_1()
                            .appearance(false)
                            .bordered(false),
                    ),
            )
            .child(
                Button::new("reload")
                    .ghost()
                    .small()
                    .rounded(ButtonRounded::None)
                    .icon(IconName::Redo2)
                    .tooltip("Fetch releases again")
                    .on_click(cx.listener(|_, _, window, cx| {
                        Self::fetch(window, cx);
                    })),
            )
    }

    fn render_rail(
        &self,
        entries: &[Entry],
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .id("releases-rail")
            .role(gpui::accesskit::Role::ListBox)
            .aria_label("Release versions")
            .when(compact, |rail| rail.w_full().h(px(ROW * 2.0)).border_b_1())
            .when(!compact, |rail| rail.w(px(RAIL)).border_r_1())
            .when(!compact, |rail| rail.flex_shrink_0())
            .when(compact, |rail| rail.flex_shrink_1().min_h_0())
            .border_color(theme.border)
            .overflow_y_scroll()
            .track_scroll(&self.rail_scroll)
            .scrollbar(&self.rail_scroll, ScrollbarAxis::Vertical)
            .children(entries.iter().map(|e| self.render_version(e, cx)))
    }

    fn render_version(&self, entry: &Entry, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = Some(entry.ix) == self.selected;
        let running = entry.version == RUNNING;
        let ix = entry.ix;

        gpui_base::Button::new(("version", entry.ix))
            .accessibility_label(format!("View release v{}", entry.version))
            .role(gpui::accesskit::Role::ListBoxOption)
            .aria_selected(selected)
            .focusable(false)
            .tab_stop(false)
            .when(selected, |row| row.aria_active_descendant())
            .focus_visible(|style| style.border_color(theme.focus_ring))
            .w_full()
            .h(px(ROW))
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .pl_4()
            .pr_3()
            // A rule and a raised fill, so selection cannot be mistaken for
            // hover.
            .border_l_2()
            .border_color(if selected {
                theme.selected_border
            } else {
                none()
            })
            .when(selected, |this| this.bg(theme.surface_elevated))
            .when(!selected, |this| this.hover(|s| s.bg(theme.hovered)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(Some(ix), cx);
            }))
            .child(
                // A fixed slot for the mark showing which release is running.
                div()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .child(
                        div()
                            .size(px(6.0))
                            .bg(if running { theme.accent } else { none() }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(mono(cx))
                    .text_size(px(22.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(entry.version.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(mono(cx))
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(entry.date.clone()),
            )
    }

    fn render_body(
        &self,
        entries: &[Entry],
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        if self.releases.is_none() {
            return match &self.fetch_error {
                Some(reason) => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .px_8()
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(theme.text)
                            .child("Could not fetch releases from GitHub."),
                    )
                    .child(
                        div()
                            .max_w(px(440.0))
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(reason.clone()),
                    )
                    .child(
                        Button::new("retry")
                            .secondary()
                            .small()
                            .rounded(ButtonRounded::None)
                            .label("Try again")
                            .on_click(cx.listener(|_, _, window, cx| {
                                Self::fetch(window, cx);
                            })),
                    )
                    .into_any_element(),
                None => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(Spinner::new())
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(theme.text_muted)
                            .child("Fetching releases from GitHub…"),
                    )
                    .into_any_element(),
            };
        }

        if entries.is_empty() {
            let query = self.search_input.read(cx).value();
            let filtered = !query.is_empty();
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    Icon::new(IconName::Inbox)
                        .size_8()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(theme.text_muted)
                        .child(if filtered {
                            format!("No release matches \u{201c}{query}\u{201d}.")
                        } else {
                            "No releases yet.".to_string()
                        }),
                )
                .when(filtered, |this| {
                    this.child(
                        gpui_base::Button::new("clear-filter")
                            .accessibility_label("Clear release filter")
                            .focus_visible(|style| style.border_color(theme.focus_ring))
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(12.0))
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|s| s.border_color(theme.accent))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.clear_filter(window, cx)),
                            )
                            .child("Clear filter"),
                    )
                })
                .into_any_element();
        }

        div()
            .flex()
            .when(compact, |body| body.flex_col())
            .when(!compact, |body| body.flex_row())
            .flex_1()
            .min_h_0()
            // `h_flex` centres children on the cross axis; the panes fill it.
            .items_stretch()
            .child(self.render_rail(entries, compact, cx))
            .child(self.render_manifest(self.selected_entry(entries), compact, cx))
            .into_any_element()
    }

    fn render_manifest(
        &self,
        entry: Option<&Entry>,
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        let Some(entry) = entry else {
            return v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(theme.text_muted)
                        .child("No release selected."),
                )
                .into_any_element();
        };

        v_flex()
            .id("releases-manifest")
            .min_h_0()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.body_scroll)
            .scrollbar(&self.body_scroll, ScrollbarAxis::Vertical)
            .child(
                v_flex()
                    .w_full()
                    .gap_5()
                    .px_3()
                    .py_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(mono(cx))
                                    .text_size(px(30.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .letter_spacing(px(-0.5))
                                    .line_height(relative(1.1))
                                    .text_color(theme.text)
                                    .child(entry.version.clone()),
                            )
                            .when_some(entry.title.clone(), |this, title| {
                                this.child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text)
                                        .child(title),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .font_family(mono(cx))
                            .text_size(px(12.0))
                            .child(div().text_color(theme.text_muted).child(entry.date.clone()))
                            .child(rule(theme))
                            .child(div().text_color(theme.text_muted).child(match entry.mine {
                                Some(_) => {
                                    format!("{} builds, 1 for this machine", entry.build_count())
                                }
                                None => format!("{} builds", entry.build_count()),
                            })),
                    )
                    .child(self.render_builds(entry, compact, cx))
                    .child(self.render_notes(entry, cx)),
            )
            .into_any_element()
    }

    fn render_builds(&self, entry: &Entry, compact: bool, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let row = |build: &Build, mine: bool| {
            h_flex()
                .w_full()
                .flex_shrink_0()
                .items_center()
                .gap_3()
                .min_h(px(30.0))
                .when(compact, |row| row.flex_col().items_start().gap_1().py_1())
                .pl_3()
                // The same accent rule the rail uses for selection marks the
                // build that works here.
                .border_l_2()
                .border_color(if mine { theme.selected_border } else { none() })
                .child(
                    div()
                        .when(!compact, |label| label.w(px(112.0)))
                        .flex_shrink_0()
                        .text_size(px(13.0))
                        .text_color(if mine { theme.text } else { theme.text_muted })
                        .child(build.label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(mono(cx))
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child(build.triple.clone()),
                )
        };

        v_flex()
            .w_full()
            .flex_shrink_0()
            .gap_0p5()
            .when_some(entry.mine.as_ref(), |this, mine| {
                this.child(row(mine, true))
            })
            .children(entry.others.iter().map(|b| row(b, false)))
            .when(!entry.installable(), |this| {
                this.child(
                    div()
                        .w_full()
                        .pt_1()
                        .text_size(px(13.0))
                        .text_color(theme.error)
                        .child(format!(
                            "No build for {} in this release.",
                            self_update::get_target()
                        )),
                )
            })
    }

    fn render_notes(&self, entry: &Entry, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .w_full()
            .flex_shrink_0()
            .gap_2()
            .when_some(entry.notes.clone(), |this, notes| {
                this.child(
                    TextView::markdown(("notes", entry.ix), notes)
                        .text_size(px(13.0))
                        .text_color(theme.text),
                )
            })
            .when(entry.notes.is_none(), |this| {
                this.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(theme.text_muted)
                        .child("This release has no notes."),
                )
            })
            .when_some(entry.changelog.clone(), |this, url| {
                this.child(
                    gpui_base::Button::new(("changelog", entry.ix))
                        .accessibility_label("Read the changelog on GitHub")
                        .focus_visible(|style| style.border_color(theme.focus_ring))
                        .flex_wrap()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .border_1()
                        .border_color(theme.border)
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme.accent))
                        .on_click({
                            let url = url.clone();
                            move |_, _, cx: &mut App| cx.open_url(&url)
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(theme.accent)
                                .child("Read the changelog on GitHub"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_right()
                                .font_family(mono(cx))
                                .text_size(px(11.0))
                                .text_color(theme.text_muted)
                                .child(bare_url(&url).to_string()),
                        ),
                )
            })
    }

    fn render_footer(
        &self,
        entry: Option<&Entry>,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        match &self.update_state {
            UpdateState::Idle => self
                .render_install_footer(entry, window, cx)
                .into_any_element(),
            UpdateState::Updating { target, progress } => self
                .render_update_footer(target, *progress, cx)
                .into_any_element(),
            UpdateState::Complete { target } => {
                self.render_complete_footer(target, cx).into_any_element()
            }
        }
    }

    fn render_install_footer(
        &self,
        entry: Option<&Entry>,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let installable = entry.is_some_and(Entry::installable);
        // Below this the footer's parts fight for room and all three truncate.
        let roomy = window.viewport_size().width >= px(900.);

        h_flex()
            .min_h(px(56.0))
            .py_2()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_4()
            .px_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                // An install replaces the running binary; say so before the
                // button is pressed.
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_3()
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .when(entry.is_some(), |this| {
                        this.child(div().child("Replaces ")).child(
                            div()
                                .font_family(mono(cx))
                                .truncate()
                                .child(self.binary.clone()),
                        )
                    })
                    .when(entry.is_none(), |this| this.child("Nothing selected"))
                    .when(roomy && entry.is_some(), |this| {
                        this.child(rule(theme))
                            .child(div().child(if installable {
                                "for"
                            } else {
                                "unavailable for"
                            }))
                            .child(
                                div()
                                    .font_family(mono(cx))
                                    .text_color(if installable {
                                        theme.text_muted
                                    } else {
                                        theme.error
                                    })
                                    .truncate()
                                    .child(self_update::get_target()),
                            )
                    }),
            )
            .when(roomy, |this| this.child(self.render_hints(cx)))
            .child(
                Button::new("install")
                    .primary()
                    .small()
                    .rounded(ButtonRounded::None)
                    .label(match entry {
                        Some(e) if e.installable() => format!("Install v{}", e.version),
                        Some(e) => format!("No build for v{}", e.version),
                        None => "Install".to_string(),
                    })
                    .disabled(!installable)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.on_confirm(&Submit, window, cx)),
                    ),
            )
    }

    fn render_hints(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let hint = |keys: &'static str, what: &'static str| {
            h_flex()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .font_family(mono(cx))
                        .text_size(px(11.0))
                        .text_color(theme.text)
                        .child(keys),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(what),
                )
        };

        h_flex()
            .flex_shrink_0()
            .gap_4()
            .child(hint("↑↓", "choose"))
            .child(hint("⏎", "install"))
            .child(hint(
                "esc",
                if self.search_input.read(cx).value().is_empty() {
                    "close"
                } else {
                    "clear filter"
                },
            ))
    }

    fn render_update_footer(
        &self,
        target: &UpdateTarget,
        progress: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let progress = progress.clamp(0.0, 100.0);
        let theme = cx.theme();
        let version = &target.version;

        v_flex()
            .flex_shrink_0()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_size(px(13.0))
                    .child(
                        div()
                            .text_color(theme.text)
                            .child(format!("Downloading v{version}")),
                    )
                    .child(
                        div()
                            .font_family(mono(cx))
                            .text_color(theme.text_muted)
                            .child(format!("{progress:.0}%")),
                    ),
            )
            .child(
                Progress::new("release-update-progress")
                    .value(progress)
                    .small()
                    .accessibility_label("Release update progress"),
            )
    }

    fn render_complete_footer(
        &self,
        target: &UpdateTarget,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        h_flex()
            .min_h(px(56.0))
            .py_2()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_4()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.0))
                    .text_color(theme.text)
                    .child(format!("Installed v{}. Restart to run it.", target.version)),
            )
            .child(
                Button::new("restart-later")
                    .ghost()
                    .small()
                    .rounded(ButtonRounded::None)
                    .label("Not now")
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.close_dialog(cx);
                    })),
            )
            .child(
                Button::new("restart")
                    .primary()
                    .small()
                    .rounded(ButtonRounded::None)
                    .label("Restart")
                    .on_click(cx.listener(|_, _, _, cx| cx.restart())),
            )
    }
}

impl Focusable for ReleasesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ReleasesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = self.entries(cx);
        let selected = self.selected_entry(&entries);
        let compact = window.viewport_size().width < px(764.0);

        v_flex()
            .id("releases-view")
            .key_context(Self::CONTEXT)
            .tab_group()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_cancel))
            .size_full()
            .child(
                crate::ui::combo_box::editable_combo_box(
                    "release-search-results",
                    "Releases",
                    "Filter versions",
                    &self.search_input,
                    true,
                    cx,
                )
                .size_full()
                .min_h_0()
                .child(self.render_header(compact, window, cx))
                .child(self.render_body(&entries, compact, cx))
                .child(self.render_footer(selected, window, cx)),
            )
    }
}

// ------------------------------------------------------------------ helpers

fn mono(cx: &App) -> SharedString {
    gpui_component::ActiveTheme::theme(cx)
        .mono_font_family
        .clone()
}

/// A border or fill that should show nothing still needs a colour.
fn none() -> Rgba {
    rgba(0x00000000)
}

fn rule(theme: &crate::Theme) -> impl IntoElement {
    div()
        .w(px(1.0))
        .h(px(14.0))
        .flex_shrink_0()
        .bg(theme.border)
}

#[derive(Debug, serde::Deserialize)]
struct AssetResult {
    browser_download_url: String,
    #[serde(flatten)]
    _rest: HashMap<String, serde_json::Value>,
}
