use crate::pty::{Event as PtyEvent, SshConnection};
use crate::terminal::{SessionId, SplitDirection};
use crate::ui::OverlayScreen;
use crate::util::UnsafeRefMut;
use crate::{EnterLuaRepl, EnterRename, ExecKeybind, Pyonji};
use anyhow::{Context as _, Result, anyhow};
use async_channel::Sender;
use gpui::{App, Context, Entity, EntityId, KeyBinding, WeakEntity, Window};
use gpui_component::ThemeMode;
use mlua::{FromLua, prelude::*};
use notify::RecursiveMode;
use path_absolutize::*;
use std::fmt::Debug;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;

pub(crate) const DEFAULT_CONFIG: &str = include_str!("../resources/default.lua");
pub const LUA_MODULES: &[(&str, &str)] =
    &[("lua.keybind", include_str!("../resources/lua/keybind.lua"))];

#[allow(dead_code)]
pub struct LuaAction {
    pub args: Vec<String>,
    pub is_var_arg: bool,
    pub name: String,
    pub callback: LuaFunction,
}

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

#[allow(dead_code)]
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

    fn as_ctx(&mut self) -> Context<'_, T> {
        Context::new_context(&mut self.app, self.entity_state.clone())
    }
}

struct LuaProxy {
    py: UnsafeRefMut<Pyonji>,
    cx: ProxyContext<Pyonji>,
    window: UnsafeRefMut<Window>,
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
            let ssh_sessions = util::collect_ssh_sessions(lua, &table);

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
            if let Some(editor) = config_option_value::<String>(lua, &table, "editor") {
                this.py.editor = editor;
            }
            this.py.terminal.update(this.cx.app.as_mut(), |term, _| {
                let Some(renderer) = term.renderer.as_mut() else {
                    return;
                };
                renderer
                    .set_font_metrics(this.py.font_size, this.py.font_size * this.py.line_height);
                if let Some(font) = this.py.font_family.as_deref() {
                    renderer.set_font_family(font);
                }
                renderer.evict_glyphs();
            });
            Ok(())
        });
        methods.add_method_mut("bind", |lua, this, args: LuaMultiValue| {
            if args.front().is_some_and(|arg| arg.is_table()) {
                /*let (mods, key, func): (Vec<String>, String, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                let mut state = Modifiers::default();
                for modifier in &mods {
                    state |= ConfigKeyBinding::parse_mod(modifier)?;
                }
                let key = ConfigKeyBinding::parse_key(&key)?;
                let binding = ConfigKeyBinding {
                    keystrokes: vec![SingleKeyBinding {
                        modifiers: state,
                        key,
                    }],
                };
                this.cx.app.bind_keys([KeyBinding::new(
                    binding.to_gpui_keys().as_str(),
                    ExecKeybind(func.clone()),
                    None,
                )]);*/
                todo!()
            } else {
                let (binding, func): (ConfigKeyBinding, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                this.cx.app.bind_keys([KeyBinding::new(
                    binding.to_gpui_keys().as_str(),
                    ExecKeybind(func.clone()),
                    None,
                )]);
            }
            Ok(())
        });
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
                this.py.registered_callbacks.push(LuaAction {
                    args,
                    is_var_arg,
                    name,
                    callback: func,
                });
                Ok(())
            },
        );
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
            let _direction = match direction.as_deref().map(str::to_lowercase).as_deref() {
                Some("horizontal") | Some("h") => SplitDirection::Horizontal,
                _ => SplitDirection::Vertical,
            };
            callable_action!(lua, this => {
                let dir = dir.clone();
                move |this: &mut Self| -> LuaResult<SessionId> {
                    let id = this.py.create_session(dir.as_ref().map(Path::new), tab, parent, &mut this.cx.as_ctx())
                        .context("failed to create session")?;
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
                this.py.close_session(id, &mut this.cx.as_ctx());
                Ok(true)
            })
        });
        methods.add_function("switch_tab", |lua, args: LuaMultiValue| {
            let (this, (tab,)) = args!(args, lua, (usize,));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                this.py.switch_tab(tab, &mut this.cx.as_ctx());
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
                let Some(session) = this.py.active_session() else {
                    return Ok(false);
                };
                Ok(this.py.move_session(None, target, session, &mut this.cx.as_ctx()))
            })
        });
        methods.add_function("split", |lua, args: LuaMultiValue| {
            let (this, direction) = args!(args, lua, String);
            let direction = match direction.to_lowercase().as_str() {
                "horizontal" | "h" => SplitDirection::Horizontal,
                _ => SplitDirection::Vertical,
            };
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<Option<SessionId>> {
                Ok(this.py.split_active(direction, &mut this.cx.as_ctx()).ok())
            })
        });
        methods.add_function("focus_next_pane", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<Option<SessionId>> {
                let Some(current) = this.py.current_tab else {
                    return Ok(None);
                };
                let res = this.py.tabs[current].as_mut().and_then(|tab| tab.focus_next());
                this.cx.as_ctx().notify();
                Ok(res)
            })
        });
        methods.add_function("detach", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<bool> {
                let Some(session) = this.py.active_session() else {
                    return Ok(false);
                };
                Ok(this.py.detach_session(session, &mut this.cx.as_ctx()))
            })
        });
        methods.add_function("attach", |lua, args: LuaMultiValue| {
            let (this, (session, tab)) = args!(args, lua, (SessionId, Option<usize>));
            callable_action!(lua, this => move |this: &mut Self| -> LuaResult<bool> {
                Ok(this.py.attach_session(session, tab, &mut this.cx.as_ctx()))
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
        methods.add_function("write", |lua, args: LuaMultiValue| {
            let (this, (id, string)) = args!(args, lua, (SessionId, String));
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.py.session_manager.send_text(id, &string);
                Ok(())
            })
        });
        methods.add_function("open_sessions", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                this.py.overlay.update(this.cx.app.as_mut(), |this, cx| {
                    this.open(OverlayScreen::Sessions, window, cx);
                });
                Ok(())
            })
        });
        methods.add_function("open_detached", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                this.py.overlay.update(this.cx.app.as_mut(), |this, cx| {
                    this.open(OverlayScreen::Detached, window, cx);
                });
                Ok(())
            })
        });
        methods.add_function("open_releases", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                this.py.overlay.update(this.cx.app.as_mut(), |this, cx| {
                    this.open(OverlayScreen::Releases, window, cx);
                });
                Ok(())
            })
        });
        methods.add_function("open_opener", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                this.py.overlay.update(this.cx.app.as_mut(), |this, cx| {
                    this.open(OverlayScreen::Opener, window, cx);
                });
                Ok(())
            })
        });
        methods.add_function("open_palette", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                this.py.overlay.update(this.cx.app.as_mut(), |this, cx| {
                    this.open(OverlayScreen::Palette, window, cx);
                });
                Ok(())
            })
        });
        methods.add_function("open_rename", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                window.dispatch_action(Box::new(EnterRename), &mut this.cx.app);
                Ok(())
            })
        });
        methods.add_function("open_lua", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                let window = this.window.as_mut();
                window.dispatch_action(Box::new(EnterLuaRepl), &mut this.cx.app);
                Ok(())
            })
        });
        methods.add_function("reload_config", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                load(&mut this.py, &mut this.window, &mut this.cx.as_ctx())?;
                Ok(())
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
                .with_compare_contents(true)
                .with_follow_symlinks(true);
            let mut watcher = RecommendedWatcher::new(watch_tx, config)?;
            watcher.watch(&path.absolutize()?, RecursiveMode::Recursive)?;
            loop {
                let ev = rx.recv();
                let Ok(Ok(event)) = ev else {
                    continue;
                };
                if matches!(event.kind, EventKind::Modify(_)) {
                    tx.force_send(PtyEvent::ConfigChanged)
                        .expect("event tx closed");
                }
            }
        };
        if let Err(e) = func() {
            tracing::error!(?e, "watcher thread error");
        }
    });
}

pub fn load(this: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>) -> Result<()> {
    let path = util::config_path().context("failed to get config path")?;

    util::create_config_if_missing(&path)
        .context(format!("failed to create default config at {path:?}"))?;

    cx.clear_key_bindings();
    Pyonji::init(cx);
    gpui_component::init(cx);
    gpui_component::Theme::change(ThemeMode::Dark, Some(window), cx);
    this.ssh_sessions.clear();
    this.registered_callbacks.clear();

    let lua = this.lua.clone();
    with_env(this, window, cx, |_| {
        let chunk = lua.load(path);
        chunk.exec()
    })
}

pub fn with_env<R>(
    this: &mut Pyonji,
    window: &mut Window,
    cx: &mut Context<Pyonji>,
    f: impl FnOnce(LuaAnyUserData) -> LuaResult<R>,
) -> Result<R> {
    let lua = this.lua.clone();
    let mut proxy = LuaProxy {
        py: UnsafeRefMut::new(this),
        cx: ProxyContext::new(cx),
        window: UnsafeRefMut::new(window),
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

pub(crate) mod util {
    use std::path::Path;

    use super::*;
    pub fn collect_ssh_sessions(lua: &Lua, table: &LuaTable) -> Option<Vec<SshConnection>> {
        let Ok(sessions) = table.get::<Vec<LuaValue>>("ssh_sessions") else {
            return None;
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
            .ok()
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigKeyBinding {
    mods: Option<Modifier>,
    key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Modifier {
    parts: Vec<String>,
    split: bool,
}

impl ConfigKeyBinding {
    pub fn to_gpui_keys(&self) -> String {
        let mut out = String::new();

        if let Some(mods) = self.mods.as_ref() {
            mods.parts.iter().for_each(|modifier| {
                out.push_str(&format!("{modifier}-"));
            });
            if mods.split {
                out.pop();
                out.push(' ');
            }
        }
        out.push_str(&self.key);
        out
    }

    pub fn parse(binding: impl AsRef<str>) -> Result<Self> {
        let binding = binding.as_ref().trim();
        if binding.starts_with('<') && binding.contains('>') {
            let end = binding
                .chars()
                .position(|c| c == '>')
                .context("failed to find `>` while parsing keybinding")?;
            let inside = &binding[1..end].trim();
            let rest = &binding[end + 1..].trim();
            let parts = inside
                .split('-')
                .map(Self::canonical_key)
                .collect::<Vec<_>>();
            let is_split = parts.iter().any(|part| !Self::is_mod(part));
            Ok(Self {
                mods: if parts.is_empty() {
                    None
                } else {
                    Some(Modifier {
                        parts,
                        split: is_split,
                    })
                },
                key: Self::canonical_key(rest),
            })
        } else {
            Err(anyhow!("{binding} is not a valid keybind"))
        }
    }

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

    fn is_mod(m: &str) -> bool {
        matches!(m.trim(), "ctrl" | "alt" | "shift" | "mod")
    }
}

impl FromLua for ConfigKeyBinding {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        let binding = value.as_string().context("not a string")?;
        Ok(Self::parse(binding.to_str()?)?)
    }
}
