//! Overlay orchestrator: shared dialog chrome and screen routing.
//!
//! Data-model split (same as t3chat `T3Chat`):
//! - `Overlay` holds its own state (fetched releases + the active screen)
//!   and implements `Render`, so it is owned as `Entity<Overlay>` (by
//!   `Surface`).
//! - Every screen is a stateful view (`Entity<T: Render>`) held by the
//!   orchestrator as `ActiveScreen`: `PaletteView`, `SessionsView`,
//!   `DetachedView`, `ReleasesView`, `OpenerView`. Searchable screens own
//!   an `Entity<SearchDialog>`; list rows inside them are `RenderOnce`
//!   snapshots (`ListRow`).

use gpui::{App, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use gpui_component::{WindowExt as _, dialog::Dialog};

use crate::Surface;

use super::{
    DetachedView, OpenerView, PaletteView, ReleasesView, Screen, SessionsView,
};

/// The screen currently shown, with its view entity. Held by the `Overlay`
/// orchestrator; each view constructor registers itself on open so every
/// open path (key action, `:command`, Lua) keeps it accurate.
pub enum ActiveScreen {
    Palette(Entity<PaletteView>),
    Sessions(Entity<SessionsView>),
    Detached(Entity<DetachedView>),
    Releases(Entity<ReleasesView>),
    Opener(Entity<OpenerView>),
}

impl ActiveScreen {
    pub fn screen(&self) -> Screen {
        match self {
            Self::Palette(_) => Screen::CmdPalette,
            Self::Sessions(_) => Screen::Sessions,
            Self::Detached(_) => Screen::Detached,
            Self::Releases(_) => Screen::Releases,
            Self::Opener(_) => Screen::Opener,
        }
    }
}

/// Orchestrator for all overlay dialogs.
///
/// Owns the release list (+ loading flag) and one instance of every screen
/// view, each constructed once on first open and reused after that. Dialog
/// requests are GPUI actions (`OpenOverlay`, dispatched from key bindings
/// or queued by Lua without a window and drained in `render`), so there is
/// no staging queue — handlers call `Overlay::open` directly with the
/// window at hand.
pub struct Overlay {
    releases: Vec<self_update::Release>,
    releases_loading: bool,
    palette: Option<Entity<PaletteView>>,
    sessions: Option<Entity<SessionsView>>,
    detached: Option<Entity<DetachedView>>,
    releases_view: Option<Entity<ReleasesView>>,
    opener: Option<Entity<OpenerView>>,
    active: Option<ActiveScreen>,
}

impl Overlay {
    pub fn new() -> Self {
        Self {
            releases: Vec::new(),
            releases_loading: false,
            palette: None,
            sessions: None,
            detached: None,
            releases_view: None,
            opener: None,
            active: None,
        }
    }

    /// Which screen (if any) was last opened through the orchestrator.
    pub fn active(&self) -> Option<Screen> {
        self.active.as_ref().map(ActiveScreen::screen)
    }

    /// The held view entity for the active screen, if any.
    pub fn active_view(&self) -> Option<&ActiveScreen> {
        self.active.as_ref()
    }

    pub fn clear_active(&mut self) {
        self.active = None;
    }

    /// Open a dialog immediately. Callers must have a window in scope —
    /// Lua without one queues an `OpenOverlay` action instead.
    pub fn open_overlay(
        &mut self,
        surface: Entity<Surface>,
        screen: Screen,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.open(surface, screen, window, cx);
    }

    pub fn ensure_releases_fetch(&mut self, event_tx: async_channel::Sender<crate::pty::Event>) {
        if self.releases.is_empty() && !self.releases_loading {
            self.releases_loading = true;
            crate::overlay::fetch_releases_async(event_tx);
        }
    }

    pub fn set_releases(&mut self, releases: Vec<self_update::Release>) {
        self.releases = releases;
        self.releases_loading = false;
    }

    pub fn releases(&self) -> &[self_update::Release] {
        &self.releases
    }

    pub fn releases_cloned(&self) -> Vec<self_update::Release> {
        self.releases.clone()
    }

    pub fn releases_loading(&self) -> bool {
        self.releases_loading
    }

    /// Open the dialog for `screen`, snapshotting data from `surface`.
    ///
    /// Reads snapshots via `cx` (no `&mut Surface` needed) so it can run
    /// from render (which owns a window) and from key handling without
    /// dead-locking on the surface's update lease. Each screen view is
    /// constructed once on first open and reused after that; `show`
    /// refreshes its snapshot and presents the reused dialog.
    pub fn open(&mut self, surface: Entity<Surface>, screen: Screen, window: &mut Window, cx: &mut App) {
        match screen {
            Screen::CmdPalette => {
                if self.palette.is_none() {
                    self.palette =
                        Some(cx.new(|cx| PaletteView::new(surface.clone(), window, cx)));
                }
                let view = self.palette.clone().expect("palette view just built");
                self.active = Some(ActiveScreen::Palette(view.clone()));
                PaletteView::show(view, window, cx);
            }
            Screen::Sessions => {
                if self.sessions.is_none() {
                    self.sessions =
                        Some(cx.new(|cx| SessionsView::new(surface.clone(), window, cx)));
                }
                let view = self.sessions.clone().expect("sessions view just built");
                self.active = Some(ActiveScreen::Sessions(view.clone()));
                SessionsView::show(view, window, cx);
            }
            Screen::Detached => {
                let current_tab = surface.read(cx).workspace.current_tab;
                if self.detached.is_none() {
                    self.detached =
                        Some(cx.new(|cx| DetachedView::new(surface.clone(), window, cx)));
                }
                let view = self.detached.clone().expect("detached view just built");
                self.active = Some(ActiveScreen::Detached(view.clone()));
                DetachedView::show(view, current_tab, window, cx);
            }
            Screen::Releases => {
                if self.releases_view.is_none() {
                    self.releases_view =
                        Some(cx.new(|cx| ReleasesView::new(surface.clone(), cx)));
                }
                let view = self
                    .releases_view
                    .clone()
                    .expect("releases view just built");
                self.active = Some(ActiveScreen::Releases(view.clone()));
                ReleasesView::show(view, window, cx);
            }
            Screen::Opener => {
                if self.opener.is_none() {
                    self.opener = Some(cx.new(|cx| OpenerView::new(surface.clone(), cx)));
                }
                let view = self.opener.clone().expect("opener view just built");
                self.active = Some(ActiveScreen::Opener(view.clone()));
                OpenerView::show(view, window, cx);
            }
        }
    }

    /// Apply shared dialog chrome and focus-back-to-terminal on close.
    pub fn shell(dialog: Dialog, surface: Entity<Surface>, window: &mut Window) -> Dialog {
        dialog
            .close_button(false)
            .p_0()
            .on_close(window.listener_for(&surface, |surface, _, window, cx| {
                surface.overlay.update(cx, |host, _| host.clear_active());
                window.focus(&surface.focus_handle, cx);
            }))
    }

    /// Open a dialog with shared chrome; `build` configures title/footer/content.
    pub fn present(
        surface: Entity<Surface>,
        window: &mut Window,
        cx: &mut App,
        build: impl 'static + Fn(Dialog, &mut Window, &mut App) -> Dialog,
    ) {
        window.open_dialog(cx, move |dialog, window, cx| {
            let dialog = Self::shell(dialog, surface.clone(), window);
            build(dialog, window, cx)
        });
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for Overlay {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Dialogs render through the dialog layer (`Root::render_dialog_layer`
        // in `Surface::render`), not inline — the orchestrator itself paints
        // nothing. It still implements `Render` so it is owned as
        // `Entity<Overlay>` like other stateful views.
        div()
    }
}
