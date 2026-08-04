use crate::overlay::{LuaAction, Screen};
use crate::pty::SshConnection;
use crate::terminal::{SplitDirection, Tab};
use crate::{App, PtyEvent, ResultExt};
use anyhow::{Context, Result};
use mlua::{FromLua, prelude::*};
use notify::RecursiveMode;
use path_absolutize::*;
use std::fmt::Debug;
use std::io::Write;
use std::net::IpAddr;
use std::path::{PathBuf};
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::{KeyCode, ModifiersState};

const DEFAULT_CONFIG: &str = include_str!("../resources/default.lua");
pub const LUA_MODULES: &[&str] = &[include_str!("../resources/lua/keybind.lua")];

macro_rules! apply {
    ($this: ident.$field: ident, $table: expr) => {
        if let Ok(value) = $table.get(stringify!($field)).inspect_err(|e| tracing::error!(%e, "failed to get field")) {
            $this.$field = value;
        }
    };
    ($this: ident.$field: ident, $table: expr, $transformer: expr) => {
        if let Ok(value) = $table.get(stringify!($field)).map($transformer).inspect_err(|e| tracing::error!(%e, "failed to get field")) {
            $this.$field = value;
        }
    };
}

macro_rules! callable_action {
    ($lua: ident, $this: ident => $body: expr) => {{
        if let Some(this) = $this {
            this.borrow_mut_scoped($body)??;
        }
        $lua.create_function(move |_, this: LuaAnyUserData| {
            this.borrow_mut_scoped($body)?
        })
    }}
}

macro_rules! args {
    ($args: ident, $lua: ident, $typ: ty) => {{
        let mut args = $args;
        let first = args[0].clone();
        let this = if first.is_userdata() {
            args.pop_front();
            Some(LuaAnyUserData::from_lua(first, $lua)?)
        } else {
            None
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
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.update_cmds(&self.ssh_sessions, &self.registered_callbacks);
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
                this.key_bindings
                    .push((KeyBinding { mods: state, key }, func.clone()));
            } else {
                let (binding, func): (KeyBinding, LuaFunction) =
                    FromLuaMulti::from_lua_multi(args, lua)?;
                this.key_bindings.push((binding, func));
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
        methods.add_function(
            "create_session",
            |lua, args: LuaMultiValue| {
                let (this, (_, tab, _, _)) = args!(args, lua, (Option<String>, Option<usize>, Option<String>,
                    Option<u64>));
                callable_action!(lua, this => move |this: &mut Self| -> LuaResult<()> {
                    let session = this.session_manager.create_session(this.terminal_rows().max(1), this.cols.max(1), None)?;
                    let tab = if let Some(tab) = tab {
                        &mut this.tabs[tab]
                    }else {
                        &mut this.tabs[this.current_tab]
                    };

                    //let dir = dir.as_deref().map(Path::new).or(this.args.path.as_deref().or(this.default_cwd.as_deref()));

                    if let Some(tab) = tab {
                        tab.split_active(SplitDirection::Vertical, session);
                    }else {
                        *tab = Some(Tab::new(session));
                    }
                    Ok(())
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
    this.key_bindings.clear();
    this.registered_callbacks.clear();
    let lua = this.lua.clone();
    with_env(this, |_| {
        let chunk = lua.load(path);
        chunk.exec()
    })
    .into_log();
}

pub fn with_env<R>(this: &mut App, f: impl FnOnce(LuaAnyUserData) -> LuaResult<R>) -> Result<()> {
    let lua = this.lua.clone();
    lua.scope(|scope| {
        let app = scope.create_userdata_ref_mut(this)?;
        lua.globals().set("py", app.clone())?;
        let ret = f(app);
        lua.globals().remove("py")?;
        ret
    })?;
    Ok(())
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

/*#[derive(Debug, Clone)]
pub struct Config {
    pub font_family: Option<String>,
    pub font_size: f64,
    pub line_height: f64,
    pub fullscreen: bool,
    pub default_cwd: Option<PathBuf>,
    ssh_sessions: Vec<SshConnection>,
    _open_palette: KeyBinding,
}

impl FromLua for Config {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        let table = value.as_table().context("failed to create table")?;
        let sessions = table
            .get::<Vec<LuaValue>>("ssh_sessions")
            .and_then(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| -> LuaResult<SshConnection> {
                        let table = session
                            .as_table()
                            .context("ssh_session entry is not a table")?;
                        Ok(SshConnection {
                            name: table.get("name").and_then(|v| Self::from_value(v, lua))?,
                            user_name: table
                                .get("user_name")
                                .and_then(|v| Self::from_value(v, lua))?,
                            ip: table
                                .get::<LuaValue>("ip")
                                .and_then(|v| Self::from_value(v, lua))
                                .map(|ip: String| IpAddr::from_str(&ip))??,
                        })
                    })
                    .collect::<LuaResult<Vec<_>>>()
            })
            .unwrap_or_default();

        Ok(Self {
            font_family: table
                .get("font_family")
                .and_then(|v| Self::from_value(v, lua))?,
            font_size: table
                .get("font_size")
                .and_then(|v| Self::from_value(v, lua))
                .unwrap_or(24.0),
            line_height: table
                .get("line_height")
                .and_then(|v| Self::from_value(v, lua))
                .unwrap_or(28.0 / 24.0),
            fullscreen: table
                .get("fullscreen")
                .and_then(|v| Self::from_value(v, lua))?,
            default_cwd: table
                .get("default_cwd")
                .and_then(|v| Self::from_value(v, lua))?,
            ssh_sessions: sessions,
            _open_palette: table
                .get("open_palette")
                .and_then(|v| Self::from_value(v, lua))
                .unwrap_or(KeyBinding::OPEN_PALETTE),
        })
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::path().context("not config path")?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)?;
            file.write_all(DEFAULT_CONFIG.as_bytes())?;
        }
        let lua = Lua::new();
        let chunk = lua.load(path);
        let chunk = chunk.into_function()?;
        let res = chunk.call::<Self>(())?;
        Ok(res)
    }

    pub fn new() -> Self {
        Self {
            font_family: None,
            font_size: 24.0,
            line_height: 28.0 / 24.0,
            fullscreen: false,
            default_cwd: None,
            ssh_sessions: vec![],
            _open_palette: KeyBinding::OPEN_PALETTE,
        }
    }

    pub fn watch(proxy: EventLoopProxy<PtyEvent>) {
        let Some(path) = Self::path() else {
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
                        && let Ok(config) = Self::load()
                    {
                        _ = proxy.send_event(PtyEvent::ConfigChanged(config));
                    }
                }
                Ok(())
            };
            if let Err(e) = func() {
                tracing::error!(?e, "watcher thread error");
            }
        });
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn path() -> Option<PathBuf> {
        cfg_select! {
            feature = "install" => {
                dirs::config_local_dir().map(|dir| dir.join("pyonji").join("init.lua"))
            }
            _ => Some("init.lua".into())
        }
    }

    pub fn font_metrics(&self) -> (f64, f64) {
        (self.font_size, self.line_height)
    }

    pub fn font_family(&self) -> Option<&str> {
        self.font_family.as_deref()
    }

    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    pub fn ssh_sessions(&self) -> Vec<SshConnection> {
        self.ssh_sessions.clone()
    }


}*/

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyBinding {
    pub mods: ModifiersState,
    pub key: KeyCode,
}

impl KeyBinding {
    pub fn new(binding: impl AsRef<str>) -> Result<Self> {
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

    /*const fn new_const(mods: ModifiersState, key: KeyCode) -> Self {
        Self { mods, key }
    }*/

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

        while let Some(next) = modifiers.chars().position(|c| c == '+') {
            let modifier = &modifiers[..next];
            mods.extend(Self::parse_mod(modifier)?);
            modifiers = &modifiers[next + 1..];
        }
        mods.extend(Self::parse_mod(modifiers)?);

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
        let key = match key.to_lowercase().as_str() {
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
            x => anyhow::bail!("`{x}` is not a valid key"),
        };
        Ok(key)
    }
}

impl FromLua for KeyBinding {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        let binding = value.as_string().context("not a string")?;
        Ok(Self::new(binding.to_str()?)?)
    }
}
