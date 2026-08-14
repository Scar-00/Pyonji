use crate::overlay::{LuaAction, Screen};
use crate::pty::{Event as PtyEvent, SshConnection};
use crate::terminal::{SessionId, SplitDirection, Tab};
use crate::{App, BuiltinAction, KeyAction, ResultExt};
use anyhow::{Context, Result};
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
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{KeyCode, ModifiersState};
use winit::window::Fullscreen;

const DEFAULT_CONFIG: &str = include_str!("../resources/default.lua");
pub const LUA_MODULES: &[(&str, &str)] = &[("lua.keybind", include_str!("../resources/lua/keybind.lua"))];

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
            let func = $lua.create_function(move |_, this: LuaAnyUserData| {
                this.borrow_mut_scoped($body)?
            })?;
            Ok(LuaValue::Function(func))
        }
    }}
}

macro_rules! args {
    ($args: ident, $lua: ident, $typ: ty) => {{
        let mut args = $args;
        let first = args.pop_front();
        let this = match first {
            Some(first) if first.is_userdata() => {
                Some(LuaAnyUserData::from_lua(first, $lua)?)
            }
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

impl App {
    pub fn apply_config(&mut self) {
        let Some(size) = self.window.as_ref().map(|window| window.inner_size()) else {
            return;
        };
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_font_metrics(self.font_size, self.line_height);
            if let Some(font_family) = self.font_family.as_deref() {
                renderer.set_font_family(font_family);
            }
            renderer.evict_glyphs();
        }
        if let Some(mut overlay) = self.overlay.take() {
            overlay.update_cmds(self);
            self.overlay = Some(overlay);
        }
        self.rows = (size.height as f32 / self.line_height) as u16;
        self.cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
        self.resize_tab();

        self.request_redraw();
    }
}

impl LuaUserData for App {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("bind", |lua, this, args: LuaMultiValue| {
            if args.front().is_some_and(|arg| arg.is_table()) {
                let (mods, key, func): (Vec<String>, String, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                let mods = mods
                    .iter()
                    .map(|modifier| KeyBinding::parse_mod(modifier))
                    .collect::<Result<Vec<_>>>()?;
                let mut state = ModifiersState::empty();
                for m in mods {
                    state |= m;
                }
                let key = KeyBinding::parse_key(&key)?;
                this.keymap
                    .insert(KeyBinding { mods: state, key }, KeyAction::Custom(func.clone()));
            } else {
                let (binding, func): (KeyBinding, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                this.keymap
                    .insert(binding, KeyAction::Custom(func));
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
            apply!(this.font_size, table, |font_size: f32| {
                this.line_height = font_size * 1.1;
                font_size
            });
            apply!(this.line_height, table, |line_height: f32| {
                line_height * this.font_size
            });
            apply!(this.font_family, table);
            apply!(this.fullscreen, table);
            apply!(this.default_cwd, table);
            apply!(this.action, table);
            this.ssh_sessions = util::collect_ssh_sessions(lua, &table);

            this.apply_config();
            Ok(())
        });
        methods.add_function("open_palette", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                if let Some(overlay) = this.overlay.as_mut() {
                    overlay.show(Some(Screen::CmdPalette));
                }
                Ok(())
            })
        });
        methods.add_function("open_sessions", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                if let Some(overlay) = this.overlay.as_mut() {
                    overlay.show(Some(Screen::Sessions));
                }
                Ok(())
            })
        });
        methods.add_function("open_detached", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                if let Some(overlay) = this.overlay.as_mut() {
                    overlay.show(Some(Screen::Detached));
                }
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
                Ok(this.reattach_session(session, tab.unwrap_or(this.current_tab)))
            })
        });
        methods.add_function("open_rename", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.open_rename_prompt();
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
                let Some(term_session) = this.session_manager.session_mut(session) else {
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
            let (this, (direction,)) = args!(args, lua, (String,));
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
                    this.session_manager.send_text(active, &text);
                    Ok(true)
                }
            })
        });
        methods.add_function("write_to", |lua, args: LuaMultiValue| {
            let (this, (session, text)) = args!(args, lua, (SessionId, String));
            callable_action!(lua, this => {
                let text = text.clone();
                move |this: &mut Self| -> LuaResult<bool> {
                    this.session_manager.send_text(session, &text);
                    Ok(true)
                }
            })
        });
        methods.add_function("open_releases", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                if let Some(overlay) = this.overlay.as_mut() {
                    overlay.show(Some(Screen::Releases));
                }
                Ok(())
            })
        });
        methods.add_function("open_opener", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                if let Some(overlay) = this.overlay.as_mut() {
                    overlay.show(Some(Screen::Opener));
                }
                Ok(())
            })
        });
        methods.add_function("open_command", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.open_status_prompt();
                Ok(())
            })
        });
        methods.add_function("open_lua", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.open_lua_prompt();
                Ok(())
            })
        });
        methods.add_function("toggle_fullscreen", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<bool> {
                let Some(window) = this.window.as_ref() else {
                    return Ok(false);
                };
                let fullscreen = window.fullscreen().is_none();
                window.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
                this.fullscreen = fullscreen;
                Ok(fullscreen)
            })
        });
        methods.add_function("toggle_decorations", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                if let Some(window) = this.window.as_mut() {
                    window.set_decorations(!window.is_decorated());
                    this.request_redraw();
                }
                Ok(())
            })
        });
        methods.add_function("toggle_status_bar", |lua, this: Option<LuaAnyUserData>| {
            callable_action!(lua, this => |this: &mut Self| -> LuaResult<()> {
                this.status_bar_hidden = !this.status_bar_hidden;
                this.resize_tab();
                this.request_redraw();
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
                _ = this._proxy.send_event(PtyEvent::Exit);
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
                        let session = this.session_manager.create_session(
                            this.terminal_rows().max(1),
                            this.cols.max(1),
                            dir
                                .as_deref()
                                .map(Path::new)
                                .or(this.default_cwd.as_deref()),
                        )?;
                        let target = tab
                            .map(|tab| tab.min(this.tabs.len().saturating_sub(1)))
                            .unwrap_or(this.current_tab);
                        let tab = &mut this.tabs[target];
                        let placed = if let Some(tab) = tab {
                            let anchor = parent
                                .filter(|anchor| tab.sessions().contains(anchor))
                                .or_else(|| tab.active_session());
                            match anchor {
                                Some(anchor) => tab.split_on(anchor, direction, session),
                                None => false,
                            }
                        } else {
                            *tab = Some(Tab::new(session));
                            true
                        };
                        if !placed {
                            this.detached_sessions.push(session);
                        }
                        this.wheel_remainder = 0.0;
                        this.resize_tab();
                        this.update_ime_cursor_area();
                        this.request_redraw();
                        Ok(session)
                    }
                })
            },
        );
    }

    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("current_tab", |_, this| Ok(this.current_tab));
        fields.add_field_method_get("font_size", |_, this| Ok(this.font_size));
        fields.add_field_method_get("line_height", |_, this| Ok(this.line_height));
        fields.add_field_method_get("font_family", |_, this| Ok(this.font_family.clone()));
        fields.add_field_method_get("rows", |_, this| Ok(this.rows));
        fields.add_field_method_get("cols", |_, this| Ok(this.cols));
        fields.add_field_method_get("active_session", |lua, this| {
            this.active_session().into_lua(lua)
        });
        fields.add_field_method_get("tab_count", |_, this| {
            Ok(this.tabs.iter().flatten().count())
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
            for (index, session) in this.ssh_sessions.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.raw_set("name", session.name.clone())?;
                entry.raw_set("user_name", session.user_name.clone())?;
                entry.raw_set("ip", session.ip.to_string())?;
                table.raw_set(index + 1, entry)?;
            }
            Ok(table)
        });

        fields.add_field_method_get("sessions", |lua, this| {
            let tabs = this.tabs.iter().filter_map(|tab| {
                lua.create_sequence_from(tab.as_ref()?.sessions()).ok()
            });
            lua.create_sequence_from(tabs)
        });
    }
}

pub fn watch(proxy: EventLoopProxy<PtyEvent>) {
    let Some(path) = util::config_path() else {
        return;
    };
    thread::spawn(move || {
        let func = move || -> Result<()> {
            use notify::{EventKind, RecommendedWatcher, Watcher};
            use std::sync::mpsc;
            let (tx, rx) = mpsc::channel();
            let config = notify::Config::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(true);
            let mut watcher = RecommendedWatcher::new(tx, config)?;
            watcher.watch(&path.absolutize()?, RecursiveMode::Recursive)?;
            while let Ok(ev) = rx.recv() {
                if let Ok(ev) = ev
                    && let EventKind::Modify(_) = ev.kind
                {
                    _ = proxy.send_event(PtyEvent::ConfigChanged);
                }
            }
            Ok(())
        };
        if let Err(e) = func() {
            tracing::error!(?e, "watcher thread error");
        }
    });
}

pub fn load(this: &mut App) {
    let Some(path) = util::config_path() else {
        return;
    };

    _ = util::create_config_if_missing(&path)
        .inspect_err(|e| tracing::error!(%e, "failed to create default config"));

    this.ssh_sessions.clear();
    this.keymap = App::default_keymap();
    this.registered_callbacks.clear();
    let lua = this.lua.clone();
    with_env(this, |_| {
        let chunk = lua.load(path);
        chunk.exec()
    })
    .into_log();
    let action_key = this.action;
    let overridden = matches!(
        this.keymap.get(&action_key),
        Some(KeyAction::Custom(_))
    );
    if !overridden {
        this.keymap
            .retain(|_, v| !matches!(v, KeyAction::Builtin(BuiltinAction::Action)));
        this.keymap.insert(action_key, KeyAction::Builtin(BuiltinAction::Action));
    }
}

pub fn with_env<R>(this: &mut App, f: impl FnOnce(LuaAnyUserData) -> LuaResult<R>) -> Result<R> {
    let lua = this.lua.clone();
    let res = lua.scope(|scope| {
        let app = scope.create_userdata_ref_mut(this)?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyBinding {
    pub mods: ModifiersState,
    pub key: KeyCode,
}

impl KeyBinding {
    /*pub fn new(mods: ModifiersState, key: KeyCode) -> Self {
        Self { mods, key }
    }*/

    pub fn parse(binding: impl AsRef<str>) -> Result<Self> {
        let binding = binding.as_ref();
        let (binding, mods) = Self::parse_mods(binding)?;
        let key = if !mods.is_empty()
            && let Some(delim) = binding.chars().position(|c| c == '-')
        {
            &binding[delim + 1..]
        } else {
            binding
        };
        let key = Self::parse_key(key)?;

        Ok(Self { mods, key })
    }

    fn parse_mods(binding: &str) -> Result<(&str, ModifiersState)> {
        if !binding.starts_with('<') {
            return Ok((binding, ModifiersState::default()));
        }
        let binding = &binding[1..];
        let end = binding
            .chars()
            .position(|c| c == '>')
            .context("failed to find `>` while parsing keybinding modifiers")?;
        let mut modifiers = &binding[..end];

        let mut mods = ModifiersState::default();

        if !modifiers.is_empty() {
            while let Some(next) = modifiers.chars().position(|c| c == '+') {
                let modifier = &modifiers[..next];
                mods.extend(Self::parse_mod(modifier)?);
                modifiers = &modifiers[next + 1..];
            }
            mods.extend(Self::parse_mod(modifiers)?);
        }

        let rest = &binding[end..];

        Ok((rest, mods))
    }

    pub fn parse_mod(m: &str) -> Result<ModifiersState> {
        Ok(match m.trim() {
            "ctrl" => ModifiersState::CONTROL,
            "alt" => ModifiersState::ALT,
            "shift" => ModifiersState::SHIFT,
            "mod" => ModifiersState::SUPER,
            x => {
                anyhow::bail!("`{x}` is not a valid modifier");
            }
        })
    }

    pub fn parse_key(key: &str) -> Result<KeyCode> {
        let key = key.to_lowercase();
        let key = match key.as_str() {
            "a" => KeyCode::KeyA,
            "b" => KeyCode::KeyB,
            "c" => KeyCode::KeyC,
            "d" => KeyCode::KeyD,
            "e" => KeyCode::KeyE,
            "f" => KeyCode::KeyF,
            "g" => KeyCode::KeyG,
            "h" => KeyCode::KeyH,
            "i" => KeyCode::KeyI,
            "j" => KeyCode::KeyJ,
            "k" => KeyCode::KeyK,
            "l" => KeyCode::KeyL,
            "m" => KeyCode::KeyM,
            "n" => KeyCode::KeyN,
            "o" => KeyCode::KeyO,
            "p" => KeyCode::KeyP,
            "q" => KeyCode::KeyQ,
            "r" => KeyCode::KeyR,
            "s" => KeyCode::KeyS,
            "t" => KeyCode::KeyT,
            "u" => KeyCode::KeyU,
            "v" => KeyCode::KeyV,
            "w" => KeyCode::KeyW,
            "x" => KeyCode::KeyX,
            "y" => KeyCode::KeyY,
            "z" => KeyCode::KeyZ,
            "0" => KeyCode::Digit0,
            "1" => KeyCode::Digit1,
            "2" => KeyCode::Digit2,
            "3" => KeyCode::Digit3,
            "4" => KeyCode::Digit4,
            "5" => KeyCode::Digit5,
            "6" => KeyCode::Digit6,
            "7" => KeyCode::Digit7,
            "8" => KeyCode::Digit8,
            "9" => KeyCode::Digit9,
            "up" | "arrowup" => KeyCode::ArrowUp,
            "down" | "arrowdown" => KeyCode::ArrowDown,
            "left" | "arrowleft" => KeyCode::ArrowLeft,
            "right" | "arrowright" => KeyCode::ArrowRight,
            "f1" => KeyCode::F1,
            "f2" => KeyCode::F2,
            "f3" => KeyCode::F3,
            "f4" => KeyCode::F4,
            "f5" => KeyCode::F5,
            "f6" => KeyCode::F6,
            "f7" => KeyCode::F7,
            "f8" => KeyCode::F8,
            "f9" => KeyCode::F9,
            "f10" => KeyCode::F10,
            "f11" => KeyCode::F11,
            "f12" => KeyCode::F12,
            "space" => KeyCode::Space,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Escape,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "semicolon" => KeyCode::Semicolon,
            "comma" => KeyCode::Comma,
            "period" | "dot" => KeyCode::Period,
            "slash" => KeyCode::Slash,
            "backslash" => KeyCode::Backslash,
            "minus" => KeyCode::Minus,
            "equals" | "equal" => KeyCode::Equal,
            "quote" | "apostrophe" => KeyCode::Quote,
            "backquote" | "grave" => KeyCode::Backquote,
            "bracketleft" | "[" => KeyCode::BracketLeft,
            "bracketright" | "]" => KeyCode::BracketRight,
            x => anyhow::bail!("`{x}` is not a valid key"),
        };
        Ok(key)
    }
}

impl FromLua for KeyBinding {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        let binding = value.as_string().context("not a string")?;
        Ok(Self::parse(binding.to_str()?)?)
    }
}
