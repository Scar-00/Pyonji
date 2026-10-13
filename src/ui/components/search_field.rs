//! The field an overlay is typed into, ruled so focus shows on the rule and
//! not only in the caret.

use gpui::{
    App, Entity, Focusable as _, IntoElement, StyleRefinement, Styled, Window, prelude::*, px,
};
use gpui_base::{RoleOverride, StyledExt as _, h_flex};
use gpui_component::{
    Icon, IconName,
    input::{Input, InputState},
};

use crate::ui::theme::PyTheme as _;

/// A bare input with an icon in front of it. Underlined by default; see
/// [`SearchField::boxed`].
///
/// The input reads as presentational because the screen's
/// [`super::editable_combo_box`] carries the field's role and value.
#[derive(IntoElement)]
pub struct SearchField {
    state: Entity<InputState>,
    icon: IconName,
    boxed: bool,
    style: StyleRefinement,
}

impl SearchField {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            icon: IconName::Search,
            boxed: false,
            style: StyleRefinement::default(),
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    /// A ruled box instead of an underline, for a field that shares its row
    /// with other controls.
    pub fn boxed(mut self) -> Self {
        self.boxed = true;
        self
    }
}

impl Styled for SearchField {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for SearchField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let focused = self.state.read(cx).focus_handle(cx).is_focused(window);
        h_flex()
            .w_full()
            .h(px(36.))
            .items_center()
            .gap_2()
            .text_color(theme.text)
            .map(|field| {
                if self.boxed {
                    field.h(px(32.)).px_3().border_1()
                } else {
                    field.border_b_1()
                }
            })
            .border_color(if focused {
                theme.focus_ring
            } else {
                theme.border
            })
            .child(
                Icon::new(self.icon)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(theme.text_muted),
            )
            .child(
                Input::new(&self.state)
                    .role(RoleOverride::Presentational)
                    .flex_1()
                    .bordered(false)
                    .appearance(false),
            )
            .refine_style(&self.style)
    }
}
