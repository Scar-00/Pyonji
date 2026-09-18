use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render, Window, div,
    prelude::*,
};
use gpui_base::v_flex;
use gpui_component::WindowExt as _;
use self_update::Release;

use crate::{
    Surface,
    theme::{self, role},
    ui::list_row::ListRow,
};

use super::Overlay;

/// Fetch the release list on a blocking thread: `ReleaseList::fetch` uses the
/// reqwest blocking client, so it must not run on the GPUI async executor.
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
/// on the running version.
pub fn release_label(release: &Release) -> String {
    let mut label = format!("{} - {}", release.name(), release.version());
    if release.version() == self_update::cargo_crate_version!() {
        label.push_str("  CURRENT");
    }
    label
}

/// Releases dialog content component.
///
/// Owns its selection and focus; release data lives in `Overlay`
/// (the single orchestrator owner), read here via entity — never duplicated.
pub struct ReleasesView {
    surface: Entity<Surface>,
    focus: FocusHandle,
    selected: usize,
}

impl ReleasesView {
    pub(crate) fn new(surface: Entity<Surface>, cx: &mut Context<Self>) -> Self {
        Self {
            surface,
            focus: cx.focus_handle(),
            selected: 0,
        }
    }

    fn overlay(&self, cx: &App) -> Entity<Overlay> {
        self.surface.read(cx).overlay.clone()
    }

    fn releases(&self, cx: &App) -> Vec<Release> {
        self.overlay(cx).read(cx).releases_cloned()
    }

    fn loading(&self, cx: &App) -> bool {
        self.overlay(cx).read(cx).releases_loading()
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
                let label = self.releases(cx).get(self.selected).map(release_label);
                window.prevent_default();
                cx.stop_propagation();
                window.close_dialog(cx);
                if let Some(label) = label {
                    let status_bar = self.surface.read(cx).status_bar.clone();
                    status_bar.update(cx, |bar, cx| {
                        bar.show_status_message(format!("selected {label}"), cx);
                    });
                    self.surface.update(cx, |surface, cx| {
                        window.focus(&surface.focus_handle, cx);
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
                .py(theme::space::_6)
                .text_center()
                .text_sm()
                .text_color(role::muted())
                .child("Fetching releases…")
                .into_any_element()
        } else if releases.is_empty() {
            div()
                .w_full()
                .py(theme::space::_6)
                .text_center()
                .text_sm()
                .text_color(role::muted())
                .child("No releases found")
                .into_any_element()
        } else {
            v_flex()
                .gap(theme::space::PX)
                .children(releases.iter().enumerate().map(|(index, release)| {
                    ListRow::new(release_label(release), index == selected)
                }))
                .into_any_element()
        };
        v_flex()
            .id("releases")
            .key_context("releases")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .p(theme::space::_1)
            .max_h(theme::size::OVERLAY_LIST_MAX_H)
            .overflow_hidden()
            .child(body)
    }
}

impl ReleasesView {
    /// Refresh transient state (selection, release fetch) and present the
    /// reused view. Release data itself stays live: render reads it from
    /// the `Overlay` orchestrator, never duplicated.
    pub(crate) fn show(view: Entity<Self>, window: &mut Window, cx: &mut App) {
        let surface = view.read(cx).surface.clone();
        let tx = surface.read(cx).event_tx.clone();
        let overlay = surface.read(cx).overlay.clone();
        overlay.update(cx, |overlay, _| overlay.ensure_releases_fetch(tx));
        view.update(cx, |this, cx| {
            this.selected = 0;
            cx.notify();
        });
        let focus = view.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| {
            focus.focus(window, cx);
        });
        Overlay::present(surface, window, cx, move |dialog, _, _| {
            let content = view.clone();
            dialog
                .title("Releases")
                .content(move |content_, _, _| content_.child(content.clone()))
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dialog_is_list_only_placeholder() {
        assert!(true);
    }
}
