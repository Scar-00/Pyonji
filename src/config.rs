use crate::PtyEvent;
use crate::pty::SshConnection;
use anyhow::{Context, Result};
use derive_more::{Deref, DerefMut};
use mlua::{FromLua, prelude::*};
use notify::RecursiveMode;
use winit::keyboard::{KeyCode, ModifiersState};
use std::fmt::Debug;
use std::io::Write;
use std::net::IpAddr;
use std::ops::Deref as _;
use std::path::PathBuf;
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;
use path_absolutize::*;

const DEFAULT_CONFIG: &str = include_str!("../resources/default.lua");

#[derive(Debug, Default, Clone, Deref, DerefMut)]
pub struct Value<T: Debug + Clone + Default>(T);

impl<T: Debug + Clone + Default> Value<T> {
    pub fn value(&self) -> T {
        self.0.clone()
    }
}

impl<T: Debug + Clone + FromLua + Default> FromLua for Value<T> {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        if let Some(v) = value.as_function() {
            v.call::<T>(()).map(|v| Self(v))
        } else {
            T::from_lua(value, lua).map(|v| Self(v))
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    pub font_family: Option<Value<String>>,
    pub font_size: Option<Value<f64>>,
    pub line_height: Option<Value<f64>>,
    pub fullscreen: Option<Value<bool>>,
    pub default_cwd: Option<Value<PathBuf>>,
    ssh_sessions: Value<Vec<SshConnection>>,
    _open_palette: Option<KeyBinding>,
}

impl FromLua for Config {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        let table = value.as_table().context("failed to create table")?;

        let sessions = table.get::<Vec<LuaValue>>("ssh_sessions")?;
        let sessions = sessions.into_iter().map(|session| -> Result<SshConnection> {
            let table = session.as_table().context("ssh_session entry is not a table")?;
            Ok(SshConnection {
                name: table.get("name")?,
                user_name: table.get("user_name")?,
                ip: table.get::<String>("ip").map(|ip| IpAddr::from_str(&ip))??,
            })
        }).collect::<Result<Vec<_>>>()?;

        Ok(Self {
            font_family: table.get("font_family")?,
            font_size: table.get("font_size")?,
            line_height: table.get("line_height")?,
            fullscreen: table.get("fullscreen")?,
            default_cwd: table.get("default_cwd")?,
            ssh_sessions: Value(sessions),
            _open_palette: table.get("open_palette")?,
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
            let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&path)?;
            file.write_all(DEFAULT_CONFIG.as_bytes())?;
        }
        let lua = Lua::new();
        let chunk = lua.load(path);
        let chunk = chunk.into_function()?;
        let res = chunk.call::<Self>(())?;
        Ok(res)
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
        let font_size = self.font_size.as_ref().map_or(24.0, Value::value);
        let line_height = self.line_height.as_ref().map_or(28.0 / 24.0, Value::value);
        (font_size, line_height)
    }

    pub fn font_family(&self) -> Option<&str> {
        self.font_family.as_ref().map(|v| v.deref().as_str())
    }

    pub fn fullscreen(&self) -> bool {
        self.fullscreen.as_ref().is_some_and(Value::value)
    }

    pub fn ssh_sessions(&self) -> Vec<SshConnection> {
        self.ssh_sessions.value()
    }
}

#[derive(Debug, Clone)]
pub struct KeyBinding {
    _mods: ModifiersState,
    _key: KeyCode,
}

impl KeyBinding {
    pub fn new(binding: impl AsRef<str>) -> Result<Self> {
        let binding = binding.as_ref();
        let (binding, mods) = Self::parse_mods(binding)?;
        let key = if !mods.is_empty() && let Some(delim) = binding.chars().position(|c| c == '-') {
            &binding[delim + 1..]
        }else {
            binding
        };
        let key = Self::parse_key(key)?;

        Ok(Self {
            _mods: mods,
            _key: key
        })
    }

    fn parse_mods(binding: &str) -> Result<(&str, ModifiersState)> {
        fn parse_mod(m: &str) -> Result<ModifiersState> {
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

        if !binding.starts_with('<') {
            return Ok((binding, ModifiersState::default()));
        }
        let binding = &binding[1..];
        let end = binding.chars().position(|c| c == '>').context("failed to find `>` while parsing keybinding modifiers")?;
        let mut modifiers = &binding[..end];

        let mut mods = ModifiersState::default();

        while let Some(next) = modifiers.chars().position(|c| c == '+') {
            let modifier = &modifiers[..next];
            mods.extend(parse_mod(modifier)?);
            modifiers = &modifiers[next + 1..];
        }
        mods.extend(parse_mod(modifiers)?);

        let rest = &binding[end..];

        Ok((rest, mods))
    }

    fn parse_key(key: &str) -> Result<KeyCode> {
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
