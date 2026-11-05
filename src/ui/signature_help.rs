//! Function arguments at the Lua prompt's current caret.

use crate::ui::PyTheme as _;
use crate::ui::components::PyStyled as _;
use async_lsp::lsp_types::{Documentation, ParameterLabel, SignatureHelp, SignatureInformation};
use gpui::{prelude::*, *};
use gpui_base::{h_flex, v_flex};
use std::ops::Range;

#[derive(Default)]
pub(super) struct LuaSignature {
    generation: u64,
    input: Option<(String, usize)>,
    pub(super) help: Option<SignatureHelp>,
}

impl LuaSignature {
    pub(super) fn clear(&mut self) {
        self.generation += 1;
        self.input = None;
        self.help = None;
    }

    pub(super) fn matches_input(&self, value: &str, cursor: usize) -> bool {
        self.input
            .as_ref()
            .is_some_and(|(text, position)| text == value && *position == cursor)
    }

    pub(super) fn begin_request(&mut self, value: &str, cursor: usize) -> Option<u64> {
        if self.matches_input(value, cursor) {
            return None;
        }
        self.clear();
        self.input = Some((value.into(), cursor));
        let cursor = value.floor_char_boundary(cursor.min(value.len()));
        // LuaLS determines the innermost call, ignoring parentheses in strings
        // and comments. Avoid requests altogether when there is no possible call.
        value[..cursor].contains('(').then_some(self.generation)
    }

    pub(super) fn finish_request(&mut self, generation: u64, help: Option<SignatureHelp>) -> bool {
        if self.generation != generation || self.input.is_none() {
            return false;
        }
        self.help = help.filter(|help| !help.signatures.is_empty());
        true
    }

    /// Show the next (`1`) or previous (`-1`) overload.
    pub(super) fn cycle(&mut self, delta: i32) {
        let Some(help) = self.help.as_mut() else {
            return;
        };
        let len = help.signatures.len() as i32;
        if len < 2 {
            return;
        }
        let current = help.active_signature.unwrap_or(0) as i32;
        help.active_signature = Some((current + delta).rem_euclid(len) as u32);
    }

    pub(super) fn dismiss(&mut self) {
        // Keep the input snapshot so cursor blinking cannot reopen a dismissed
        // popover. A real edit or caret movement starts a new request.
        self.generation += 1;
        self.help = None;
    }
}

#[derive(IntoElement)]
pub(super) struct SignaturePopover {
    help: SignatureHelp,
    width: Pixels,
    callee: Option<SharedString>,
}

impl SignaturePopover {
    pub(super) fn new(help: SignatureHelp, width: Pixels) -> Self {
        Self {
            help,
            width,
            callee: None,
        }
    }

    pub(super) fn callee(mut self, callee: Option<SharedString>) -> Self {
        self.callee = callee;
        self
    }
}

/// A signature label as the call it describes: the `(method) ` style prefix
/// split off as the kind, and the callee's name put in front of a bare
/// parameter list. `highlight` is moved along with the text.
fn display_label(
    label: &str,
    callee: Option<&str>,
    highlight: Option<Range<usize>>,
) -> (Option<&'static str>, String, Option<Range<usize>>) {
    const KINDS: [(&str, &str); 3] = [
        ("(method) ", "Method"),
        ("(function) ", "Function"),
        ("(field) ", "Field"),
    ];
    let (kind, cut) = KINDS
        .iter()
        .find(|(prefix, _)| label.starts_with(prefix))
        .map_or((None, 0), |(prefix, kind)| (Some(*kind), prefix.len()));
    let rest = &label[cut..];
    let name = callee.filter(|_| rest.starts_with('(')).unwrap_or("");
    let shift = |ix: usize| ix.saturating_sub(cut) + name.len();
    let highlight = highlight
        .filter(|range| range.start >= cut)
        .map(|range| shift(range.start)..shift(range.end));
    (kind, format!("{name}{rest}"), highlight)
}

/// The name of the innermost call the caret sits in, `py:rename` for
/// `py:rename("a`.
pub(super) fn callee(value: &str, cursor: usize) -> Option<String> {
    let before = &value[..value.floor_char_boundary(cursor.min(value.len()))];
    let mut depth = 0usize;
    let open = before.char_indices().rev().find_map(|(ix, ch)| match ch {
        ')' => {
            depth += 1;
            None
        }
        '(' if depth == 0 => Some(ix),
        '(' => {
            depth -= 1;
            None
        }
        _ => None,
    })?;
    let head = before[..open].trim_end();
    let start = head
        .char_indices()
        .rev()
        .find(|(_, ch)| !(ch.is_alphanumeric() || matches!(ch, '_' | '.' | ':')))
        .map_or(0, |(ix, ch)| ix + ch.len_utf8());
    let name = &head[start..];
    (!name.is_empty()).then(|| name.to_string())
}

impl RenderOnce for SignaturePopover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let index = self.help.active_signature.unwrap_or(0) as usize;
        let index = if index < self.help.signatures.len() {
            index
        } else {
            0
        };
        let Some(signature) = self.help.signatures.get(index) else {
            return div().into_any_element();
        };
        let parameter_index = signature
            .active_parameter
            .or(self.help.active_parameter)
            .unwrap_or(0) as usize;
        let parameters = signature.parameters.as_deref().unwrap_or(&[]);
        let parameter = parameters.get(parameter_index);
        let highlight =
            parameter.and_then(|parameter| parameter_range(signature, &parameter.label));
        // LuaLS labels a method `(method) (name: string)`; show it as the call
        // being written, `py:rename(name: string)`, and keep the kind for the header.
        let (_kind, label, highlight) =
            display_label(&signature.label, self.callee.as_deref(), highlight);
        let documentation = parameter
            .and_then(|parameter| parameter.documentation.as_ref())
            .or(signature.documentation.as_ref())
            .map(documentation_summary)
            .filter(|text| !text.is_empty());
        let max_height = (window.viewport_size().height * 0.3).min(px(160.0));
        v_flex()
            .id("lua-signature-help")
            .occlude()
            .w(self.width)
            .max_h(max_height)
            .popover(cx)
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_3()
                    .justify_between()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(if parameter.is_some() {
                                format!("Argument {} of {}", parameter_index + 1, parameters.len())
                            } else {
                                "Arguments".into()
                            }),
                    )
                    // Kept at every width (shortened when narrow), so other
                    // overloads stay discoverable.
                    // Spelt like every other key hint: the key bright, what
                    // it does muted.
                    .when(self.help.signatures.len() > 1, |header| {
                        header.child(crate::ui::components::hint(
                            "↑↓",
                            format!(
                                "{}{} of {}",
                                if self.width >= px(360.0) {
                                    "overload "
                                } else {
                                    ""
                                },
                                index + 1,
                                self.help.signatures.len()
                            ),
                            cx,
                        ))
                    }),
            )
            .child(
                v_flex()
                    .id(("lua-signature-content", index))
                    .max_h((max_height - px(28.0)).max(px(24.0)))
                    .flex_shrink_0()
                    .px_3()
                    .pb_2()
                    .gap_1()
                    .child(div().text_color(theme.text).child(
                        StyledText::new(label).with_highlights(
                            highlight.map(|range| (range, theme.match_highlight())),
                        ),
                    ))
                    .when_some(documentation, |content, text| {
                        content.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child(text),
                        )
                    })
                    .overflow_y_scroll(),
            )
            .into_any_element()
    }
}

fn documentation_summary(documentation: &Documentation) -> String {
    let text = match documentation {
        Documentation::String(text) => text,
        Documentation::MarkupContent(content) => &content.value,
    };
    // Keep signature help compact; full documentation lives in completion details.
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn parameter_range(
    signature: &SignatureInformation,
    label: &ParameterLabel,
) -> Option<Range<usize>> {
    match label {
        ParameterLabel::Simple(parameter) => {
            // Start after the function name so a parameter named like the
            // function cannot highlight the name instead of the argument.
            let start = signature.label.find('(').map_or(0, |index| index + 1);
            let offset = signature.label[start..].find(parameter)? + start;
            (!parameter.is_empty()).then_some(offset..offset + parameter.len())
        }
        ParameterLabel::LabelOffsets([start, end]) => {
            let start = utf16_offset(&signature.label, *start)?;
            let end = utf16_offset(&signature.label, *end)?;
            (start < end).then_some(start..end)
        }
    }
}

fn utf16_offset(value: &str, offset: u32) -> Option<usize> {
    let mut units = 0;
    for (index, character) in value.char_indices() {
        if units == offset {
            return Some(index);
        }
        units += character.len_utf16() as u32;
    }
    (units == offset).then_some(value.len())
}

#[cfg(test)]
mod tests {
    use super::{LuaSignature, parameter_range};
    use async_lsp::lsp_types::{ParameterLabel, SignatureHelp, SignatureInformation};

    fn help() -> SignatureHelp {
        SignatureHelp {
            signatures: vec![SignatureInformation {
                label: "function sub(s: string, i: integer, j?: integer)".into(),
                documentation: None,
                parameters: None,
                active_parameter: Some(1),
            }],
            active_signature: Some(0),
            active_parameter: None,
        }
    }

    #[test]
    fn stale_signatures_cannot_reopen_after_edits_or_dismissal() {
        let mut state = LuaSignature::default();
        let old = state.begin_request("string.sub(", 11).unwrap();
        let current = state.begin_request("string.sub('abc',", 17).unwrap();
        assert!(!state.finish_request(old, Some(help())));
        assert!(state.finish_request(current, Some(help())));
        state.dismiss();
        assert!(!state.finish_request(current, Some(help())));
        assert!(state.begin_request("string.sub('abc',", 17).is_none());
        assert!(state.begin_request("string.sub('abc', ", 18).is_some());
        state.clear();
        assert!(!state.finish_request(current, Some(help())));
    }

    #[test]
    fn cursor_movement_refreshes_help_and_empty_results_clear_it() {
        let mut state = LuaSignature::default();
        let text = "string.sub('abc', 1)";
        let first = state.begin_request(text, 11).unwrap();
        state.finish_request(first, Some(help()));
        let second = state.begin_request(text, 17).unwrap();
        assert!(state.help.is_none());
        assert!(!state.finish_request(first, Some(help())));
        assert!(state.finish_request(second, None));
        assert!(state.help.is_none());
        assert!(state.begin_request("string.sub", 10).is_none());
    }

    #[test]
    fn parameter_offsets_convert_utf16_without_splitting_unicode() {
        let mut signature = help().signatures.remove(0);
        signature.label = "f(한😀: string, x: number)".into();
        let range = parameter_range(&signature, &ParameterLabel::LabelOffsets([2, 13])).unwrap();
        assert_eq!(&signature.label[range], "한😀: string");
        assert!(parameter_range(&signature, &ParameterLabel::LabelOffsets([4, 13])).is_none());
        assert!(parameter_range(&signature, &ParameterLabel::LabelOffsets([13, 2])).is_none());
        assert!(parameter_range(&signature, &ParameterLabel::LabelOffsets([2, 100])).is_none());
        assert_eq!(
            parameter_range(&signature, &ParameterLabel::Simple("x: number".into())),
            Some(19..28)
        );
        signature.label = "foo(foo: string)".into();
        assert_eq!(
            parameter_range(&signature, &ParameterLabel::Simple("foo".into())),
            Some(4..7)
        );
    }
}

#[cfg(test)]
mod callee_tests {
    use super::callee;

    #[test]
    fn a_method_label_reads_as_the_call() {
        let label = "(method) (name: string)";
        let start = label.find("name").unwrap();
        let (kind, text, highlight) =
            super::display_label(label, Some("py:rename"), Some(start..start + 4));
        assert_eq!(kind, Some("Method"));
        assert_eq!(text, "py:rename(name: string)");
        assert_eq!(&text[highlight.unwrap()], "name");
        assert_eq!(super::display_label("f(a)", None, None).1, "f(a)");
    }

    #[test]
    fn the_innermost_open_call_is_named() {
        assert_eq!(callee("py:rename(", 10).as_deref(), Some("py:rename"));
        assert_eq!(callee("f(g(1), h(", 10).as_deref(), Some("h"));
        assert_eq!(callee("f(g(1), ", 8).as_deref(), Some("f"));
        assert_eq!(callee("x = 1", 5), None);
    }
}
