pub mod opener;
pub mod palette;
pub mod releases;
pub mod sessions;

//use gpui_shell::{ShellRoot, action::ShellAction};
use opener::{FileOpener, FileOpenerEvent};
use palette::PaletteView;
use releases::ReleasesView;
use sessions::SessionsView;

use gpui::*;
use gpui_component::{WindowExt, dialog::Dialog};

use crate::{PushError, PyTheme, Pyonji, util};
use std::path::{Path, PathBuf};

struct SessionTarget {
    directory: PathBuf,
    command: Option<String>,
}

fn session_target(path: &Path, editor: Option<&str>) -> anyhow::Result<SessionTarget> {
    let metadata = path.metadata()?;
    if metadata.is_dir() {
        return Ok(SessionTarget {
            directory: path.to_path_buf(),
            command: None,
        });
    }
    if !metadata.is_file() {
        anyhow::bail!("selected path is not a file or directory");
    }
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("selected file has no name"))?
        .to_string_lossy();
    let command = editor.map(|editor| {
        // Use a name relative to the session's cwd; the prefix prevents a filename
        // starting with '-' from being interpreted as an editor option.
        #[cfg(windows)]
        let argument = format!("\".\\{name}\"");
        #[cfg(not(windows))]
        let argument = format!("'./{}'", name.replace('\'', "'\\''"));
        format!("{editor} {argument}\r")
    });
    Ok(SessionTarget { directory, command })
}

#[derive(Debug, Clone, Copy)]
pub enum OverlayScreen {
    Palette,
    Sessions,
    Detached,
    Releases,
    Opener,
}

pub struct Overlay {
    pyonji: WeakEntity<Pyonji>,

    palette: Entity<PaletteView>,
    sessions: Entity<SessionsView>,
    detached: Entity<SessionsView>,
    releases: Entity<ReleasesView>,
    opener: Entity<FileOpener>,
    //js: Entity<ShellRoot>,
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

            palette: cx.new(|cx| PaletteView::new(&pyonji, window, cx)),
            sessions: cx.new(|cx| SessionsView::new(&pyonji, false, window, cx)),
            detached: cx.new(|cx| SessionsView::new(&pyonji, true, window, cx)),
            releases: cx.new(|cx| ReleasesView::new(&pyonji, window, cx)),
            opener: Self::setup_opener(window, cx),
            //js: init_shell(window, cx).unwrap(),
        }
    }
}

impl Overlay {
    pub fn open(&mut self, screen: OverlayScreen, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
        }
        match screen {
            OverlayScreen::Sessions => self.sessions.update(cx, |this, cx| this.reset(window, cx)),
            OverlayScreen::Detached => self.detached.update(cx, |this, cx| this.reset(window, cx)),
            _ => {}
        }
        let builder = cx.processor(move |this, dialog: Dialog, window, cx| {
            let (width, height) = Self::dialog_size(screen, window);
            let dialog = dialog
                .p_0()
                .backdrop_blur(px(24.0))
                .bg(cx.theme().surface.opacity(0.15))
                .border_color(cx.theme().border)
                .shadow(crate::ui::surface_shadow())
                .close_button(false);
            let dialog = match (width, height) {
                (Some(width), Some(height)) => dialog.w(width).h(height),
                (Some(width), None) => dialog.w(width).h_4_5(),
                (None, Some(height)) => dialog.h(height),
                (None, None) => dialog.h_4_5(),
            };
            dialog.child(div().size_full().p_1().child(match screen {
                OverlayScreen::Palette => this.palette.clone().into_any_element(),
                OverlayScreen::Sessions => this.sessions.clone().into_any_element(),
                OverlayScreen::Detached => this.detached.clone().into_any_element(),
                OverlayScreen::Releases => this.releases.clone().into_any_element(),
                OverlayScreen::Opener => this.opener.clone().into_any_element(),
            }))
        });
        window.open_dialog(cx, builder);
        let handle = match screen {
            OverlayScreen::Palette => self.palette.focus_handle(cx),
            OverlayScreen::Sessions => self.sessions.focus_handle(cx),
            OverlayScreen::Detached => self.detached.focus_handle(cx),
            OverlayScreen::Releases => self.releases.focus_handle(cx),
            OverlayScreen::Opener => self.opener.focus_handle(cx),
        };
        window.defer(cx, move |window, cx| {
            window.focus(&handle, cx);
        });
    }

    fn dialog_size(screen: OverlayScreen, window: &Window) -> (Option<Pixels>, Option<Pixels>) {
        let (width, height) = match screen {
            OverlayScreen::Palette => (px(620.), px(520.)),
            OverlayScreen::Sessions | OverlayScreen::Detached => (px(660.), px(520.)),
            OverlayScreen::Releases => (px(920.), px(500.)),
            OverlayScreen::Opener => (px(760.), px(560.)),
        };
        let size = window.viewport_size();
        let inset = px(64.);
        (
            Some(width.min((size.width - inset.min(size.width * 0.1)).max(px(0.)))),
            Some(height.min((size.height - inset.min(size.height * 0.1)).max(px(0.)))),
        )
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
                        let target = match session_target(path, this.editor.as_deref()) {
                            Ok(target) => target,
                            Err(error) => {
                                window.dispatch_action(Box::new(PushError::new(error)), cx);
                                return;
                            }
                        };
                        let id = match this.create_session(Some(&target.directory), None, None, cx)
                        {
                            Err(e) => {
                                window.dispatch_action(Box::new(PushError::new(e)), cx);
                                window.close_dialog(cx);
                                return;
                            }
                            Ok(id) => id,
                        };
                        if let Some(command) = target.command {
                            this.session_manager.send_text(id, &command);
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

#[cfg(test)]
mod tests {
    use super::session_target;

    #[test]
    fn a_file_opens_in_its_parent_and_is_passed_to_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("-my notes.txt");
        std::fs::write(&file, "").unwrap();
        let target = session_target(&file, Some("nvim")).unwrap();
        assert_eq!(target.directory, dir.path());
        #[cfg(windows)]
        assert_eq!(
            target.command.as_deref(),
            Some("nvim \".\\-my notes.txt\"\r")
        );
        #[cfg(not(windows))]
        assert_eq!(target.command.as_deref(), Some("nvim './-my notes.txt'\r"));
        assert!(session_target(&file, None).unwrap().command.is_none());
    }

    #[test]
    fn a_directory_opens_itself_without_starting_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let target = session_target(dir.path(), Some("nvim")).unwrap();
        assert_eq!(target.directory, dir.path());
        assert!(target.command.is_none());
        assert!(session_target(&dir.path().join("missing"), Some("nvim")).is_err());
    }
}
/*
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
*/
