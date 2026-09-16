//! OverlayHost entity: fetched release state.
//!
//! Owns the release list plus its loading flag. Dialog requests are GPUI
//! actions now (`OpenOverlay`, dispatched from key bindings or queued by
//! Lua without a window and drained in `render`), so the host no longer
//! needs its own staging queue — handlers call `Overlay::open` directly
//! with the window at hand.

use gpui::{App, Entity, Window};

use crate::{Surface, overlay::Screen};

pub struct OverlayHost {
    releases: Vec<self_update::Release>,
    releases_loading: bool,
}

impl OverlayHost {
    pub fn new() -> Self {
        Self {
            releases: Vec::new(),
            releases_loading: false,
        }
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
        crate::overlay::Overlay::open(screen, surface, window, cx);
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
}

impl Default for OverlayHost {
    fn default() -> Self {
        Self::new()
    }
}
