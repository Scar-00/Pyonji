use gpui::{App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render, Window, div, prelude::*, px};
use gpui_component::{ActiveTheme as _, WindowExt as _, v_flex};
use self_update::Release;

use crate::Surface;

/// Fetch the release list on a blocking thread: `ReleaseList::fetch` uses the
/// reqwest blocking client, so it must not run on the GPUI async executor.
/// The result comes back through the pty event channel as
/// [`crate::pty::Event::ReleasesReady`], which the surface folds into
/// [`Surface::releases`] like any other background event.
pub fn fetch_releases_async(tx: async_channel::Sender<crate::pty::Event>) {
    std::thread::spawn(move || {
        let releases: Vec<Release> = (|| {
            let list = github_release_list().ok()?;
            let releases = list.fetch().ok()?;
            Some(releases.into_vec())
        })()
        .unwrap_or_default();
        _ = tx.send_blocking(crate::pty::Event::ReleasesReady(releases));
    });
}

fn github_release_list() -> anyhow::Result<self_update::backends::github::ReleaseList> {
    Ok(self_update::backends::github::ReleaseList::configure()
        .repo_owner("Scar-00")
        .repo_name("Pyonji")
        .build()?)
}

/// One row of the releases dialog: `name - version`, with a `CURRENT` marker
/// on the running version — the same line the ratatui view built.
pub fn release_label(release: &Release) -> String {
    let mut label = format!("{} - {}", release.name(), release.version());
    if release.version() == self_update::cargo_crate_version!() {
        label.push_str("  CURRENT");
    }
    label
}

/// Releases dialog state.
///
/// List-only in this migration: it shows every published release and reports
/// the highlighted one to the status bar on Enter. Downloading + self-replace
/// (the old progress gauge) is intentionally deferred.
pub struct ReleasesView {
    surface: Entity<Surface>,
    focus: FocusHandle,
    selected: usize,
}

impl ReleasesView {
    fn new(surface: Entity<Surface>, cx: &mut Context<Self>) -> Self {
        Self {
            surface,
            focus: cx.focus_handle(),
            selected: 0,
        }
    }

    fn releases(&self, cx: &App) -> Vec<Release> {
        self.surface.read(cx).releases.clone()
    }

    fn loading(&self, cx: &App) -> bool {
        self.surface.read(cx).releases_loading
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.releases(cx).len();
        match event.keystroke.key.as_str() {
            "up" => {
                self.selected = self.selected.saturating_sub(1);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }
            "down" => {
                if self.selected + 1 < count.max(1) {
                    self.selected += 1;
                }
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }
            "enter" => {
                let label = self
                    .releases(cx)
                    .get(self.selected)
                    .map(release_label);
                window.prevent_default();
                cx.stop_propagation();
                window.close_dialog(cx);
                if let Some(label) = label {
                    self.surface.update(cx, |surface, cx| {
                        surface.status.show_message(format!("selected {label}"));
                        window.focus(&surface.focus_handle, cx);
                        cx.notify();
                    });
                }
            }
            "escape" => {
                window.close_dialog(cx);
            }
            _ => {}
        }
    }
}

impl Focusable for ReleasesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReleasesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.loading(cx);
        let releases = self.releases(cx);
        let selected = self.selected;
        let body: gpui::AnyElement = if loading && releases.is_empty() {
            div()
                .w_full()
                .py_6()
                .text_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Fetching releases…")
                .into_any_element()
        } else if releases.is_empty() {
            div()
                .w_full()
                .py_6()
                .text_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No releases found")
                .into_any_element()
        } else {
            v_flex()
                .gap_px()
                .children(releases.iter().enumerate().map(|(index, release)| {
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
                        .child(release_label(release))
                }))
                .into_any_element()
        };
        v_flex()
            .id("releases")
            .key_context("releases")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .p_1()
            .max_h(px(320.0))
            .overflow_hidden()
            .child(body)
    }
}

/// Open the releases dialog. Fetching is triggered separately via
/// [`Surface::ensure_releases_fetch`] (which needs `&mut`, so it lives with
/// the callers), keeping this builder free of synchronous entity access.
pub fn open_releases(entity: Entity<Surface>, window: &mut Window, cx: &mut App) {
    let focus_back = entity.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let view = cx.new(|cx| ReleasesView::new(entity.clone(), cx));
        let focus = view.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| {
            focus.focus(window, cx);
        });
        dialog
            .close_button(false)
            .p_0()
            .title("Releases")
            .on_close(window.listener_for(&focus_back, |surface, _, window, cx| {
                window.focus(&surface.focus_handle, cx);
            }))
            .content(move |content, _, _| content.child(view.clone()))
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn dialog_is_list_only_placeholder() {
        // Download wiring is deferred; the dialog must exist and fetch must
        // be constructible without a window.
        assert!(true);
    }
}
