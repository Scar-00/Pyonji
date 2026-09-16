pub mod entity;

use gpui::{App, IntoElement, Pixels, div, prelude::*, px};
use gpui_component::{h_flex, v_flex};

use crate::{
    theme::{self, role},
    ui::completion_menu::completion_menu,
};

/// Status bar height in logical pixels: `status_height` rows of `line_height`.
pub fn bar_height_px(status_height: f32, line_height: f32) -> f32 {
    (status_height.max(0.5) * line_height).max(1.0)
}

pub fn height_pixels(status_height: f32, line_height: f32) -> Pixels {
    px(bar_height_px(status_height, line_height))
}

pub use entity::StatusBar;

/// Snapshot of everything the status bar needs to paint.
///
/// Command, rename and Lua prompts all render a gpui-component input
/// (`Input` for command/rename, `Editor` for Lua) inside `editor_slot`;
/// the bar only adds the completion menu and sizing.
pub enum StatusBarModel {
    Hidden,
    Tabs {
        height: Pixels,
        tabs: Vec<(String, bool)>,
        message: Option<String>,
    },
    Command {
        height: Pixels,
        completions: Vec<String>,
        completion_selected: usize,
        editor_slot: gpui::AnyElement,
    },
    Rename {
        height: Pixels,
        editor_slot: gpui::AnyElement,
    },
    Lua {
        height: Pixels,
        matches: Vec<String>,
        /// Pre-wired key-context wrapper (actions bound by Surface).
        editor_slot: gpui::AnyElement,
    },
}

pub fn render(model: StatusBarModel) -> impl IntoElement {
    match model {
        StatusBarModel::Hidden => div().into_any_element(),
        StatusBarModel::Tabs {
            height,
            tabs,
            message,
        } => render_tabs(height, tabs, message).into_any_element(),
        StatusBarModel::Command {
            height,
            completions,
            completion_selected,
            editor_slot,
        } => render_command(height, completions, completion_selected, editor_slot)
            .into_any_element(),
        StatusBarModel::Rename {
            height,
            editor_slot,
        } => render_rename(height, editor_slot).into_any_element(),
        StatusBarModel::Lua {
            height,
            matches,
            editor_slot,
        } => render_lua(height, matches, editor_slot).into_any_element(),
    }
}

fn render_tabs(
    height: Pixels,
    tabs: Vec<(String, bool)>,
    message: Option<String>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .text_sm()
        .px(theme::space::_1)
        .h(height)
        .rounded(theme::radius::SM)
        .items_center()
        .bg(role::bar_bg())
        .children(tabs.into_iter().map(|(label, is_active)| {
            div()
                .px(theme::space::_2)
                .py(theme::space::_0_5)
                .mr(theme::space::_1)
                .rounded(theme::radius::SM)
                .when(is_active, |this| {
                    this.bg(role::accent()).text_color(role::accent_fg())
                })
                .when(!is_active, |this| {
                    this.bg(role::surface()).text_color(role::text())
                })
                .child(label)
        }))
        .when_some(message, |this, message| {
            this.child(
                div()
                    .ml_auto()
                    .px(theme::space::_2)
                    .py(theme::space::_0_5)
                    .rounded(theme::radius::SM)
                    .bg(role::message())
                    .text_color(role::message_fg())
                    .child(message),
            )
        })
}

fn render_command(
    height: Pixels,
    completions: Vec<String>,
    completion_selected: usize,
    editor_slot: gpui::AnyElement,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .text_sm()
        .child(completion_menu(&completions, completion_selected))
        .child(
            div()
                .w_full()
                .h(height)
                .flex()
                .flex_col()
                .justify_center()
                .child(editor_slot),
        )
}

fn render_rename(height: Pixels, editor_slot: gpui::AnyElement) -> impl IntoElement {
    v_flex().w_full().text_sm().child(
        div()
            .w_full()
            .h(height)
            .flex()
            .flex_col()
            .justify_center()
            .child(editor_slot),
    )
}

fn render_lua(
    height: Pixels,
    matches: Vec<String>,
    editor_slot: gpui::AnyElement,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .text_sm()
        .child(completion_menu(&matches, 0))
        .child(
            div()
                .w_full()
                .h(height)
                .flex()
                .flex_col()
                .justify_center()
                .child(editor_slot),
        )
}

#[allow(dead_code)]
fn _app(_: &App) {}
