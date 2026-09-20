mod palette;
mod releases;
mod sessions;

use gpui_base::StyledExt as _;
use sessions::SessionsView;

use gpui::*;
use gpui_component::{
    WindowExt,
    dialog::{Dialog, DialogContent},
};

use crate::{PyTheme, Pyonji, util};

#[derive(Debug, Clone, Copy)]
pub enum OverlayScreen {
    Palette,
    Sessions,
    Releases,
}

pub struct Overlay {
    pyonji: WeakEntity<Pyonji>,

    sessions: Entity<SessionsView>,
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
        }
    }
}

impl Overlay {
    pub fn open(&mut self, screen: OverlayScreen, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        let builder = cx.processor(move |this, dialog: Dialog, _window, cx| {
            dialog.p_0().h_4_5().close_button(false).child(
                div()
                    .size_full()
                    .bg(cx.theme().surface)
                    .backdrop_blur(px(24.0))
                    .p_1()
                    .child(match screen {
                        OverlayScreen::Sessions => this.sessions.clone().into_any_element(),
                        _ => div().into_any_element(),
                    }),
            )
        });
        window.open_dialog(cx, builder);
        let handle = match screen {
            OverlayScreen::Sessions => self.sessions.focus_handle(cx),
            _ => todo!(),
        };
        window.defer(cx, move |window, cx| {
            window.focus(&handle, cx);
        });
    }
}
