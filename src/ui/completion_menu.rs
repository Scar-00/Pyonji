//! Compact Lua suggestions with a separate reading area for the selected item.

use super::components::{PyStyled as _, hint};
use super::signature_help::SignaturePopover;
use super::theme::{PyTheme as _, Theme};
use async_lsp::lsp_types::SignatureHelp;
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
    signature_help: Option<SignatureHelp>,
    callee: Option<SharedString>,
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
            signature_help: None,
            callee: None,
        }
    }

    pub(super) fn signature_help(mut self, help: Option<SignatureHelp>) -> Self {
        self.signature_help = help;
        self
    }

    /// The name of the function being called, for the signature help header.
    pub(super) fn callee(mut self, callee: Option<SharedString>) -> Self {
        self.callee = callee;
        self
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
        let (symbol, kind) = kind_label(effective_kind(item));
        let symbol_color = kind_color(effective_kind(item), theme);
        let highlights =
            match_prefix(&item.label, fragment).map(|range| (range, theme.match_highlight()));
        let deprecated = item.deprecated == Some(true)
            || item.tags.as_ref().is_some_and(|tags| {
                tags.contains(&async_lsp::lsp_types::CompletionItemTag::DEPRECATED)
            });
        h_flex()
            .id(("lua-completion", index))
            .aria_label(format!("Insert {} ({kind})", item.label))
            .role(accesskit::Role::ListBoxOption)
            .aria_selected(selected)
            .when(selected, |row| row.aria_active_descendant())
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .gap_2()
            .pr_3()
            .border_l_2()
            // Not `list_row`: these rows sit on glass.
            .border_color(if selected {
                theme.accent
            } else {
                Theme::TRANSPARENT
            })
            .when(selected, |row| row.bg(theme.glass_selected))
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
                    .text_color(symbol_color)
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
                // The glyph already says the kind on every row; the word is
                // kept for the selected one only.
                div()
                    .flex_shrink_0()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .when(!selected, |kind| kind.invisible())
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
            .max_h(height)
            .popover(cx)
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                v_flex()
                    .id(("lua-completion-documentation", index))
                    .w_full()
                    .max_h(height)
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
                                    details.child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(theme.text)
                                            .child(readable_signature(detail)),
                                    )
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
                                        // The signature is already shown as the
                                        // detail line; keep only the prose.
                                        let prose = if item.detail.is_some() {
                                            without_code_blocks(&content.value)
                                        } else {
                                            content.value.clone()
                                        };
                                        if prose.trim().is_empty() {
                                            return details;
                                        }
                                        TextView::markdown("lua-completion-markdown", prose)
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
        if self.items.is_empty() && self.signature_help.is_none() {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let available_width = (window.viewport_size().width - px(16.0)).max(px(0.0));
        // Keep both popovers inside the window. In narrow windows the list
        // takes priority; documentation stays to the right whenever it fits.
        let room_for_details = available_width >= px(640.0);
        // As wide as the longest label needs (glyph, label, kind), not a share
        // of the window, so short names don't sit across a wide empty row.
        let longest = self
            .items
            .iter()
            .map(|item| item.label.chars().count())
            .max()
            .unwrap_or(0);
        let wanted = px((longest as f32 * 8.0 + 150.0).clamp(320.0, 520.0));
        let width = if room_for_details {
            wanted.min(available_width * 0.55)
        } else {
            wanted.min(available_width)
        };
        let compact = width < px(300.0);
        let signature_budget = if self.signature_help.is_some() {
            (f32::from(window.viewport_size().height) * 0.3).min(160.0) + 8.0
        } else {
            0.0
        };
        let available = (f32::from(window.viewport_size().height) - 100.0 - signature_budget)
            .max(ROW_HEIGHT + HEADER_HEIGHT + FOOTER_HEIGHT);
        let selected_item = self.selected.and_then(|index| self.items.get(index));
        let show_details = selected_item.is_some_and(has_details);
        // Too narrow to sit beside the list, the details go under it instead,
        // in a short strip taken out of the list's height.
        const STACKED_DETAILS: f32 = 96.0;
        let stacked = show_details && !room_for_details;
        let list_budget = if stacked {
            available - STACKED_DETAILS - 8.0
        } else {
            available
        };
        let list_height = (self.items.len().min(6) as f32 * ROW_HEIGHT)
            .min(list_budget - HEADER_HEIGHT - FOOTER_HEIGHT - 2.0)
            .max(ROW_HEIGHT);
        let count = self.items.len();
        let position = self
            .selected
            .filter(|index| *index < count)
            .map(|index| index + 1)
            .unwrap_or(0);
        let details = Self::render_details(
            self.selected.unwrap_or(0),
            selected_item,
            if stacked {
                width
            } else {
                (available_width - width - px(8.0))
                    .min(px(400.0))
                    .max(px(0.0))
            },
            if stacked {
                px(STACKED_DETAILS)
            } else {
                // A single matching row still needs a useful reading area.
                px((list_height + HEADER_HEIGHT + FOOTER_HEIGHT + 2.0)
                    .max(240.0)
                    .min(available))
            },
            cx,
        );
        let has_items = !self.items.is_empty();
        let signature_help = self.signature_help;
        let callee = self.callee;
        let Self {
            items,
            selected,
            fragment,
            scroll_handle,
            on_accept,
            ..
        } = self;
        // Paint here to keep the suggestions under their editing ancestor in
        // the accessibility tree. GPUI deferred draws preserve dispatch ancestry,
        // but do not preserve the accessibility ancestor stack.
        anchored()
            .anchor(Anchor::BottomLeft)
            .offset(point(px(0.0), px(-8.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                v_flex()
                    .gap_2()
                    .flex_none()
                    .mono(cx)
                    .text_size(px(13.0))
                    .text_color(theme.text)
                    .when_some(signature_help, |popovers, help| {
                        popovers.child(
                            SignaturePopover::new(help.clone(), {
                                // Sized to the overload on screen, plus room for
                                // its header, so a short one stays compact.
                                let active = help
                                    .active_signature
                                    .and_then(|ix| help.signatures.get(ix as usize))
                                    .or(help.signatures.first())
                                    .map_or(0, |signature| signature.label.chars().count());
                                px((active as f32 * 7.6 + 48.0).clamp(360.0, 600.0))
                                    .min(available_width)
                            })
                            .callee(callee),
                        )
                    })
                    .when(has_items, |popovers| {
                        popovers.child(
                            h_flex()
                                // Beside the list the card lines up with its
                                // top, where the selection starts.
                                .items_start()
                                .when(stacked, |row| row.flex_col())
                                .gap_2()
                                .child(
                                    v_flex()
                                        .id("lua-completion-menu")
                                        .role(accesskit::Role::ListBox)
                                        .aria_label("Lua suggestions")
                                        .occlude()
                                        .w(width)
                                        .popover(cx)
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
                                                .child(super::components::scroll_viewport(
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
                                                    &scroll_handle,
                                                ))
                                                // Revealed on hover or while scrolling.
                                                .child(
                                                    div().absolute().inset_0().child(
                                                        gpui_base::Scrollbar::new(&scroll_handle)
                                                            .id("lua-completion-scrollbar")
                                                            .axis(ScrollbarAxis::Vertical)
                                                            .mode(gpui_base::ScrollbarMode::Hover)
                                                            .viewport_from_layout(),
                                                    ),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .h(px(FOOTER_HEIGHT))
                                                // Hints use the UI font, as on
                                                // every other surface.
                                                .font_family(
                                                    gpui_component::ActiveTheme::theme(cx)
                                                        .font_family
                                                        .clone(),
                                                )
                                                .flex_shrink_0()
                                                .px_3()
                                                .items_center()
                                                .gap_4()
                                                .border_t_1()
                                                .border_color(theme.border)
                                                .when(!compact, |footer| {
                                                    footer.child(hint("↑↓", "choose", cx))
                                                })
                                                .child(hint("tab", "insert", cx))
                                                .child(hint("esc", "dismiss", cx)),
                                        ),
                                )
                                .when(show_details, |popovers| popovers.child(details)),
                        )
                    }),
            )
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

/// Markdown with its fenced code blocks and horizontal rules removed.
fn without_code_blocks(markdown: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed == "---" {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A LuaLS `fun(self: T, a: x, b?: y):r` detail as `(a: x, b?: y) → r`: the
/// implicit receiver dropped, and spaces inside a parameter made unbreakable
/// so a long signature wraps between parameters, not inside one. Anything
/// that isn't a `fun(...)` comes back as it was.
fn readable_signature(detail: &str) -> String {
    let Some(rest) = detail.trim().strip_prefix("fun(") else {
        return detail.to_string();
    };
    let mut depth = 0usize;
    let mut params = Vec::new();
    let mut start = 0;
    let mut close = None;
    for (ix, ch) in rest.char_indices() {
        match ch {
            '(' | '<' | '{' | '[' => depth += 1,
            ')' if depth == 0 => {
                params.push(&rest[start..ix]);
                close = Some(ix);
                break;
            }
            ')' | '>' | '}' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                params.push(&rest[start..ix]);
                start = ix + 1;
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return detail.to_string();
    };
    let params: Vec<String> = params
        .into_iter()
        .map(str::trim)
        .filter(|param| !param.is_empty())
        .filter(|param| *param != "self" && !param.starts_with("self:"))
        .map(|param| param.replace(' ', "\u{a0}"))
        .collect();
    let returns = rest[close + 1..].trim().trim_start_matches(':').trim();
    let mut out = format!("({})", params.join(", "));
    if !returns.is_empty() {
        out.push_str(" \u{2192} ");
        out.push_str(&returns.replace(' ', "\u{a0}"));
    }
    out
}

#[cfg(test)]
mod signature_tests {
    use super::readable_signature;

    #[test]
    fn method_details_drop_the_receiver_and_keep_parameters_whole() {
        assert_eq!(
            readable_signature("fun(self: Pyonji, session: integer, tab?: integer):boolean"),
            "(session:\u{a0}integer, tab?:\u{a0}integer) \u{2192} boolean"
        );
        assert_eq!(
            readable_signature("fun(f: fun(a: x):y)"),
            "(f:\u{a0}fun(a:\u{a0}x):y)"
        );
        assert_eq!(readable_signature("integer"), "integer");
    }
}
/// LuaLS reports a table's functions as fields; a field holding a function
/// is a method as far as the reader is concerned.
fn effective_kind(item: &CompletionItem) -> Option<Kind> {
    let holds_function = item.detail.as_deref().is_some_and(|detail| {
        let detail = detail.trim_start();
        detail.starts_with("fun(") || detail.starts_with("function")
    });
    match item.kind {
        Some(Kind::FIELD | Kind::PROPERTY | Kind::VARIABLE) if holds_function => Some(Kind::METHOD),
        kind => kind,
    }
}

/// One hue per family of kinds, so the glyph column can be scanned.
fn kind_color(kind: Option<Kind>, theme: &crate::Theme) -> Rgba {
    match kind {
        Some(Kind::METHOD | Kind::FUNCTION | Kind::CONSTRUCTOR) => theme.info,
        Some(
            Kind::CLASS
            | Kind::INTERFACE
            | Kind::STRUCT
            | Kind::ENUM
            | Kind::TYPE_PARAMETER
            | Kind::MODULE,
        ) => theme.success,
        Some(Kind::CONSTANT | Kind::ENUM_MEMBER | Kind::VALUE | Kind::UNIT | Kind::COLOR) => {
            theme.warning
        }
        Some(Kind::KEYWORD | Kind::SNIPPET) => theme.accent,
        _ => theme.text_muted,
    }
}
