use std::ops::Range;

use gpui::*;

use super::Terminal;

/// Only the uncommitted composition is editable; shell output is not a text buffer.
#[derive(Default)]
pub(super) struct TerminalInput {
    pub preedit: Option<String>,
    selection: Range<usize>,
}

impl TerminalInput {
    pub fn clear(&mut self) {
        self.preedit = None;
        self.selection = 0..0;
    }

    fn len(&self) -> usize {
        self.preedit
            .as_deref()
            .unwrap_or_default()
            .encode_utf16()
            .count()
    }

    fn mark(&mut self, range: Option<Range<usize>>, text: &str, selection: Option<Range<usize>>) {
        let current = self.preedit.get_or_insert_with(String::new);
        let range = range.unwrap_or(0..current.encode_utf16().count());
        let (bytes, adjusted) = utf16_range(current, range);
        current.replace_range(bytes, text);
        let end = adjusted.start + text.encode_utf16().count();
        let selection = selection
            .map(|range| adjusted.start + range.start..adjusted.start + range.end)
            .unwrap_or(end..end);
        self.selection = utf16_range(current, selection).1;
        if current.is_empty() {
            self.clear();
        }
    }
}

/// Clamp platform offsets to Unicode scalar boundaries, including surrogate pairs.
fn utf16_range(text: &str, range: Range<usize>) -> (Range<usize>, Range<usize>) {
    let mut boundaries = vec![(0, 0)];
    let mut offset = 0;
    for (byte, ch) in text.char_indices() {
        offset += ch.len_utf16();
        boundaries.push((offset, byte + ch.len_utf8()));
    }
    let start = range.start.min(offset);
    let end = range.end.max(start).min(offset);
    let &(start_utf16, start_byte) = boundaries.iter().rev().find(|(o, _)| *o <= start).unwrap();
    let &(end_utf16, end_byte) = boundaries.iter().find(|(o, _)| *o >= end).unwrap();
    (start_byte..end_byte, start_utf16..end_utf16)
}

impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.input.preedit.as_deref().unwrap_or_default();
        let (bytes, adjusted) = utf16_range(text, range);
        *adjusted_range = Some(adjusted);
        Some(text[bytes].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.input.selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.input.preedit.as_ref().map(|_| 0..self.input.len())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_ime(cx);
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_ime(cx);
        if text.is_empty() {
            return;
        }
        if let Some(pyonji) = self.pyonji.upgrade() {
            pyonji.update(cx, |py, cx| {
                if let Some(id) = py.active_session() {
                    if let Some(session) = py.session_manager.session_mut(id) {
                        session.reset_scrollback();
                    }
                    py.session_manager.send_text(id, text);
                    cx.notify();
                }
            });
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.mark(range, text, selection);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let py = self.pyonji.upgrade()?;
        let py = py.read(cx);
        let id = py.active_session()?;
        let session = py.session_manager.session(id)?;
        let (_, pane) = py
            .tab_layouts(self.rows, self.cols)
            .into_iter()
            .find(|(s, _)| *s == id)?;
        let (row, col) = session.vt.screen().cursor_position();
        let (cell_width, line_height) =
            self.cell_metrics(py.font_size, py.font_size * py.line_height)?;
        let text = self.input.preedit.as_deref().unwrap_or_default();
        let (prefix, _) = utf16_range(text, 0..range.start);
        let offset = unicode_width::UnicodeWidthStr::width(&text[prefix]) as f32;
        Some(Bounds::new(
            bounds.origin
                + point(
                    px((f32::from(pane.x) + f32::from(col) + offset) * cell_width),
                    px((f32::from(pane.y) + f32::from(row)) * line_height),
                ),
            size(px(cell_width), px(line_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        // Terminal output cannot be selected or edited through the IME buffer.
        None
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.input.len())
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalInput, utf16_range};

    #[test]
    fn composition_updates_replace_preedit_and_clear_on_cancel() {
        let mut input = TerminalInput::default();
        input.mark(None, "ㅎ", None);
        input.mark(None, "한", Some(1..1));
        assert_eq!(input.preedit.as_deref(), Some("한"));
        assert_eq!(input.selection, 1..1);
        input.mark(None, "", None);
        assert_eq!(input.preedit, None);
        assert_eq!(input.selection, 0..0);
    }

    #[test]
    fn composition_ranges_use_utf16_and_preserve_surrogate_pairs() {
        let mut input = TerminalInput::default();
        input.mark(None, "a😀한", None);
        assert_eq!(input.len(), 4);
        assert_eq!(input.selection, 4..4);
        assert_eq!(utf16_range("a😀한", 2..3), (1..5, 1..3));
        input.mark(Some(1..3), "文", Some(0..1));
        assert_eq!(input.preedit.as_deref(), Some("a文한"));
        assert_eq!(input.selection, 1..2);
        input.clear();
        assert_eq!(input.preedit, None);
        assert_eq!(input.selection, 0..0);
    }
}
