//! Pane-local text ranges, anchored to scrollback rather than viewport rows.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Cell {
    row: i64,
    col: u16,
}

#[derive(Clone, Debug)]
pub struct Selection {
    anchor: Cell,
    end: Cell,
}

impl Selection {
    pub fn new(screen: &vt100::Screen, row: u16, col: u16) -> Self {
        let anchor = Self::cell(screen, row, col);
        Self {
            anchor,
            end: anchor,
        }
    }

    pub fn extend(&mut self, screen: &vt100::Screen, row: u16, col: u16) {
        self.end = Self::cell(screen, row, col);
    }

    fn cell(screen: &vt100::Screen, row: u16, col: u16) -> Cell {
        let col = if screen
            .cell(row, col)
            .is_some_and(vt100::Cell::is_wide_continuation)
        {
            col.saturating_sub(1)
        } else {
            col
        };
        Cell {
            row: i64::from(row) - screen.scrollback() as i64,
            col,
        }
    }

    fn range(&self) -> (Cell, Cell) {
        (self.anchor.min(self.end), self.anchor.max(self.end))
    }

    pub fn contains(&self, screen: &vt100::Screen, row: u16, col: u16) -> bool {
        let cell = Self::cell(screen, row, col);
        let (start, end) = self.range();
        start <= cell && cell <= end
    }

    pub fn text(&self, screen: &vt100::Screen) -> String {
        let (start, end) = self.range();
        let mut screen = screen.clone();
        let mut result = String::new();
        let (_, cols) = screen.size();
        for absolute_row in start.row..=end.row {
            let row = if absolute_row < 0 {
                screen.set_scrollback((-absolute_row) as usize);
                0
            } else {
                screen.set_scrollback(0);
                absolute_row as u16
            };
            let from = if absolute_row == start.row {
                start.col
            } else {
                0
            };
            let to = if absolute_row == end.row {
                let width = if screen.cell(row, end.col).is_some_and(vt100::Cell::is_wide) {
                    2
                } else {
                    1
                };
                end.col.saturating_add(width).min(cols)
            } else {
                cols
            };
            result.push_str(&screen.contents_between(row, from, row, to));
            if absolute_row != end.row && !screen.row_wrapped(row) {
                result.push('\n');
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    #[test]
    fn copies_both_drag_directions_multiline_and_unicode() {
        let mut parser = vt100::Parser::new(3, 20, 20);
        parser.process("hello 世界\r\nsecond line".as_bytes());
        let screen = parser.screen();
        let mut selection = Selection::new(screen, 0, 1);
        selection.extend(screen, 1, 5);
        assert_eq!(selection.text(screen), "ello 世界\nsecond");
        let mut reversed = Selection::new(screen, 1, 5);
        reversed.extend(screen, 0, 1);
        assert_eq!(reversed.text(screen), selection.text(screen));
        let mut wide = Selection::new(screen, 0, 7);
        wide.extend(screen, 0, 9);
        assert_eq!(wide.text(screen), "世界");
        assert!(wide.contains(screen, 0, 7));
    }

    #[test]
    fn preserves_soft_wraps_and_scrollback_ranges() {
        let mut parser = vt100::Parser::new(2, 5, 20);
        parser.process(b"abcdefghij\r\nklmno\r\npqrst");
        parser.screen_mut().set_scrollback(2);
        let mut selection = Selection::new(parser.screen(), 0, 0);
        parser.screen_mut().set_scrollback(0);
        selection.extend(parser.screen(), 0, 4);
        assert_eq!(selection.text(parser.screen()), "abcdefghij\nklmno");
        assert!(selection.contains(parser.screen(), 0, 2));
        assert!(!selection.contains(parser.screen(), 1, 0));
    }
}
