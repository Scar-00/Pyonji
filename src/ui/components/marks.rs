//! Small marks the screens use to say the same things the same way.

use gpui::{App, Div, IntoElement, SharedString, Styled, div, prelude::*, px};
use gpui_base::h_flex;

use super::PyStyled as _;
use crate::ui::theme::{PyTheme as _, Theme};

/// One key and what it does, the way every footer spells it: the key in the
/// grid's face, the action muted and lower case.
pub fn hint(keys: impl Into<SharedString>, action: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1p5()
        .text_size(px(11.0))
        .child(div().mono(cx).text_color(theme.text).child(keys.into()))
        .child(div().text_color(theme.text_muted).child(action.into()))
}

/// How much of a list is showing, the way every screen's header counts it:
/// "18 commands" until a query narrows it, then "4 / 18".
pub fn counter(shown: usize, total: usize, noun: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let counter = h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .mono(cx)
        .text_size(px(11.0))
        .text_color(theme.text_muted);
    if shown == total {
        let plural = if total == 1 { "" } else { "s" };
        counter.child(format!("{total} {noun}{plural}"))
    } else {
        counter
            .child(format!("{shown}"))
            .child(div().text_color(theme.text_disabled).child("/"))
            .child(format!("{total}"))
    }
}

/// A short upright hairline between items on one line.
pub fn rule(cx: &App) -> impl IntoElement + use<> {
    div()
        .w(px(1.0))
        .h(px(14.0))
        .flex_shrink_0()
        .bg(cx.theme().border)
}

/// The ⏎ in a list row's trailing gutter. The gutter stays empty until the
/// row is selected, so a mark in it means enter will do this.
pub fn enter_mark(selected: bool, cx: &App) -> Div {
    h_flex()
        .w(px(18.0))
        .flex_shrink_0()
        .justify_end()
        .mono(cx)
        .text_size(px(12.0))
        .text_color(if selected {
            cx.theme().accent
        } else {
            Theme::TRANSPARENT
        })
        .child("\u{23ce}")
}
