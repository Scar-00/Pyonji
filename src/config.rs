use crate::PtyEvent;
use anyhow::{Context, Result};
use mlua::{FromLua, prelude::*};
use notify::RecursiveMode;
use std::fmt::Debug;
use std::path::PathBuf;
use std::thread;
use winit::event_loop::EventLoopProxy;

#[derive(Debug, Clone)]
pub struct Value<T: Debug + Clone>(T);

impl<T: Debug + Clone + FromLua> Value<T> {
    pub fn value(&self) -> T {
        self.0.clone()
    }
}

impl<T: Debug + Clone + FromLua> FromLua for Value<T> {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        if let Some(v) = value.as_function() {
            v.call::<T>(()).map(|v| Self(v))
        } else {
            T::from_lua(value, lua).map(|v| Self(v))
        }
    }
}

#[derive(Debug, Clone)]
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
    pub fn load(path: PathBuf) -> Result<Self> {
        let lua = Lua::new();
        let chunk = lua.load(path);
        let chunk = chunk.into_function()?;
        let res = chunk.call::<Self>(())?;
        Ok(res)
    }

    pub fn watch(path: PathBuf, proxy: EventLoopProxy<PtyEvent>) {
        let config_path = path.clone();
        thread::spawn(move || {
            use notify::{Event, EventKind, RecommendedWatcher, Watcher};
            let config = notify::Config::default();
            let Ok(mut watcher) = RecommendedWatcher::new(
                move |ev: notify::Result<Event>| {
                    if let Ok(ev) = ev
                        && let EventKind::Modify(_) = ev.kind
                        && let Ok(config) = Self::load(config_path.clone())
                    {
                        _ = proxy.send_event(PtyEvent::ConfigChanged(config));
                    }
                },
                config,
            ) else {
                return;
            };
            _ = watcher.watch(&path, RecursiveMode::NonRecursive);
        });
    }

    pub fn font_metrics(&self) -> (f64, f64) {
        let font_size = self.font_size.as_ref().map_or(24.0, Value::value);
        let line_height = self
            .line_height
            .as_ref()
            .map_or(28.0 / 24.0, Value::value);
        (font_size, line_height)
    }
}
