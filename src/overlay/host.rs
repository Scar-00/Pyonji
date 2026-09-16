//! Overlay host: shared dialog chrome and screen routing.
//!
//! Child screens own their content; this module owns open/close routing and the
//! common dialog shell so chrome can change without editing every overlay.

use gpui::{App, Entity, Window, prelude::*};
use gpui_component::{WindowExt as _, dialog::Dialog};

use crate::Surface;

use super::{Screen, detached, opener, palette, releases, sessions};

/// Parent coordinator for all overlay dialogs.
pub struct Overlay;

impl Overlay {
    /// Open the dialog for `screen`, snapshotting data from `surface`.
    ///
    /// Reads snapshots via `cx` (no `&mut Surface` needed) so it can run
    /// from render (which owns a window) and from key handling without
    /// dead-locking on the surface's update lease.
    pub fn open(
        screen: Screen,
        entity: Entity<Surface>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let surface = entity.read(cx);
        match screen {
            Screen::CmdPalette => {
                let commands = surface.status_bar.read(cx).palette_commands().to_vec();
                palette::open(commands, entity.clone(), window, cx);
            }
            Screen::Sessions => {
                let entries =
                    sessions::session_entries(&surface.workspace);
                sessions::open(entries, entity.clone(), window, cx);
            }
            Screen::Detached => {
                let entries =
                    detached::detached_entries(&surface.workspace);
                let current_tab = surface.workspace.current_tab;
                detached::open(entries, current_tab, entity.clone(), window, cx);
            }
            Screen::Releases => {
                let tx = surface.event_tx.clone();
                let host = surface.overlay_host.clone();
                host.update(cx, |host, _| host.ensure_releases_fetch(tx));
                releases::open(entity.clone(), window, cx);
            }
            Screen::Opener => opener::open(entity.clone(), window, cx),
        }
    }

    /// Apply shared dialog chrome and focus-back-to-terminal on close.
    pub fn shell(dialog: Dialog, surface: Entity<Surface>, window: &mut Window) -> Dialog {
        dialog
            .close_button(false)
            .p_0()
            .on_close(window.listener_for(&surface, |surface, _, window, cx| {
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
