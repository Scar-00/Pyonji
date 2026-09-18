mod detached;
mod host;
mod opener;
mod palette;
mod releases;
mod search;
mod sessions;

pub(crate) use detached::DetachedView;
pub use host::Overlay;
pub use opener::OpenerView;
pub use palette::{PaletteView, complete_command_name, filter_commands};
pub use releases::{ReleasesView, fetch_releases_async};
pub use search::{SearchDialog, SearchDialogEvent, SearchItem};
pub(crate) use sessions::SessionsView;

use std::rc::Rc;

use gpui::{App, Entity, Window};

use crate::{ResultExt, Surface, config};

/// Which overlay dialog to show.
///
/// Kept from the ratatui implementation so Lua (`py:open_*`), the status
/// `:sessions`-style commands, and keybindings keep working unchanged.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum Screen {
    CmdPalette,
    Sessions,
    Detached,
    Releases,
    Opener,
}

#[derive(Clone)]
pub struct Arg {
    pub placeholder: String,
}

impl Arg {
    pub fn new(n: impl ToString) -> Self {
        Self {
            placeholder: n.to_string(),
        }
    }
}

/// A palette command. Faithful to the ratatui version: a display name, the
/// `<arg>` placeholders shown next to it, and the action to run with the
/// trailing query words as `args`.
#[derive(Clone)]
pub struct Cmd {
    pub name: String,
    pub args: Vec<Arg>,
    pub action: Rc<dyn Fn(Entity<Surface>, &mut Window, &mut App, Vec<String>)>,
}

impl Cmd {
    pub fn new(
        name: impl ToString,
        args: impl IntoIterator<Item = Arg>,
        f: impl 'static + Fn(Entity<Surface>, &mut Window, &mut App, &[String]),
    ) -> Self {
        let name = name.to_string();
        Self {
            name,
            args: args.into_iter().collect(),
            action: Rc::new(move |surface, window, cx, args| {
                f(surface, window, cx, &args);
            }),
        }
    }

    /// `"<name> <arg…>"` hint, e.g. `switch <tab>`.
    pub fn hint(&self) -> String {
        let mut hint = self.name.clone();
        for arg in &self.args {
            hint.push_str(&format!(" <{}>", arg.placeholder));
        }
        hint
    }
}

impl PartialEq for Cmd {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
impl Eq for Cmd {}

/// All commands currently available: builtins, one `ssh: <name>` per
/// configured SSH session, and every Lua-registered action.
///
/// Takes explicit slices so callers (Workspace, StatusBar) don't need to
/// hand over the whole `Surface`.
pub fn commands_for(
    ssh_sessions: &[crate::pty::SshConnection],
    registered_callbacks: &[LuaAction],
) -> Vec<Cmd> {
    let mut commands = builtin_commands();
    commands.extend(commands_from_ssh_sessions(ssh_sessions));
    commands.extend(commands_from_lua_actions(registered_callbacks));
    commands
}

/// Build command list from parts owned by StatusBar and Workspace.
pub fn commands_for_surface(
    _status: &crate::status::StatusBar,
    registered_callbacks: &[LuaAction],
    _lua: &mlua::Lua,
    ssh_sessions: &[crate::pty::SshConnection],
) -> Vec<Cmd> {
    let mut commands = builtin_commands();
    commands.extend(commands_from_ssh_sessions(ssh_sessions));
    commands.extend(commands_from_lua_actions(registered_callbacks));
    commands
}

/// Run a `:command args…` line from the status prompt. Returns `false` when
/// no command matches `name`, so the caller can report `unknown command`.
pub fn execute_command(
    commands: &[Cmd],
    entity: Entity<Surface>,
    window: &mut Window,
    cx: &mut App,
    input: &str,
) -> bool {
    let mut split = input.split(' ');
    let Some(name) = split.next() else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    let args = split.map(str::to_string).collect::<Vec<_>>();
    let action = commands
        .iter()
        .find_map(|cmd| (cmd.name == name).then(|| cmd.action.clone()));
    let Some(action) = action else {
        return false;
    };
    window.defer(cx, move |window, cx| {
        action(entity, window, cx, args);
    });
    true
}

fn builtin_commands() -> Vec<Cmd> {
    vec![
        Cmd::new("close", [Arg::new("tab")], |surface, _, cx, args| {
            surface.update(cx, |surface, cx| {
                let Some(tab) = args.first() else {
                    if let Some(session) = surface.workspace.active_session() {
                        surface.close_session(session);
                    }
                    cx.notify();
                    return;
                };
                let Ok(tab) = tab.parse::<usize>() else {
                    return;
                };
                if tab == 0 || tab > 9 {
                    return;
                }
                let Some(tab) = surface
                    .workspace
                    .tabs
                    .get(tab - 1)
                    .and_then(Option::as_ref)
                else {
                    return;
                };
                let Some(session) = tab.sessions().first().copied() else {
                    return;
                };
                surface.close_session(session);
                cx.notify();
            });
        }),
        Cmd::new("next", [], |surface, _, cx, _| {
            surface.update(cx, |surface, cx| {
                let index = surface.workspace.next_tab_index();
                surface.switch_tab(index);
                cx.notify();
            });
        }),
        Cmd::new("prev", [], |surface, _, cx, _| {
            surface.update(cx, |surface, cx| {
                let index = surface.workspace.previous_tab_index();
                surface.switch_tab(index);
                cx.notify();
            });
        }),
        Cmd::new("switch", [Arg::new("tab")], |surface, _, cx, args| {
            surface.update(cx, |surface, cx| {
                let Some(tab) = args.first() else {
                    return;
                };
                let Ok(tab) = tab.parse::<usize>() else {
                    return;
                };
                if tab > 9 || tab == 0 {
                    return;
                }
                surface.switch_tab(tab - 1);
                cx.notify();
            });
        }),
        Cmd::new("sessions", [], |surface, window, cx, _| {
            let overlay = surface.read(cx).overlay.clone();
            overlay.update(cx, |overlay, cx| {
                overlay.open(surface.clone(), Screen::Sessions, window, cx);
            });
        }),
        Cmd::new("detach", [], |surface, _, cx, _| {
            surface.update(cx, |surface, cx| {
                surface.detach_active_session();
                cx.notify();
            });
        }),
        Cmd::new("rename", [Arg::new("name")], |surface, _, cx, args| {
            surface.update(cx, |surface, cx| {
                let name = args.join(" ");
                if name.is_empty() {
                    return;
                }
                if let Some(session) = surface.workspace.active_session()
                    && let Some(session) = surface
                        .workspace
                        .session_manager
                        .session_mut(session)
                {
                    session.rename(name);
                }
                cx.notify();
            });
        }),
        Cmd::new("move-to", [Arg::new("tab")], |surface, _, cx, args| {
            surface.update(cx, |surface, cx| {
                let Some(tab) = args.first() else {
                    return;
                };
                let Ok(tab) = tab.parse::<usize>() else {
                    return;
                };
                if tab == 0 || tab > 9 {
                    return;
                }
                if let Some(session) = surface.workspace.active_session() {
                    surface.move_session_to_tab(session, tab - 1);
                }
                cx.notify();
            });
        }),
        Cmd::new("attach", [], |surface, window, cx, _| {
            let overlay = surface.read(cx).overlay.clone();
            overlay.update(cx, |overlay, cx| {
                overlay.open(surface.clone(), Screen::Detached, window, cx);
            });
        }),
        Cmd::new("releases", [], |surface, window, cx, _| {
            let overlay = surface.read(cx).overlay.clone();
            overlay.update(cx, |overlay, cx| {
                overlay.open(surface.clone(), Screen::Releases, window, cx);
            });
        }),
        Cmd::new("ssh", [Arg::new("session")], |surface, _, cx, args| {
            surface.update(cx, |surface, cx| {
                let Some(name) = args.first() else {
                    return;
                };
                let Some(connection) = surface
                    .workspace
                    .ssh_sessions
                    .iter()
                    .find(|s| s.name == *name)
                    .cloned()
                else {
                    return;
                };
                surface.create_remote_session(&connection);
                cx.notify();
            });
        }),
        Cmd::new("reload-config", [], |surface, _, cx, _| {
            surface.update(cx, |surface, _cx| {
                config::load(surface);
            });
        }),
        Cmd::new("open-in", [], |surface, window, cx, _| {
            let overlay = surface.read(cx).overlay.clone();
            overlay.update(cx, |overlay, cx| {
                overlay.open(surface.clone(), Screen::Opener, window, cx);
            });
        }),
    ]
}

fn commands_from_ssh_sessions(sessions: &[crate::pty::SshConnection]) -> Vec<Cmd> {
    sessions
        .iter()
        .map(|session| {
            let session = session.clone();
            let name = format!("ssh: {}", session.name);
            Cmd::new(name, [], move |surface, _, cx, _| {
                surface.update(cx, |surface, cx| {
                    surface.create_remote_session(&session);
                    cx.notify();
                });
            })
        })
        .collect()
}

fn commands_from_lua_actions(actions: &[LuaAction]) -> Vec<Cmd> {
    use mlua::prelude::*;
    actions
        .iter()
        .map(|action| {
            let mut args = action.args.iter().map(Arg::new).collect::<Vec<_>>();
            if action.is_var_arg {
                args.push(Arg::new("..."));
            }
            let func = action.callback.clone();
            Cmd::new(action.name.clone(), args, move |surface, _, cx, args| {
                surface.update(cx, |surface, cx| {
                    let args = args
                        .iter()
                        .cloned()
                        .map(|arg| arg.into_lua(&surface.lua))
                        .collect::<LuaResult<Vec<_>>>();
                    let args = match args {
                        Ok(args) => args,
                        Err(error) => {
                            tracing::error!(%error, "failed to convert palette args to lua");
                            return;
                        }
                    };
                    config::with_env(surface, |_| {
                        func.call::<LuaValue>(LuaMultiValue::from_vec(args))
                    })
                    .into_log();
                    cx.notify();
                });
            })
        })
        .collect()
}

pub struct LuaAction {
    pub args: Vec<String>,
    pub is_var_arg: bool,
    pub name: String,
    pub callback: mlua::Function,
}

impl Clone for LuaAction {
    fn clone(&self) -> Self {
        Self {
            args: self.args.clone(),
            is_var_arg: self.is_var_arg,
            name: self.name.clone(),
            callback: self.callback.clone(),
        }
    }
}
