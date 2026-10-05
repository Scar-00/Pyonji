use anyhow::Result;
use gpui::{AnyElement, App, AssetSource, IntoElement, RenderOnce, SharedString, Window};
use gpui_component::{Icon, IconNamed};
use gpui_component_macros::icon_named;
use std::borrow::Cow;

/// Combines multiple [`AssetSource`]s into one, tried in order.
///
/// Mirrors `t3chat/crates/assets`: first source wins on `load`, while `list`
/// merges the results of every source so e.g. the bundled
/// `gpui-component` icons and Pyonji's own `assets/icons` are all visible.
pub struct GlobalAssets {
    sources: Vec<Box<dyn AssetSource>>,
}

impl GlobalAssets {
    pub fn new<const N: usize>(assets: [Box<dyn AssetSource>; N]) -> Self {
        let mut sources: Vec<Box<dyn AssetSource>> = vec![];
        sources.extend(assets);
        Self { sources }
    }
}

impl AssetSource for GlobalAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        for source in &self.sources {
            let Ok(res) = source.load(path) else {
                continue;
            };
            if res.is_some() {
                return Ok(res);
            }
        }
        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(self
            .sources
            .iter()
            .filter_map(|source| source.list(path).ok())
            .flatten()
            .collect())
    }
}

icon_named!(PyonjiAsset, "assets/icons", [Copy]);

#[derive(rust_embed::RustEmbed, Default)]
#[folder = "assets/"]
#[include = "icons/**/*.svg"]
pub struct PyonjiAssetsSource;

impl AssetSource for PyonjiAssetsSource {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow::anyhow!("could not find asset at path \"{}\"", path))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

impl From<PyonjiAsset> for AnyElement {
    fn from(value: PyonjiAsset) -> Self {
        Icon::new(value).into_any_element()
    }
}

impl RenderOnce for PyonjiAsset {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Icon::new(self)
    }
}
