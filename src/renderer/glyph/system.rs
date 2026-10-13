//! The faces Pyonji takes from the installed fonts: the grid's text faces,
//! from the configured family or a platform default, and a CJK fallback.

use std::{fs::File, sync::OnceLock, thread};

use fontdb::{Database, Family, Query, Source, Stretch, Weight};

use super::Font;

/// Tried in order when no family is configured, or the configured one is not
/// installed.
#[cfg(windows)]
pub const DEFAULT_FAMILIES: &[&str] = &["Cascadia Mono", "Consolas"];
#[cfg(target_os = "macos")]
pub const DEFAULT_FAMILIES: &[&str] = &["Menlo", "Monaco"];
#[cfg(not(any(windows, target_os = "macos")))]
pub const DEFAULT_FAMILIES: &[&str] = &[
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
];

/// The bundled face's family, drawn in when no monospaced font is installed.
pub const BUNDLED_FAMILY: &str = "JetBrainsMono Nerd Font Mono";

/// Hangul first, as that is what the IME preedit is mostly used for.
#[cfg(windows)]
const CJK_FAMILIES: &[&str] = &["Malgun Gothic", "Microsoft YaHei", "Yu Gothic", "MS Gothic"];
#[cfg(target_os = "macos")]
const CJK_FAMILIES: &[&str] = &["Apple SD Gothic Neo", "PingFang SC", "Hiragino Sans"];
#[cfg(not(any(windows, target_os = "macos")))]
const CJK_FAMILIES: &[&str] = &[
    "Noto Sans Mono CJK KR",
    "Noto Sans CJK KR",
    "Noto Sans CJK SC",
    "WenQuanYi Zen Hei",
];

/// The installed fonts, scanned once. The scan takes most of a second, so
/// [`prewarm`] starts it at launch; a lookup made before it has finished
/// waits for it.
static DATABASE: OnceLock<Database> = OnceLock::new();

pub fn prewarm() {
    thread::spawn(database);
}

fn database() -> &'static Database {
    DATABASE.get_or_init(|| {
        let mut database = Database::new();
        database.load_system_fonts();
        database
    })
}

/// The family the grid is drawn in: the configured one if it is installed,
/// else the first installed default, else any monospaced family, else the
/// bundled one.
pub fn resolve_family(configured: Option<&str>) -> String {
    let database = database();
    if let Some(name) = configured
        && let Some(family) = installed_family(database, name)
    {
        return family;
    }
    let fallback = DEFAULT_FAMILIES
        .iter()
        .find_map(|name| installed_family(database, name))
        .or_else(|| {
            database
                .faces()
                .filter(|face| face.monospaced)
                .find_map(|face| face.families.first())
                .map(|(family, _)| family.clone())
        })
        .unwrap_or_else(|| BUNDLED_FAMILY.to_owned());
    if let Some(name) = configured {
        tracing::error!("font family \"{name}\" is not installed, using {fallback}");
    }
    fallback
}

/// The installed spelling of `name`, which config files rarely match in case.
fn installed_family(database: &Database, name: &str) -> Option<String> {
    database
        .faces()
        .flat_map(|face| &face.families)
        .find(|(family, _)| family.eq_ignore_ascii_case(name))
        .map(|(family, _)| family.clone())
}

/// The regular and bold faces of an installed `family`. A family without a
/// bold face gets its closest weight.
pub fn text_faces(family: &str) -> Option<(Font, Font)> {
    Some((face(family, Weight::NORMAL)?, face(family, Weight::BOLD)?))
}

/// The first installed CJK family's regular face.
pub fn cjk_face() -> Option<Font> {
    let database = database();
    CJK_FAMILIES
        .iter()
        .find_map(|name| installed_family(database, name))
        .and_then(|family| face(&family, Weight::NORMAL))
}

fn face(family: &str, weight: Weight) -> Option<Font> {
    let database = database();
    let id = database.query(&Query {
        families: &[Family::Name(family)],
        weight,
        stretch: Stretch::Expanded,
        ..Default::default()
    })?;
    let (source, index) = database.face_source(id)?;
    let font = match source {
        Source::File(path) => {
            let file = File::open(&path)
                .inspect_err(|error| tracing::error!(%error, ?path, "failed to open font"))
                .ok()?;
            // SAFETY: font files are not expected to change while they are
            // installed; fontdb maps them the same way.
            let map = unsafe { memmap2::Mmap::map(&file) }
                .inspect_err(|error| tracing::error!(%error, ?path, "failed to map font"))
                .ok()?;
            Font::from_data(map, index as usize)
        }
        Source::Binary(data) | Source::SharedFile(_, data) => {
            Font::from_shared(data, index as usize)
        }
    };
    if font.is_none() {
        tracing::error!(family, "failed to parse font");
    }
    font
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::TerminalRenderer;

    #[test]
    fn families_match_regardless_of_case() {
        let mut database = Database::new();
        database.load_font_data(TerminalRenderer::ICON_FONT.to_vec());
        let installed = installed_family(&database, "jetbrainsmono nerd font mono");
        assert_eq!(installed.as_deref(), Some(BUNDLED_FAMILY));
        assert_eq!(installed_family(&database, "Iosevka"), None);
    }
}
