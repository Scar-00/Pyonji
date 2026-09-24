pub mod opener;
pub mod palette;
pub mod releases;
pub mod sessions;

use gpui_shell::{ShellRoot, action::ShellAction};
use opener::{FileOpener, FileOpenerEvent};
use releases::ReleasesView;
use sessions::SessionsView;

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
    js: Entity<ShellRoot>,
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
            js: init_shell(window, cx).unwrap(),
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
                .child(div().size_full().p_1().child(match screen {
                    OverlayScreen::Sessions => this.sessions.clone().into_any_element(),
                    OverlayScreen::Releases => this.releases.clone().into_any_element(),
                    OverlayScreen::Opener => this.opener.clone().into_any_element(),
                    OverlayScreen::Palette => this.js.clone().into_any_element(),
                }))
        });
        window.open_dialog(cx, builder);
        let handle = match screen {
            OverlayScreen::Sessions => self.sessions.focus_handle(cx),
            OverlayScreen::Releases => self.releases.focus_handle(cx),
            OverlayScreen::Opener => self.opener.focus_handle(cx),
            OverlayScreen::Palette => {
                window.dispatch_action(ShellAction::new("focus-in").boxed_clone(), cx);
                return;
            },
        };
        window.defer(cx, move |window, cx| {
            window.focus(&handle, cx);
        });
    }

    fn setup_opener(window: &mut Window, cx: &mut Context<Self>) -> Entity<FileOpener> {
        let opener = cx.new(|cx| {
            FileOpener::new(std::env::current_dir().unwrap(), window, cx).show_hidden(true)
        });
        cx.subscribe_in(
            &opener,
            window,
            |this, _, ev: &FileOpenerEvent, window, cx| {
                _ = this.pyonji.update(cx, |this, cx| match ev {
                    FileOpenerEvent::Opened(path) => {
                        let id = match this.create_session(Some(path), None, None, cx) {
                            Err(e) => {
                                window.dispatch_action(Box::new(PushError::new(e)), cx);
                                window.close_dialog(cx);
                                return;
                            }
                            Ok(id) => id,
                        };
                        if path.is_file()
                            && let Some(editor) = this.editor.as_ref()
                        {
                            this.session_manager.send_text(id, &format!("{editor} .\r"));
                        }
                        window.close_dialog(cx);
                    }
                    FileOpenerEvent::Cancelled => {
                        window.close_dialog(cx);
                    }
                });
            },
        )
        .detach();
        opener
    }
}

pub fn init_shell(window: &mut Window, cx: &mut App) -> Result<Entity<ShellRoot>> {
    use gpui_shell::*;
    gpui_shell::init(cx);
    theme_mod()?;
    dbg!(gpui_shell::write_type_declarations(&std::path::Path::new("./resources"))?);
    let runtime = ShellRuntime::new(cx)?;
    let view = runtime.load("./resources/generated/main.js", window, cx);
    runtime.watch(&view, window, cx)?.forget();
    Ok(view)
}

fn theme_mod() -> anyhow::Result<()> {
    use gpui_shell::{HostError, HostModule, with_current_app};

    gpui_shell::export_module(
        HostModule::new("py-theme")
            .declarations(
                r#"
                export interface PyonjiTheme {
                  readonly background: import("gpui").Color;
                  readonly surface: import("gpui").Color;
                  readonly surface_elevated: import("gpui").Color;
                  readonly text: import("gpui").Color;
                  readonly text_muted: import("gpui").Color;
                  readonly text_disabled: import("gpui").Color;
                  readonly selected: import("gpui").Color;
                  readonly unselected: import("gpui").Color;
                  readonly hovered: import("gpui").Color;
                  readonly selected_border: import("gpui").Color;
                  readonly unselected_border: import("gpui").Color;
                  readonly accent: import("gpui").Color;
                  readonly accent_muted: import("gpui").Color;
                  readonly cursor: import("gpui").Color;
                  readonly cursor_text: import("gpui").Color;
                  readonly selection: import("gpui").Color;
                  readonly search_match: import("gpui").Color;
                  readonly search_match_active: import("gpui").Color;
                  readonly scrollbar: import("gpui").Color;
                  readonly scrollbar_hover: import("gpui").Color;
                  readonly split_divider: import("gpui").Color;
                  readonly split_divider_active: import("gpui").Color;
                  readonly success: import("gpui").Color;
                  readonly warning: import("gpui").Color;
                  readonly error: import("gpui").Color;
                  readonly info: import("gpui").Color;
                  readonly overlay_backdrop: import("gpui").Color;
                  readonly tooltip_background: import("gpui").Color;
                  readonly border: import("gpui").Color;
                  readonly focus_ring: import("gpui").Color;
                }
                export function theme(): PyonjiTheme;
                "#,
            )
            .function("theme", |_| {
                with_current_app(|cx| theme_value(cx.theme()))
                    .ok_or_else(|| HostError::new("theme is unavailable outside a host call"))
            }),
    )?;
    Ok(())
}

fn theme_value(theme: &crate::Theme) -> gpui_shell::HostValue {
    use gpui_shell::HostObject;

    HostObject::new()
        .field("background", theme_color(theme.background))
        .field("surface", theme_color(theme.surface))
        .field("surface_elevated", theme_color(theme.surface_elevated))
        .field("text", theme_color(theme.text))
        .field("text_muted", theme_color(theme.text_muted))
        .field("text_disabled", theme_color(theme.text_disabled))
        .field("selected", theme_color(theme.selected))
        .field("unselected", theme_color(theme.unselected))
        .field("hovered", theme_color(theme.hovered))
        .field("selected_border", theme_color(theme.selected_border))
        .field("unselected_border", theme_color(theme.unselected_border))
        .field("accent", theme_color(theme.accent))
        .field("accent_muted", theme_color(theme.accent_muted))
        .field("cursor", theme_color(theme.cursor))
        .field("cursor_text", theme_color(theme.cursor_text))
        .field("selection", theme_color(theme.selection))
        .field("search_match", theme_color(theme.search_match))
        .field("search_match_active", theme_color(theme.search_match_active))
        .field("scrollbar", theme_color(theme.scrollbar))
        .field("scrollbar_hover", theme_color(theme.scrollbar_hover))
        .field("split_divider", theme_color(theme.split_divider))
        .field("split_divider_active", theme_color(theme.split_divider_active))
        .field("success", theme_color(theme.success))
        .field("warning", theme_color(theme.warning))
        .field("error", theme_color(theme.error))
        .field("info", theme_color(theme.info))
        .field("overlay_backdrop", theme_color(theme.overlay_backdrop))
        .field("tooltip_background", theme_color(theme.tooltip_background))
        .field("border", theme_color(theme.border))
        .field("focus_ring", theme_color(theme.focus_ring))
        .into()
}

fn theme_color(color: Rgba) -> gpui_shell::HostValue {
    use gpui_shell::HostValue;

    let color = color.into_format::<u8, u8>();
    HostValue::from(format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        color.red, color.green, color.blue, color.alpha
    ))
}
