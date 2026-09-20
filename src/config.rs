use crate::Pyonji;
use crate::pty::{Event as PtyEvent, SshConnection};
use crate::terminal::{SessionId, SplitDirection, Tab as TerminalTab};
use crate::util::UnsafeRefMut;
use anyhow::{Context as _, Result};
use async_channel::Sender;
use gpui::{App, Context, Entity, EntityId, KeyDownEvent, Modifiers, WeakEntity};
use mlua::{FromLua, prelude::*};
use notify::RecursiveMode;
use path_absolutize::*;
use std::fmt::Debug;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;
use std::time::Duration;

const DEFAULT_CONFIG: &str = include_str!("../resources/default.lua");
pub const LUA_MODULES: &[(&str, &str)] =
    &[("lua.keybind", include_str!("../resources/lua/keybind.lua"))];

#[allow(unused_macros)]
macro_rules! apply {
    ($this: ident.$field: ident, $table: expr) => {
        if let Ok(value) = $table.get(stringify!($field)).inspect_err(|e| tracing::error!(%e, "failed to get field `{}`", stringify!($field))) {
            $this.$field = value;
        }
    };
    ($this: ident.$field: ident, $table: expr, $transformer: expr) => {
        if let Ok(value) = $table.get(stringify!($field)).map($transformer).inspect_err(|e| tracing::error!(%e, "failed to get field `{}`", stringify!($field))) {
            $this.$field = value;
        }
    };
}

macro_rules! callable_action {
    ($lua: ident, $this: ident => $body: expr) => {{
        if let Some(this) = $this {
            let value = this.borrow_mut_scoped($body)??;
            Ok(value
                .into_lua_multi($lua)?
                .into_iter()
                .next()
                .unwrap_or(LuaValue::Nil))
        } else {
            let func = $lua
                .create_function(move |_, this: LuaAnyUserData| this.borrow_mut_scoped($body)?)?;
            Ok(LuaValue::Function(func))
        }
    }};
}

macro_rules! args {
    ($args: ident, $lua: ident, $typ: ty) => {{
        let mut args = $args;
        let first = args.pop_front();
        let this = match first {
            Some(first) if first.is_userdata() => Some(LuaAnyUserData::from_lua(first, $lua)?),
            Some(first) => {
                args.push_front(first);
                None
            }
            None => None,
        };
        let rest: $typ = FromLuaMulti::from_lua_multi(args, $lua)?;
        (this, rest)
    }};
}

/*impl Surface {
    pub fn apply_config(&mut self) {
        // Font metrics live in the terminal component; the renderer picks
        // them up here. Rows/cols are driven by GPUI layout (`sync_surface`),
        // palette/tabs sync to the bar every frame in `render`.
        self.terminal.apply_font_metrics();
    }
}#*/

pub struct ProxyContext<T> {
    app: UnsafeRefMut<App>,
    entity_state: WeakEntity<T>,
}

impl<T: 'static> ProxyContext<T> {
    fn new(cx: &mut Context<T>) -> Self {
        Self {
            app: UnsafeRefMut::new(cx),
            entity_state: cx.weak_entity(),
        }
    }

    fn entity_id(&self) -> EntityId {
        self.entity_state.entity_id()
    }

    /// Returns a handle to the entity belonging to this context.
    fn entity(&self) -> Entity<T> {
        self.weak_entity()
            .upgrade()
            .expect("The entity must be alive if we have a entity context")
    }

    /// Returns a weak handle to the entity belonging to this context.
    fn weak_entity(&self) -> WeakEntity<T> {
        self.entity_state.clone()
    }

    fn to_ctx(&mut self) -> Context<'_, T> {
        Context::new_context(&mut self.app, self.entity_state.clone())
    }
}

struct LuaProxy {
    py: UnsafeRefMut<Pyonji>,
    cx: ProxyContext<Pyonji>,
}

impl LuaUserData for LuaProxy {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("config", |lua, this, table: LuaTable| {
            let font_size: Option<f32> = config_value(lua, &table, "font_size");
            if font_size.is_some_and(|size| size <= 0.0) {
                tracing::error!("invalid `font_size` (must be positive), ignoring");
            }
            let line_height: Option<f32> = config_value(lua, &table, "line_height");
            if line_height.is_some_and(|height| height <= 0.0) {
                tracing::error!("invalid `line_height` (must be positive), ignoring");
            }
            let font_family: Option<Option<String>> =
                config_option_value(lua, &table, "font_family");
            let default_cwd: Option<Option<String>> =
                config_option_value(lua, &table, "default_cwd");
            let ssh_sessions = config_ssh_sessions(lua, &table);

            if let Some(size) = font_size.filter(|size| *size > 0.0) {
                this.py.font_size = size;
            }
            if let Some(height) = line_height.filter(|height| *height > 0.0) {
                this.py.line_height = height;
            }
            if let Some(family) = font_family {
                this.py.font_family = family;
            }
            if let Some(cwd) = default_cwd {
                this.py.default_cwd = cwd.map(PathBuf::from);
            }
            if let Some(sessions) = ssh_sessions {
                this.py.ssh_sessions = sessions;
            }
            Ok(())
        });
        methods.add_function("create_session", |lua, args: LuaMultiValue| {
            let (this, (dir, tab, direction, parent)) = args!(
                args,
                lua,
                (
                    Option<String>,
                    Option<usize>,
                    Option<String>,
                    Option<SessionId>
                )
            );
            let direction = match direction.as_deref().map(str::to_lowercase).as_deref() {
                Some("horizontal") | Some("h") => SplitDirection::Horizontal,
                _ => SplitDirection::Vertical,
            };
            callable_action!(lua, this => {
                let dir = dir.clone();
                move |this: &mut Self| -> LuaResult<SessionId> {
                    let slot = match tab.or(this.py.current_tab) {
                        Some(slot) if slot < this.py.tabs.len() => slot,
                        _ => return Err(mlua::Error::external("tab index out of range")),
                    };
                    let cwd = this.py.default_cwd.clone();
                    let owned_dir = dir.clone().map(PathBuf::from);
                    let path = owned_dir.as_deref().or(cwd.as_deref());
                    let id = this
                        .py
                        .session_manager
                        .create_session(20, 80, path)
                        .map_err(mlua::Error::external)?;
                    if this.py.tabs[slot].is_none() {
                        this.py.tabs[slot] = Some(TerminalTab::new(id));
                    } else {
                        let placed = this.py.tabs[slot]
                            .as_mut()
                            .map(|tab| match parent.filter(|id| tab.sessions().contains(id)) {
                                Some(at) => tab.split_on(at, direction, id),
                                None => tab.split_active(direction, id),
                            })
                            .unwrap_or(false);
                        if !placed {
                            this.py.session_manager.remove_session(id);
                            return Err(mlua::Error::external("failed to place session"));
                        }
                    }
                    Ok(id)
                }
            })
        });
        methods.add_function("close", |lua, args: LuaMultiValue| {
            let (this, (session,)) = args!(args, lua, (Option<SessionId>,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                let Some(id) = session.or_else(|| this.py.active_session()) else {
                    return Ok(false);
                };
                if this.py.session_manager.session(id).is_none() {
                    return Ok(false);
                }
                if let Some(term_session) = this.py.session_manager.session_mut(id) {
                    term_session.pty.kill();
                }
                remove_session_from_tabs(&mut this.py, id);
                this.py.session_manager.remove_session(id);
                this.py.detached_sessions.retain(|detached| *detached != id);
                Ok(true)
            })
        });
        methods.add_function("switch_tab", |lua, args: LuaMultiValue| {
            let (this, (tab,)) = args!(args, lua, (usize,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                let py = &mut this.py;
                if tab >= py.tabs.len() || py.tabs[tab].is_none() {
                    return Ok(false);
                }
                py.current_tab = Some(tab);
                py.wheel_remainder = 0.0;
                Ok(true)
            })
        });
        methods.add_function("next_tab", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<usize> {
                let py = &mut this.py;
                let live: Vec<usize> = (0..py.tabs.len())
                    .filter(|&index| py.tabs[index].is_some())
                    .collect();
                let next = match live.as_slice() {
                    [] => py.current_tab.unwrap_or(0),
                    live => match py
                        .current_tab
                        .and_then(|current| live.iter().position(|&index| index == current))
                    {
                        Some(pos) => live[(pos + 1) % live.len()],
                        None => live[0],
                    },
                };
                if !live.is_empty() {
                    py.current_tab = Some(next);
                    py.wheel_remainder = 0.0;
                }
                Ok(next)
            })
        });
        methods.add_function("prev_tab", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<usize> {
                let py = &mut this.py;
                let live: Vec<usize> = (0..py.tabs.len())
                    .filter(|&index| py.tabs[index].is_some())
                    .collect();
                let prev = match live.as_slice() {
                    [] => py.current_tab.unwrap_or(0),
                    live => match py
                        .current_tab
                        .and_then(|current| live.iter().position(|&index| index == current))
                    {
                        Some(pos) => live[(pos + live.len() - 1) % live.len()],
                        None => live[live.len() - 1],
                    },
                };
                if !live.is_empty() {
                    py.current_tab = Some(prev);
                    py.wheel_remainder = 0.0;
                }
                Ok(prev)
            })
        });
        methods.add_function("move_to", |lua, args: LuaMultiValue| {
            let (this, (tab,)) = args!(args, lua, (Option<usize>,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                let Some(target) = tab else {
                    return Ok(false);
                };
                if target >= this.py.tabs.len() {
                    return Ok(false);
                }
                let Some(session) = this.py.active_session() else {
                    return Ok(false);
                };
                let py = &mut this.py;
                remove_session_from_tabs(py, session);
                if py.tabs[target].is_none() {
                    py.tabs[target] = Some(TerminalTab::new(session));
                } else if let Some(tab) = py.tabs[target].as_mut() {
                    tab.split_active(SplitDirection::Vertical, session);
                }
                py.current_tab = Some(target);
                py.wheel_remainder = 0.0;
                Ok(true)
            })
        });
        methods.add_function("split", |lua, args: LuaMultiValue| {
            let (this, direction) = args!(args, lua, String);
            let direction = match direction.to_lowercase().as_str() {
                "horizontal" | "h" => SplitDirection::Horizontal,
                _ => SplitDirection::Vertical,
            };
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<Option<SessionId>> {
                let Some(current) = this.py.current_tab else {
                    return Ok(None);
                };
                if this.py.active_session().is_none() {
                    return Ok(None);
                }
                let cwd = this.py.default_cwd.clone();
                let id = match this.py.session_manager.create_session(20, 80, cwd.as_deref()) {
                    Ok(id) => id,
                    Err(_) => return Ok(None),
                };
                let placed = this.py.tabs[current]
                    .as_mut()
                    .map(|tab| tab.split_active(direction, id))
                    .unwrap_or(false);
                if !placed {
                    this.py.session_manager.remove_session(id);
                    return Ok(None);
                }
                this.py.resize_tab(current, &mut this.cx.to_ctx());
                Ok(Some(id))
            })
        });
        methods.add_function("focus_next_pane", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<Option<SessionId>> {
                let Some(current) = this.py.current_tab else {
                    return Ok(None);
                };
                Ok(this.py.tabs[current].as_mut().and_then(|tab| tab.focus_next()))
            })
        });
        methods.add_function("detach", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<bool> {
                let Some(session) = this.py.active_session() else {
                    return Ok(false);
                };
                if !this.py.detached_sessions.contains(&session) {
                    this.py.detached_sessions.push(session);
                }
                remove_session_from_tabs(&mut this.py, session);
                Ok(true)
            })
        });
        methods.add_function("attach", |lua, args: LuaMultiValue| {
            let (this, (session, tab)) = args!(args, lua, (SessionId, Option<usize>));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                if this.py.session_manager.session(session).is_none() {
                    return Ok(false);
                }
                let Some(target) = tab.or(this.py.current_tab) else {
                    return Ok(false);
                };
                if target >= this.py.tabs.len() {
                    return Ok(false);
                }
                let py = &mut this.py;
                py.detached_sessions.retain(|detached| *detached != session);
                remove_session_from_tabs(py, session);
                if py.tabs[target].is_none() {
                    py.tabs[target] = Some(TerminalTab::new(session));
                } else if let Some(tab) = py.tabs[target].as_mut() {
                    tab.split_active(SplitDirection::Vertical, session);
                }
                py.current_tab = Some(target);
                py.wheel_remainder = 0.0;
                Ok(true)
            })
        });
        methods.add_function("rename", |lua, args: LuaMultiValue| {
            let (this, (rest,)) = args!(args, lua, (LuaMultiValue,));
            let (session, name) = {
                let with_session: LuaResult<(SessionId, String)> =
                    FromLuaMulti::from_lua_multi(rest.clone(), lua);
                match with_session {
                    Ok((session, name)) => (Some(session), name),
                    Err(_) => {
                        let (name,) = FromLuaMulti::from_lua_multi(rest, lua)?;
                        (None, name)
                    }
                }
            };
            callable_action!(lua, this => {
                let name = name.clone();
                move |this: &mut Self| -> LuaResult<bool> {
                    let Some(session) = session.or_else(|| this.py.active_session()) else {
                        return Ok(false);
                    };
                    let Some(term_session) = this.py.session_manager.session_mut(session) else {
                        return Ok(false);
                    };
                    term_session.rename(name.clone());
                    Ok(true)
                }
            })
        });
    }

    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("font_size", |_, this| Ok(this.py.font_size));
        fields.add_field_method_get("line_height", |_, this| Ok(this.py.line_height));
        fields.add_field_method_get("font_family", |_, this| Ok(this.py.font_family.clone()));
        fields.add_field_method_get("default_cwd", |_, this| {
            Ok(this
                .py
                .default_cwd
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()))
        });
        fields.add_field_method_get("current_tab", |_, this| Ok(this.py.current_tab));
        fields.add_field_method_get("active_session", |_, this| Ok(this.py.active_session()));
        fields.add_field_method_get("tab_count", |_, this| {
            Ok(this.py.tabs.iter().flatten().count())
        });
        fields.add_field_method_get("detached_sessions", |lua, this| {
            lua.create_sequence_from(this.py.detached_sessions.clone())
        });
        fields.add_field_method_get("ssh_sessions", |lua, this| {
            let table = lua.create_table()?;
            for (index, session) in this.py.ssh_sessions.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", session.name.clone())?;
                entry.set("user_name", session.user_name.clone())?;
                entry.set("ip", session.ip.to_string())?;
                table.raw_set(index + 1, entry)?;
            }
            Ok(table)
        });
        fields.add_field_method_get("sessions", |lua, this| {
            let outer = lua.create_table()?;
            for (index, sessions) in this
                .py
                .tabs
                .iter()
                .filter_map(|tab| tab.as_ref().map(|tab| tab.sessions()))
                .enumerate()
            {
                outer.raw_set(index + 1, lua.create_sequence_from(sessions)?)?;
            }
            Ok(outer)
        });
    }
}

fn remove_session_from_tabs(py: &mut Pyonji, session: SessionId) {
    let current = py.current_tab;
    let mut emptied_current = false;
    for (index, tab) in py.tabs.iter_mut().enumerate() {
        let Some(tab_state) = tab.as_mut() else {
            continue;
        };
        if !tab_state.remove_session(session) {
            continue;
        }
        if tab_state.is_empty() {
            *tab = None;
            emptied_current |= Some(index) == current;
        }
    }
    if !emptied_current {
        return;
    }
    let Some(closed) = current else {
        return;
    };
    if let Some(tab) = (0..py.tabs.len())
        .map(|offset| (closed + py.tabs.len() - 1 - offset) % py.tabs.len())
        .find(|&tab| py.tabs[tab].is_some())
    {
        py.current_tab = Some(tab);
        py.wheel_remainder = 0.0;
        return;
    }
    py.current_tab = Some(closed.min(py.tabs.len().saturating_sub(1)));
}

fn config_value<T: FromLua>(lua: &Lua, table: &LuaTable, key: &str) -> Option<T> {
    if !table.contains_key(key).unwrap_or(false) {
        return None;
    }
    let value: LuaValue = table.get(key).unwrap_or(LuaValue::Nil);
    resolve_value(lua, value, key)
}

fn config_option_value<T: FromLua>(lua: &Lua, table: &LuaTable, key: &str) -> Option<Option<T>> {
    if !table.contains_key(key).unwrap_or(false) {
        return None;
    }
    let value: LuaValue = table.get(key).unwrap_or(LuaValue::Nil);
    match value {
        LuaValue::Nil => Some(None),
        LuaValue::Function(func) => match func.call::<LuaValue>(()) {
            Ok(LuaValue::Nil) => Some(None),
            Ok(value) => match T::from_lua(value, lua) {
                Ok(resolved) => Some(Some(resolved)),
                Err(error) => {
                    tracing::error!(%error, key, "invalid config value");
                    None
                }
            },
            Err(error) => {
                tracing::error!(%error, key, "config value function failed");
                None
            }
        },
        value => match T::from_lua(value, lua) {
            Ok(resolved) => Some(Some(resolved)),
            Err(error) => {
                tracing::error!(%error, key, "invalid config value");
                None
            }
        },
    }
}

fn resolve_value<T: FromLua>(lua: &Lua, value: LuaValue, key: &str) -> Option<T> {
    match value {
        LuaValue::Nil => None,
        LuaValue::Function(func) => match func.call::<LuaValue>(()) {
            Ok(LuaValue::Nil) => None,
            Ok(value) => match T::from_lua(value, lua) {
                Ok(resolved) => Some(resolved),
                Err(error) => {
                    tracing::error!(%error, key, "invalid config value");
                    None
                }
            },
            Err(error) => {
                tracing::error!(%error, key, "config value function failed");
                None
            }
        },
        value => match T::from_lua(value, lua) {
            Ok(resolved) => Some(resolved),
            Err(error) => {
                tracing::error!(%error, key, "invalid config value");
                None
            }
        },
    }
}

fn config_ssh_sessions(lua: &Lua, table: &LuaTable) -> Option<Vec<SshConnection>> {
    if !table.contains_key("ssh_sessions").unwrap_or(false) {
        return None;
    }
    let value: LuaValue = table.get("ssh_sessions").unwrap_or(LuaValue::Nil);
    let value = match value {
        LuaValue::Nil => return Some(Vec::new()),
        LuaValue::Function(func) => match func.call::<LuaValue>(()) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "config value function failed for `ssh_sessions`");
                return None;
            }
        },
        value => value,
    };
    let entries = match Vec::<LuaValue>::from_lua(value, lua) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::error!(%error, "invalid `ssh_sessions` (expected a list of tables)");
            return None;
        }
    };
    let mut sessions = Vec::with_capacity(entries.len());
    for (index, entry) in entries.into_iter().enumerate() {
        let entry = match entry {
            LuaValue::Function(func) => match func.call::<LuaValue>(()) {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!(%error, index, "ssh session function failed, skipping entry");
                    continue;
                }
            },
            entry => entry,
        };
        let LuaValue::Table(entry) = entry else {
            tracing::error!(index, "ssh session entry is not a table, skipping");
            continue;
        };
        let name: Option<String> = entry
            .get::<LuaValue>("name")
            .ok()
            .and_then(|value| resolve_value(lua, value, "ssh_sessions[].name"));
        let Some(name) = name else {
            tracing::error!(index, "ssh session entry misses `name`, skipping");
            continue;
        };
        let user_name: Option<String> = entry
            .get::<LuaValue>("user_name")
            .ok()
            .and_then(|value| resolve_value(lua, value, "ssh_sessions[].user_name"));
        let ip: Option<String> = entry
            .get::<LuaValue>("ip")
            .ok()
            .and_then(|value| resolve_value(lua, value, "ssh_sessions[].ip"));
        let Some(ip) = ip else {
            tracing::error!(index, "ssh session entry misses `ip`, skipping");
            continue;
        };
        let Ok(ip) = IpAddr::from_str(&ip) else {
            tracing::error!(index, ip, "ssh session entry has an invalid `ip`, skipping");
            continue;
        };
        sessions.push(SshConnection {
            user_name: user_name.unwrap_or_else(|| name.clone()),
            name,
            ip,
        });
    }
    Some(sessions)
}
/*
impl LuaUserData for Pyonji {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        /*methods.add_method_mut("bind", |lua, this, args: LuaMultiValue| {
            if args.front().is_some_and(|arg| arg.is_table()) {
                let (mods, key, func): (Vec<String>, String, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                let mut state = Modifiers::default();
                for modifier in &mods {
                    state |= KeyBinding::parse_mod(modifier)?;
                }
                let key = KeyBinding::parse_key(&key)?;
                this.input.keymap.insert(
                    KeyBinding { modifiers: state, key },
                    KeyAction::Custom(func.clone()),
                );
            } else {
                let (binding, func): (KeyBinding, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                this.input
                    .keymap
                    .insert(binding, KeyAction::Custom(func));
            }

            Ok(())
        });*/
        methods.add_method_mut(
            "register",
            |lua, this, (name, func): (String, LuaFunction)| {
                let (args, is_var_arg) = lua
                    .globals()
                    .get::<LuaFunction>("__HOST_INSPECT_FUNC")
                    .and_then(|f| f.call::<(LuaTable, bool)>(func.clone()))
                    .and_then(|(names, var_arg)| {
                        names
                            .sequence_values::<String>()
                            .collect::<LuaResult<Vec<_>>>()
                            .map(|names| (names, var_arg))
                    })?;
                // Source of truth lives on Surface for sync Lua access;
                // `render` syncs the copy into the StatusBar component.
                this.registered_callbacks.push(LuaAction {
                    args,
                    is_var_arg,
                    name,
                    callback: func,
                });
                Ok(())
            },
        );
        methods.add_method_mut("config", |lua, this, table: LuaTable| {
            if let Ok(font_size) = table.get::<f32>("font_size") {
                this.font_size = font_size;
            }
            if let Ok(line_height) = table.get::<f32>("line_height") {
                this.line_height = line_height;
            }
            if let Ok(font_family) = table.get::<Option<String>>("font_family") {
                this.font_family = font_family;
            }
            /*if let Ok(fullscreen) = table.get::<bool>("fullscreen") {
                this.fullscreen = fullscreen;
            }*/
            if let Ok(default_cwd) = table.get::<Option<PathBuf>>("default_cwd") {
                this.default_cwd = default_cwd;
            }
            /*if let Ok(action) = table.get::<KeyBinding>("action") {
                this.input.action = action;
            }*/
            /*if let Ok(status_height) = table.get::<f32>("status_height") {
                // Owned synchronously by the terminal viewport (like the
                // font metrics); `render` syncs a copy into the bar.
                this.terminal.status_height = status_height.max(0.5);
            }*/
            this.ssh_sessions = util::collect_ssh_sessions(lua, &table);

            Ok(())
        });
        methods.add_function("open_palette", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                Ok(())
            })
        });
        methods.add_function("open_sessions", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                Ok(())
            })
        });
        methods.add_function("open_detached", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                Ok(())
            })
        });
        methods.add_function("detach", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<bool> {
                Ok(this.detach_active_session())
            })
        });
        methods.add_function("attach", |lua, args: LuaMultiValue| {
            let (this, (session, tab)) = args!(args, lua, (SessionId, Option<usize>));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                Ok(this.reattach_session(session, tab.unwrap_or(this.workspace.current_tab)))
            })
        });
        methods.add_function("open_rename", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let name = this
                    .workspace
                    .active_session()
                    .and_then(|id| this.workspace.session_manager.session(id))
                    .map(|s| s.title().to_owned())
                    .unwrap_or_default();
                this.queue_action(OpenRenamePrompt { name });
                Ok(())
            })
        });
        methods.add_function("rename", |lua, args: LuaMultiValue| {
            let (this, (rest,)) = args!(args, lua, (LuaMultiValue,));
            let (session, name) = {
                let with_session: LuaResult<(SessionId, String)> =
                    FromLuaMulti::from_lua_multi(rest.clone(), lua);
                match with_session {
                    Ok((session, name)) => (Some(session), name),
                    Err(_) => {
                        let (name,) = FromLuaMulti::from_lua_multi(rest, lua)?;
                        (None, name)
                    }
                }
            };
            callable_action!(lua, this => {
                let name = name.clone();
                move |this: &mut Self| -> LuaResult<bool> {
                    let Some(session) = session.or_else(|| this.active_session()) else {
                        return Ok(false);
                    };
                    Ok(this.rename_session(session, &name))
                }
            })
        });
        methods.add_function("close", |lua, args: LuaMultiValue| {
            let (this, (session,)) = args!(args, lua, (Option<SessionId>,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                let Some(session) = session.or_else(|| this.active_session()) else {
                    return Ok(false);
                };
                let Some(term_session) = this
                    .workspace
                    .session_manager
                    .session_mut(session)
                else {
                    return Ok(false);
                };
                term_session.pty.kill();
                Ok(this.close_session(session))
            })
        });
        methods.add_function("move_to", |lua, args: LuaMultiValue| {
            let (this, (tab,)) = args!(args, lua, (Option<usize>,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                let Some(tab) = tab else {
                    return Ok(false);
                };
                let Some(session) = this.active_session() else {
                    return Ok(false);
                };
                Ok(this.move_session_to_tab(session, tab))
            })
        });
        methods.add_function("switch_tab", |lua, args: LuaMultiValue| {
            let (this, (tab,)) = args!(args, lua, (usize,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                Ok(this.switch_tab(tab))
            })
        });
        methods.add_function("next_tab", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<usize> {
                let index = this.next_tab_index();
                this.switch_tab(index);
                Ok(index)
            })
        });
        methods.add_function("prev_tab", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<usize> {
                let index = this.previous_tab_index();
                this.switch_tab(index);
                Ok(index)
            })
        });
        methods.add_function("split", |lua, args: LuaMultiValue| {
            let (this, direction) = args!(args, lua, String);
            let direction = match direction.to_lowercase().as_str() {
                "horizontal" | "h" => SplitDirection::Horizontal,
                _ => SplitDirection::Vertical,
            };
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<Option<SessionId>> {
                Ok(this.split_current_tab(direction))
            })
        });
        methods.add_function("focus_next_pane", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<Option<SessionId>> {
                Ok(this.focus_next_pane())
            })
        });
        methods.add_function("write", |lua, args: LuaMultiValue| {
            let (this, (text,)) = args!(args, lua, (String,));
            callable_action!(lua, this => {
                let text = text.clone();
                move |this: &mut Self| -> LuaResult<bool> {
                    let Some(active) = this.active_session() else {
                        return Ok(false);
                    };
                    this.workspace.session_manager.send_text(active, &text);
                    Ok(true)
                }
            })
        });
        methods.add_function("write_to", |lua, args: LuaMultiValue| {
            let (this, (session, text)) = args!(args, lua, (SessionId, String));
            callable_action!(lua, this => {
                let text = text.clone();
                move |this: &mut Self| -> LuaResult<bool> {
                    this.workspace.session_manager.send_text(session, &text);
                    Ok(true)
                }
            })
        });
        methods.add_function("open_releases", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.request_overlay(Screen::Releases);
                Ok(())
            })
        });
        methods.add_function("open_opener", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.request_overlay(Screen::Opener);
                Ok(())
            })
        });
        methods.add_function("open_command", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.queue_action(OpenCommandPrompt);
                Ok(())
            })
        });
        methods.add_function("open_lua", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.queue_action(OpenLuaPrompt);
                Ok(())
            })
        });
        methods.add_function("toggle_fullscreen", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<bool> {
                // The `ToggleFullscreen` handler flips `fullscreen` and the
                // window together; predict the post-toggle value (the queue
                // drains in order before any other Lua runs).
                this.queue_action(ToggleFullscreen);
                Ok(!this.fullscreen)
            })
        });
        methods.add_function("toggle_decorations", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                // GPUI manages window chrome itself; there is no runtime
                // toggle. Stay honest instead of silently doing nothing.
                // No window/cx here (Lua) — queue for `render` to dispatch.
                this.queue_action(ShowStatusMessage {
                    text: "window decorations toggle is not supported on GPUI".to_string(),
                });
                Ok(())
            })
        });
        methods.add_function("toggle_status_bar", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                // No window/cx here — queue; the handler flips synchronously.
                this.queue_action(ToggleStatusBar);
                Ok(())
            })
        });
        methods.add_function("reload_config", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                load(this);
                Ok(())
            })
        });
        methods.add_function("quit", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                _ = this.event_tx.try_send(PtyEvent::Exit);
                Ok(())
            })
        });
        methods.add_function(
            "create_session",
            |lua, args: LuaMultiValue| {
                let (this, (dir, tab, direction, parent)) = args!(
                    args,
                    lua,
                    (
                        Option<String>,
                        Option<usize>,
                        Option<String>,
                        Option<SessionId>
                    )
                );
                let direction = match direction.as_deref().map(str::to_lowercase).as_deref() {
                    Some("horizontal") | Some("h") => SplitDirection::Horizontal,
                    _ => SplitDirection::Vertical,
                };
                callable_action!(lua, this => {
                    let dir = dir.clone();
                    move |this: &mut Self| -> LuaResult<SessionId> {
                        let cols = this.terminal.cols;
                        let rows = this.terminal_rows();
                        this.workspace.create_session_placed(
                            dir.as_deref().map(Path::new),
                            tab,
                            direction,
                            parent,
                            cols,
                            rows,
                        ).map_err(|e| mlua::Error::external(e))
                    }
                })
            },
        );
    }

    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("current_tab", |_, this| Ok(this.workspace.current_tab));
        fields.add_field_method_get("font_size", |_, this| Ok(this.terminal.font_size));
        fields.add_field_method_get("status_height", |_, this| {
            Ok(this.terminal.status_height)
        });
        fields.add_field_method_get("line_height", |_, this| Ok(this.terminal.line_height));
        fields.add_field_method_get("font_family", |_, this| {
            Ok(this.terminal.font_family.clone())
        });
        fields.add_field_method_get("rows", |_, this| Ok(this.terminal.rows));
        fields.add_field_method_get("cols", |_, this| Ok(this.terminal.cols));
        fields.add_field_method_get("active_session", |lua, this| {
            this.active_session().into_lua(lua)
        });
        fields.add_field_method_get("tab_count", |_, this| {
            Ok(this.workspace.tabs.iter().flatten().count())
        });
        fields.add_field_method_get("detached_sessions", |lua, this| {
            let table = lua.create_table()?;
            for (index, session) in this.live_detached_sessions().into_iter().enumerate() {
                table.raw_set(index + 1, session)?;
            }
            Ok(table)
        });
        fields.add_field_method_get("ssh_sessions", |lua, this| {
            let table = lua.create_table()?;
            for (index, session) in this.workspace.ssh_sessions.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.raw_set("name", session.name.clone())?;
                entry.raw_set("user_name", session.user_name.clone())?;
                entry.raw_set("ip", session.ip.to_string())?;
                table.raw_set(index + 1, entry)?;
            }
            Ok(table)
        });

        fields.add_field_method_get("sessions", |lua, this| {
            let tabs = this.workspace.tabs.iter().filter_map(|tab| {
                lua.create_sequence_from(tab.as_ref()?.sessions()).ok()
            });
            lua.create_sequence_from(tabs)
        });
    }
}*/

pub fn watch(tx: Sender<PtyEvent>) {
    let Some(path) = util::config_path() else {
        return;
    };
    thread::spawn(move || {
        let func = move || -> Result<()> {
            use notify::{EventKind, RecommendedWatcher, Watcher};
            use std::sync::mpsc;
            let (watch_tx, rx) = mpsc::channel();
            let config = notify::Config::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(true);
            let mut watcher = RecommendedWatcher::new(watch_tx, config)?;
            watcher.watch(&path.absolutize()?, RecursiveMode::Recursive)?;
            while let Ok(ev) = rx.recv() {
                if let Ok(ev) = ev
                    && let EventKind::Modify(_) = ev.kind
                {
                    tx.force_send(PtyEvent::ConfigChanged)
                        .expect("event tx closed");
                }
            }
            Ok(())
        };
        if let Err(e) = func() {
            tracing::error!(?e, "watcher thread error");
        }
    });
}

pub fn load(this: &mut Pyonji, cx: &mut Context<Pyonji>) {
    let Some(path) = util::config_path() else {
        return;
    };

    _ = util::create_config_if_missing(&path)
        .inspect_err(|e| tracing::error!(%e, "failed to create default config"));

    //this.workspace.ssh_sessions.clear();
    //this.input.keymap = Surface::default_keymap();
    //this.registered_callbacks.clear();
    let lua = this.lua.clone();
    _ = with_env(this, cx, |_| {
        let chunk = lua.load(path);
        chunk.exec()
    });
    /*let action_key = this.input.action.clone();
    let overridden = matches!(
        this.input.keymap.get(&action_key),
        Some(KeyAction::Custom(_))
    );
    if !overridden {
        this.input
            .keymap
            .retain(|_, v| !matches!(v, KeyAction::Builtin(BuiltinAction::Action)));
        this.input
            .keymap
            .insert(action_key, KeyAction::Builtin(BuiltinAction::Action));
    }
    // Palette/tabs/ssh sync to the StatusBar every frame in `render`; no
    // explicit refresh needed here (keeps `load` synchronous for the file
    // watcher and Lua without a window/cx).
    */
}

pub fn with_env<R>(
    this: &mut Pyonji,
    cx: &mut Context<Pyonji>,
    f: impl FnOnce(LuaAnyUserData) -> LuaResult<R>,
) -> Result<R> {
    let lua = this.lua.clone();
    let mut proxy = LuaProxy {
        py: UnsafeRefMut::new(this),
        cx: ProxyContext::new(cx),
    };
    let res = lua.scope(|scope| {
        let app = scope.create_userdata_ref_mut(&mut proxy)?;
        lua.globals().set("py", app.clone())?;
        let ret = f(app);
        lua.globals().remove("py")?;
        ret
    })?;
    Ok(res)
}

pub fn install_inspect(lua: &Lua) -> Result<()> {
    lua.load_std_libs(mlua::StdLib::DEBUG)?;
    {
        let chunk = lua.load(include_str!("../resources/lua/inspect.lua"));
        lua.globals()
            .set("__HOST_INSPECT_FUNC", chunk.call::<LuaFunction>(())?)?;
    }
    Ok(())
}

mod util {
    use std::path::Path;

    use super::*;
    pub fn collect_ssh_sessions(lua: &Lua, table: &LuaTable) -> Vec<SshConnection> {
        let Ok(sessions) = table.get::<Vec<LuaValue>>("ssh_sessions") else {
            return vec![];
        };
        sessions
            .into_iter()
            .map(|session| -> LuaResult<SshConnection> {
                let table = session
                    .as_table()
                    .context("ssh_session entry is not a table")?;
                let name = table
                    .get("name")
                    .and_then(|v| from_value::<String>(v, lua))?;
                Ok(SshConnection {
                    name: name.clone(),
                    user_name: table
                        .get("user_name")
                        .and_then(|v| from_value(v, lua))
                        .unwrap_or(name),
                    ip: table
                        .get::<LuaValue>("ip")
                        .and_then(|v| from_value(v, lua))
                        .map(|ip: String| IpAddr::from_str(&ip))??,
                })
            })
            .collect::<LuaResult<Vec<_>>>()
            .unwrap_or_default()
    }

    pub fn from_value<T: FromLua>(value: LuaValue, lua: &Lua) -> LuaResult<T> {
        if let Some(v) = value.as_function() {
            v.call::<T>(())
        } else {
            T::from_lua(value, lua)
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn config_path() -> Option<PathBuf> {
        cfg_select! {
            feature = "install" => {
                dirs::config_local_dir().map(|dir| dir.join("pyonji").join("init.lua"))
            }
            _ => Some("init.lua".into())
        }
    }

    pub fn create_config_if_missing(path: &Path) -> Result<()> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            file.write_all(DEFAULT_CONFIG.as_bytes())?;
        }
        Ok(())
    }
}

/// A keyboard shortcut, independent of any windowing toolkit.
///
/// `modifiers` uses [`gpui::Modifiers`] (`control`/`alt`/`shift`/`platform`)
/// and `key` is the lowercase GPUI key name (`"b"`, `"f1"`, `"up"`,
/// `"space"`, `"semicolon"`, …). The Lua surface keeps accepting the
/// historical `"<ctrl+shift>-F"` spelling; [`KeyBinding::parse`] normalizes it
/// into this form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub modifiers: Modifiers,
    pub key: String,
}

impl KeyBinding {
    pub fn new(modifiers: Modifiers, key: impl Into<String>) -> Self {
        Self {
            modifiers,
            key: Self::canonical_key(&key.into()),
        }
    }

    pub fn control(key: impl Into<String>) -> Self {
        Self::new(
            Modifiers {
                control: true,
                ..Default::default()
            },
            key,
        )
    }

    /// Whether this binding matches a GPUI key-down event.
    ///
    /// Modifiers must match exactly (ignoring `function`, which has no
    /// historical equivalent). The key matches either [`Keystroke::key`] or
    /// the typed [`Keystroke::key_char`], so punctuation spellings like
    /// `"semicolon"` keep working even though GPUI reports the literal `";"`.
    pub fn matches_event(&self, event: &KeyDownEvent) -> bool {
        let mods = &event.keystroke.modifiers;
        if self.modifiers.control != mods.control
            || self.modifiers.alt != mods.alt
            || self.modifiers.shift != mods.shift
            || self.modifiers.platform != mods.platform
        {
            return false;
        }
        let key = event.keystroke.key.to_lowercase();
        if key == self.key {
            return true;
        }
        if let Some(literal) = Self::key_literal(&self.key) {
            if key == literal {
                return true;
            }
        }
        if let Some(ch) = event.keystroke.key_char.as_deref() {
            let ch = ch.to_lowercase();
            if ch == self.key {
                return true;
            }
            if let Some(literal) = Self::key_literal(&self.key) {
                if ch == literal {
                    return true;
                }
            }
        }
        false
    }

    /// Canonicalize a key name to the lowercase GPUI spelling.
    fn canonical_key(key: &str) -> String {
        match key.to_lowercase().as_str() {
            "esc" => "escape".to_string(),
            "return" => "enter".to_string(),
            "del" => "delete".to_string(),
            "ins" => "insert".to_string(),
            "arrowup" => "up".to_string(),
            "arrowdown" => "down".to_string(),
            "arrowleft" => "left".to_string(),
            "arrowright" => "right".to_string(),
            "pageup" => "pageup".to_string(),
            "pagedown" => "pagedown".to_string(),
            "grave" => "backquote".to_string(),
            "dot" => "period".to_string(),
            "equal" => "equal".to_string(),
            "apostrophe" => "quote".to_string(),
            "[" => "bracketleft".to_string(),
            "]" => "bracketright".to_string(),
            other => other.to_string(),
        }
    }

    /// The literal character a punctuation name produces, if any.
    fn key_literal(name: &str) -> Option<&'static str> {
        Some(match name {
            "space" => " ",
            "semicolon" => ";",
            "comma" => ",",
            "period" => ".",
            "slash" => "/",
            "backslash" => "\\",
            "minus" => "-",
            "equal" => "=",
            "quote" => "'",
            "backquote" => "`",
            "bracketleft" => "[",
            "bracketright" => "]",
            _ => return None,
        })
    }

    pub fn parse(binding: impl AsRef<str>) -> Result<Self> {
        let binding = binding.as_ref();
        let (binding, modifiers) = Self::parse_mods(binding)?;
        let key = if !matches!(
            (
                modifiers.control,
                modifiers.alt,
                modifiers.shift,
                modifiers.platform
            ),
            (false, false, false, false)
        ) && let Some(delim) = binding.chars().position(|c| c == '-')
        {
            &binding[delim + 1..]
        } else {
            binding
        };
        let key = Self::parse_key(key)?;

        Ok(Self { modifiers, key })
    }

    fn parse_mods(binding: &str) -> Result<(&str, Modifiers)> {
        if !binding.starts_with('<') {
            return Ok((binding, Modifiers::default()));
        }
        let binding = &binding[1..];
        let end = binding
            .chars()
            .position(|c| c == '>')
            .context("failed to find `>` while parsing keybinding modifiers")?;
        let mut modifiers = &binding[..end];

        let mut mods = Modifiers::default();

        if !modifiers.is_empty() {
            while let Some(next) = modifiers.chars().position(|c| c == '+') {
                let modifier = &modifiers[..next];
                mods |= Self::parse_mod(modifier)?;
                modifiers = &modifiers[next + 1..];
            }
            mods |= Self::parse_mod(modifiers)?;
        }

        let rest = &binding[end..];

        Ok((rest, mods))
    }

    pub fn parse_mod(m: &str) -> Result<Modifiers> {
        Ok(match m.trim() {
            "ctrl" => Modifiers {
                control: true,
                ..Default::default()
            },
            "alt" => Modifiers {
                alt: true,
                ..Default::default()
            },
            "shift" => Modifiers {
                shift: true,
                ..Default::default()
            },
            "mod" => Modifiers {
                platform: true,
                ..Default::default()
            },
            x => {
                anyhow::bail!("`{x}` is not a valid modifier");
            }
        })
    }

    /// Parse a historical key name into its canonical GPUI spelling.
    pub fn parse_key(key: &str) -> Result<String> {
        let key = key.to_lowercase();
        let canonical = match key.as_str() {
            "a" | "b" | "c" | "d" | "e" | "f" | "g" | "h" | "i" | "j" | "k" | "l" | "m" | "n"
            | "o" | "p" | "q" | "r" | "s" | "t" | "u" | "v" | "w" | "x" | "y" | "z" | "0" | "1"
            | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => key,
            "up" | "arrowup" => "up".to_string(),
            "down" | "arrowdown" => "down".to_string(),
            "left" | "arrowleft" => "left".to_string(),
            "right" | "arrowright" => "right".to_string(),
            "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11"
            | "f12" => key,
            "space" => "space".to_string(),
            "enter" | "return" => "enter".to_string(),
            "esc" | "escape" => "escape".to_string(),
            "tab" => "tab".to_string(),
            "backspace" => "backspace".to_string(),
            "delete" | "del" => "delete".to_string(),
            "insert" | "ins" => "insert".to_string(),
            "home" => "home".to_string(),
            "end" => "end".to_string(),
            "pageup" => "pageup".to_string(),
            "pagedown" => "pagedown".to_string(),
            "semicolon" => "semicolon".to_string(),
            "comma" => "comma".to_string(),
            "period" | "dot" => "period".to_string(),
            "slash" => "slash".to_string(),
            "backslash" => "backslash".to_string(),
            "minus" => "minus".to_string(),
            "equals" | "equal" => "equal".to_string(),
            "quote" | "apostrophe" => "quote".to_string(),
            "backquote" | "grave" => "backquote".to_string(),
            "bracketleft" | "[" => "bracketleft".to_string(),
            "bracketright" | "]" => "bracketright".to_string(),
            x => anyhow::bail!("`{x}` is not a valid key"),
        };
        Ok(canonical)
    }

    /// Digit value for `1`–`9` bindings (tab selection), if this is one.
    pub fn digit_index(&self) -> Option<usize> {
        if self.modifiers.control
            || self.modifiers.alt
            || self.modifiers.shift
            || self.modifiers.platform
        {
            return None;
        }
        match self.key.as_str() {
            "1" => Some(0),
            "2" => Some(1),
            "3" => Some(2),
            "4" => Some(3),
            "5" => Some(4),
            "6" => Some(5),
            "7" => Some(6),
            "8" => Some(7),
            "9" => Some(8),
            _ => None,
        }
    }
}

impl FromLua for KeyBinding {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        let binding = value.as_string().context("not a string")?;
        Ok(Self::parse(binding.to_str()?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(key: &str, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers,
                key: key.to_string(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn parses_historical_spellings() {
        let binding = KeyBinding::parse("<ctrl>-B").unwrap();
        assert!(binding.modifiers.control);
        assert_eq!(binding.key, "b");

        let binding = KeyBinding::parse("<ctrl+shift>-F").unwrap();
        assert!(binding.modifiers.control && binding.modifiers.shift);
        assert_eq!(binding.key, "f");

        let binding = KeyBinding::parse("semicolon").unwrap();
        assert_eq!(binding.key, "semicolon");

        assert!(KeyBinding::parse("<bogus>-a").is_err());
        assert!(KeyBinding::parse("<ctrl>-bogus").is_err());
    }

    #[test]
    fn matches_gpui_key_events() {
        let binding = KeyBinding::control("b");
        let event = keystroke(
            "b",
            Modifiers {
                control: true,
                ..Default::default()
            },
        );
        assert!(binding.matches_event(&event));
        assert!(!KeyBinding::control("c").matches_event(&event));

        // Punctuation keeps working when GPUI reports the literal character.
        let binding = KeyBinding::parse("semicolon").unwrap();
        let event = KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: Modifiers::default(),
                key: ";".to_string(),
                key_char: Some(";".to_string()),
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(binding.matches_event(&event));
    }

    #[test]
    fn digit_index_covers_tabs() {
        assert_eq!(KeyBinding::parse("1").unwrap().digit_index(), Some(0));
        assert_eq!(KeyBinding::parse("9").unwrap().digit_index(), Some(8));
        assert_eq!(KeyBinding::parse("<ctrl>-1").unwrap().digit_index(), None);
    }
}
