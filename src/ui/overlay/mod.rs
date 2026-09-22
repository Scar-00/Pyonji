pub mod palette;
pub mod releases;
pub mod sessions;
pub mod opener;

use gpui_base::StyledExt as _;
use sessions::SessionsView;
use releases::ReleasesView;
use opener::{FileOpener, FileOpenerEvent};

use gpui::*;
use gpui_component::{WindowExt, dialog::Dialog};

use crate::{PushError, PyTheme, Pyonji, util};

#[derive(Debug, Clone, Copy)]
pub enum OverlayScreen {
    Palette,
    Sessions,
    Releases,
    Opener,
}

pub struct Overlay {
    pyonji: WeakEntity<Pyonji>,

    sessions: Entity<SessionsView>,
    releases: Entity<ReleasesView>,
    opener: Entity<FileOpener>,
}

impl Overlay {
    pub fn new(pyonji: WeakEntity<Pyonji>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_focus_lost(window, |this, window, cx| {
            let focus = util::read!(this.pyonji, cx).focus_handle.clone();
            window.focus(&focus, cx);
        })
        .detach();
        Self {
            pyonji: pyonji.clone(),

            sessions: cx.new(|cx| SessionsView::new(&pyonji, window, cx)),
            releases: cx.new(|cx| ReleasesView::new(&pyonji, window, cx)),
            opener: Self::setup_opener(window, cx),
        }
    }
}

impl Overlay {
    pub fn open(&mut self, screen: OverlayScreen, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        let builder = cx.processor(move |this, dialog: Dialog, _window, cx| {
            dialog
                .p_0()
                .h_4_5()
                .backdrop_blur(px(24.0))
                .bg(cx.theme().surface.opacity(0.15))
                .close_button(false)
                .child(
                    div()
                        .size_full()
                        .p_1()
                        .child(match screen {
                            OverlayScreen::Sessions => this.sessions.clone().into_any_element(),
                            OverlayScreen::Releases => this.releases.clone().into_any_element(),
                            OverlayScreen::Opener => this.opener.clone().into_any_element(),
                            _ => div().into_any_element(),
                        }),
            )
        });
        window.open_dialog(cx, builder);
        let handle = match screen {
            OverlayScreen::Sessions => self.sessions.focus_handle(cx),
            OverlayScreen::Releases => self.releases.focus_handle(cx),
            OverlayScreen::Opener => self.opener.focus_handle(cx),
            _ => todo!(),
        };
        window.defer(cx, move |window, cx| {
            window.focus(&handle, cx);
        });
    }

    fn setup_opener(window: &mut Window, cx: &mut Context<Self>) -> Entity<FileOpener> {
        let opener = cx.new(|cx| {
            FileOpener::new(std::env::current_dir().unwrap(), window, cx)
                .show_hidden(true)
        });
        cx.subscribe_in(&opener, window, |this, _, ev: &FileOpenerEvent, window, cx| {
            _ = this.pyonji.update(cx, |this, cx| {
                match ev {
                    FileOpenerEvent::Opened(path) => {
                        let id = match this.create_session(Some(path), None, None, cx) {
                            Err(e) => {
                                window.dispatch_action(Box::new(PushError::new(e)), cx);
                                window.close_dialog(cx);
                                return;
                            }
                            Ok(id) => id,
                        };
                        if path.is_file() && let Some(editor) = this.editor.as_ref() {
                            this.session_manager.send_text(id, &format!("{editor} .\r"));
                        }
                        window.close_dialog(cx);
                    }
                    FileOpenerEvent::Cancelled => {
                        window.close_dialog(cx);
                    }
                }
            });
        })
        .detach();
        opener
    }
}
