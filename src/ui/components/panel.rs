//! The frame every overlay screen shares: a header strip, a body that is
//! either a list or an [`EmptyState`], and a footer strip of hints.

use gpui::{
    AnyElement, App, Div, IntoElement, ParentElement, Rgba, SharedString, Styled, Window, div,
    prelude::*, px,
};
use gpui_base::{h_flex, v_flex};

use crate::ui::theme::PyTheme as _;

/// The strip along the top of an overlay: a title row, then a field.
pub fn panel_header() -> Div {
    v_flex().flex_shrink_0().gap_2().px_4().pt_3().pb_2()
}

/// The strip along the bottom of an overlay. Lead with a
/// [`footer_status`], then the [`super::hint`]s for the keys that work.
pub fn panel_footer(cx: &App) -> Div {
    h_flex()
        .min_h(px(40.))
        .py_2()
        .flex_wrap()
        .flex_shrink_0()
        .items_center()
        .gap_4()
        .px_4()
        .border_t_1()
        .border_color(cx.theme().border)
}

/// The footer's leading slot: what enter will do now, or what went wrong.
/// Every screen answers that in the same place.
pub fn footer_status(text: impl IntoElement, color: Rgba) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(px(12.))
        .text_color(color)
        .child(text)
}

/// What sits where a list would be when there is nothing to list: what
/// happened, then what to do about it, then anything to do it with.
#[derive(IntoElement)]
pub struct EmptyState {
    heading: SharedString,
    detail: Option<SharedString>,
    tone: Option<Rgba>,
    icon: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl EmptyState {
    pub fn new(heading: impl Into<SharedString>) -> Self {
        Self {
            heading: heading.into(),
            detail: None,
            tone: None,
            icon: None,
            children: Vec::new(),
        }
    }

    /// A line under the heading, saying what to do next.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The heading's colour. Plain text by default.
    pub fn tone(mut self, color: Rgba) -> Self {
        self.tone = Some(color);
        self
    }

    /// A quiet heading, for states that are a pause rather than news:
    /// loading, nothing selected.
    pub fn muted(mut self, cx: &App) -> Self {
        self.tone = Some(cx.theme().text_muted);
        self
    }

    /// A mark above the heading: an icon, a spinner.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }
}

impl ParentElement for EmptyState {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .children(self.icon)
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(self.tone.unwrap_or(theme.text))
                    .child(self.heading),
            )
            .when_some(self.detail, |this, detail| {
                this.child(
                    div()
                        .max_w(px(440.))
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .child(detail),
                )
            })
            .children(self.children)
    }
}
