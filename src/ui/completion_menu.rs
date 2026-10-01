//! Compact Lua suggestions with a separate reading area for the selected item.

use crate::PyTheme as _;
use async_lsp::lsp_types::{CompletionItem, CompletionItemKind as Kind, Documentation, MarkupKind};
use gpui::{prelude::*, *};
use gpui_base::{ScrollbarAxis, h_flex, v_flex};
use gpui_component::{scroll::ScrollableElement, text::TextView};
use std::rc::Rc;

const ROW_HEIGHT: f32 = 32.0;
const HEADER_HEIGHT: f32 = 28.0;
const FOOTER_HEIGHT: f32 = 32.0;
type Accept = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub(super) struct CompletionMenu {
    items: Vec<CompletionItem>,
    selected: Option<usize>,
    fragment: String,
    scroll_handle: UniformListScrollHandle,
    on_accept: Accept,
}

impl CompletionMenu {
    pub(super) fn new(
        items: Vec<CompletionItem>,
        selected: Option<usize>,
        fragment: String,
        scroll_handle: UniformListScrollHandle,
        on_accept: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            items,
            selected,
            fragment,
            scroll_handle,
            on_accept: Rc::new(on_accept),
        }
    }

    fn render_item(
        item: &CompletionItem,
        index: usize,
        selected: bool,
        fragment: &str,
        on_accept: Accept,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (symbol, kind) = kind_label(item.kind);
        let highlights = match_prefix(&item.label, fragment).map(|range| {
            (
                range,
                HighlightStyle {
                    color: Some(rgb_to_hsla(theme.accent)),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            )
        });
        let deprecated = item.deprecated == Some(true)
            || item.tags.as_ref().is_some_and(|tags| {
                tags.contains(&async_lsp::lsp_types::CompletionItemTag::DEPRECATED)
            });
        h_flex()
            .id(("lua-completion", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .gap_2()
            .pr_3()
            .border_l_2()
            .border_color(if selected {
                theme.accent
            } else {
                theme.accent.opacity(0.0)
            })
            .when(selected, |row| row.bg(theme.accent.opacity(0.12)))
            .when(!selected, |row| row.hover(|style| style.bg(theme.hovered)))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                // Keep the input focused until the insertion click is delivered.
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_accept(index, window, cx);
            })
            .child(
                div()
                    .w(px(30.0))
                    .flex_shrink_0()
                    .text_center()
                    .text_color(if selected {
                        theme.accent
                    } else {
                        theme.text_muted
                    })
                    .child(symbol),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(if deprecated {
                                theme.text_muted
                            } else {
                                theme.text
                            })
                            .when(deprecated, |label| label.line_through())
                            .child(StyledText::new(item.label.clone()).with_highlights(highlights)),
                    )
                    .when_some(
                        item.label_details
                            .as_ref()
                            .and_then(|details| details.detail.clone()),
                        |row, detail| {
                            row.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.text_muted)
                                    .child(detail),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(kind),
            )
    }

    fn render_details(
        index: usize,
        item: Option<&CompletionItem>,
        width: Pixels,
        height: Pixels,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        v_flex()
            .id("lua-completion-details")
            .occlude()
            .w(width)
            .h(height)
            .flex_none()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface.opacity(0.15))
            .backdrop_blur(px(24.0))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                v_flex()
                    .id(("lua-completion-documentation", index))
                    .size_full()
                    .p_3()
                    .gap_2()
                    .when_some(item, |details, item| {
                        details
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(item.label.clone()),
                            )
                            .when_some(
                                item.detail.as_ref().filter(|detail| {
                                    let detail = detail.trim();
                                    !detail.is_empty()
                                        && !(detail == "function"
                                            && matches!(
                                                item.kind,
                                                Some(
                                                    Kind::FUNCTION
                                                        | Kind::METHOD
                                                        | Kind::CONSTRUCTOR
                                                )
                                            ))
                                }),
                                |details, detail| {
                                    details
                                        .child(div().text_color(theme.text).child(detail.clone()))
                                },
                            )
                            .when_some(
                                item.label_details
                                    .as_ref()
                                    .and_then(|details| details.description.clone()),
                                |details, description| {
                                    details.child(
                                        div().text_color(theme.text_muted).child(description),
                                    )
                                },
                            )
                            .when_some(item.documentation.as_ref(), |details, documentation| {
                                let content = match documentation {
                                    Documentation::MarkupContent(content)
                                        if content.kind == MarkupKind::Markdown =>
                                    {
                                        TextView::markdown(
                                            "lua-completion-markdown",
                                            content.value.clone(),
                                        )
                                        .into_any_element()
                                    }
                                    Documentation::MarkupContent(content) => {
                                        div().child(content.value.clone()).into_any_element()
                                    }
                                    Documentation::String(content) => {
                                        div().child(content.clone()).into_any_element()
                                    }
                                };
                                details.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme.text_muted)
                                        .child(content),
                                )
                            })
                    })
                    .overflow_y_scrollbar(),
            )
    }
}

impl RenderOnce for CompletionMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let mono = gpui_component::ActiveTheme::theme(cx)
            .mono_font_family
            .clone();
        let available_width = (window.viewport_size().width - px(16.0)).max(px(0.0));
        // Keep both popovers inside the window. In narrow windows the list
        // takes priority; documentation stays to the right whenever it fits.
        let room_for_details = available_width >= px(640.0);
        let width = if room_for_details {
            (available_width * 0.55).min(px(520.0))
        } else {
            available_width.min(px(520.0))
        };
        let compact = width < px(360.0);
        let available = (f32::from(window.viewport_size().height) - 100.0)
            .max(ROW_HEIGHT + HEADER_HEIGHT + FOOTER_HEIGHT);
        let selected_item = self.selected.and_then(|index| self.items.get(index));
        let show_details = room_for_details && selected_item.is_some_and(has_details);
        let list_height = (self.items.len().min(8) as f32 * ROW_HEIGHT)
            .min(available - HEADER_HEIGHT - FOOTER_HEIGHT - 2.0);
        let count = self.items.len();
        let position = self
            .selected
            .filter(|index| *index < count)
            .map(|index| index + 1)
            .unwrap_or(0);
        let details = Self::render_details(
            self.selected.unwrap_or(0),
            selected_item,
            (available_width - width - px(8.0))
                .min(px(400.0))
                .max(px(0.0)),
            // A single matching row still needs a useful reading area.
            px((list_height + HEADER_HEIGHT + FOOTER_HEIGHT + 2.0)
                .max(240.0)
                .min(available)),
            cx,
        );
        let Self {
            items,
            selected,
            fragment,
            scroll_handle,
            on_accept,
        } = self;
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .offset(point(px(0.0), px(-8.0)))
                .snap_to_window_with_margin(px(8.0))
                .child(
                    h_flex()
                        .items_end()
                        .gap_2()
                        .flex_none()
                        .font_family(mono)
                        .text_size(px(13.0))
                        .text_color(theme.text)
                        .child(
                            v_flex()
                                .id("lua-completion-menu")
                                .occlude()
                                .w(width)
                                .flex_none()
                                .overflow_hidden()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.surface.opacity(0.15))
                                .backdrop_blur(px(24.0))
                                .text_color(theme.text)
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .child(
                                    h_flex()
                                        .h(px(HEADER_HEIGHT))
                                        .flex_shrink_0()
                                        .px_3()
                                        .items_center()
                                        .justify_between()
                                        .text_size(px(11.0))
                                        .text_color(theme.text_muted)
                                        .child("Lua suggestions")
                                        .child(format!("{position} / {count}")),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .h(px(list_height))
                                        .flex_shrink_0()
                                        .child(
                                            uniform_list(
                                                "completion-items-list",
                                                count,
                                                move |range, _, cx| {
                                                    range
                                                        .map(|index| {
                                                            Self::render_item(
                                                                &items[index],
                                                                index,
                                                                selected == Some(index),
                                                                &fragment,
                                                                on_accept.clone(),
                                                                cx,
                                                            )
                                                        })
                                                        .collect()
                                                },
                                            )
                                            .w_full()
                                            .h(px(list_height))
                                            .flex_shrink_0()
                                            .track_scroll(&scroll_handle),
                                        )
                                        .scrollbar(&scroll_handle, ScrollbarAxis::Vertical),
                                )
                                .child(
                                    h_flex()
                                        .h(px(FOOTER_HEIGHT))
                                        .flex_shrink_0()
                                        .px_3()
                                        .items_center()
                                        .gap_4()
                                        .border_t_1()
                                        .border_color(theme.border)
                                        .when(!compact, |footer| {
                                            footer.child(hint("↑ ↓", "select", cx))
                                        })
                                        .child(hint("Tab", "insert", cx))
                                        .child(hint("Esc", "dismiss", cx)),
                                ),
                        )
                        .when(show_details, |popovers| popovers.child(details)),
                ),
        )
        .priority_auto()
        .into_any_element()
    }
}

fn has_details(item: &CompletionItem) -> bool {
    item.detail
        .as_ref()
        .is_some_and(|detail| !detail.trim().is_empty())
        || item.documentation.is_some()
        || item
            .label_details
            .as_ref()
            .is_some_and(|details| details.description.is_some())
}

fn match_prefix(label: &str, fragment: &str) -> Option<std::ops::Range<usize>> {
    if fragment.is_empty() {
        return None;
    }
    let end = label
        .char_indices()
        .nth(fragment.chars().count())
        .map_or(label.len(), |(index, _)| index);
    (label[..end].to_lowercase() == fragment.to_lowercase()).then_some(0..end)
}

fn hint(key: &'static str, action: &'static str, cx: &App) -> impl IntoElement + use<> {
    h_flex()
        .items_center()
        .gap_1p5()
        .text_size(px(11.0))
        .child(div().text_color(cx.theme().text).child(key))
        .child(div().text_color(cx.theme().text_muted).child(action))
}

fn kind_label(kind: Option<Kind>) -> (&'static str, &'static str) {
    match kind {
        Some(Kind::METHOD) => ("ƒ", "Method"),
        Some(Kind::FUNCTION) => ("ƒ", "Function"),
        Some(Kind::CONSTRUCTOR) => ("ƒ", "Constructor"),
        Some(Kind::FIELD) => ("·", "Field"),
        Some(Kind::VARIABLE) => ("x", "Variable"),
        Some(Kind::CLASS) => ("T", "Class"),
        Some(Kind::INTERFACE) => ("T", "Interface"),
        Some(Kind::MODULE) => ("{}", "Module"),
        Some(Kind::PROPERTY) => ("·", "Property"),
        Some(Kind::UNIT) => ("#", "Unit"),
        Some(Kind::VALUE) => ("=", "Value"),
        Some(Kind::ENUM) => ("T", "Enum"),
        Some(Kind::KEYWORD) => ("k", "Keyword"),
        Some(Kind::SNIPPET) => ("↳", "Snippet"),
        Some(Kind::COLOR) => ("#", "Color"),
        Some(Kind::FILE) => ("/", "File"),
        Some(Kind::REFERENCE) => ("&", "Reference"),
        Some(Kind::FOLDER) => ("/", "Folder"),
        Some(Kind::ENUM_MEMBER) => ("=", "Enum member"),
        Some(Kind::CONSTANT) => ("=", "Constant"),
        Some(Kind::STRUCT) => ("T", "Struct"),
        Some(Kind::EVENT) => ("~", "Event"),
        Some(Kind::OPERATOR) => ("+", "Operator"),
        Some(Kind::TYPE_PARAMETER) => ("T", "Type parameter"),
        _ => ("·", "Text"),
    }
}

#[cfg(test)]
mod tests {
    use super::match_prefix;

    #[test]
    fn prefix_highlights_use_utf8_boundaries_and_ignore_case() {
        assert_eq!(match_prefix("Print", "pr"), Some(0..2));
        assert_eq!(match_prefix("écrire", "Éc"), Some(0..3));
        assert_eq!(match_prefix("한글", "한"), Some(0..3));
        assert_eq!(match_prefix("print", "printf"), None);
        assert_eq!(match_prefix("print", "int"), None);
        assert_eq!(match_prefix("print", ""), None);
    }
}
