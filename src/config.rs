use crate::PtyEvent;
use anyhow::{Context, Result};
use mlua::{FromLua, prelude::*};
use notify::RecursiveMode;
use std::fmt::Debug;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Default, Clone)]
pub struct Value<T: Debug + Clone + Default>(T);

impl<T: Debug + Clone + FromLua + Default> Value<T> {
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
}

impl FromLua for Config {
    fn from_lua(value: LuaValue, _: &Lua) -> LuaResult<Self> {
        let table = value.as_table().context("failed to create table")?;

        Ok(Self {
            font_family: table.get("font_family")?,
            font_size: table.get("font_size")?,
            line_height: table.get("line_height")?,
        })
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = Self::path().context("not config path")?;
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
                watcher.watch(&dbg!(path.absolute()?), RecursiveMode::Recursive)?;
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

    pub fn path() -> Option<PathBuf> {
        Some("init.lua".into())
        //dirs::config_local_dir().map(|dir| dir.join("pyonji").join("init.lua"))
    }

    pub fn font_metrics(&self) -> (f64, f64) {
        let font_size = self.font_size.as_ref().map_or(24.0, Value::value);
        let line_height = self.line_height.as_ref().map_or(28.0 / 24.0, Value::value);
        (font_size, line_height)
    }
}
