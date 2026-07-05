pub mod manager;
use crate::pty::Pty;
pub use manager::*;
use winit::{
    event::{ElementState, KeyEvent, MouseButton},
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey},
};

#[derive(Debug)]
pub enum CursorState {
    Bar,
    Block,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanePathStep {
    First,
    Second,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneGeometry {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

impl PaneGeometry {
    pub fn contains_global_cell(self, col: u16, row: u16) -> bool {
        let min_col = self.x.saturating_add(1);
        let min_row = self.y.saturating_add(1);
        let max_col = self.x.saturating_add(self.cols);
        let max_row = self.y.saturating_add(self.rows);
        col >= min_col && col <= max_col && row >= min_row && row <= max_row
    }

    pub fn local_cell(self, col: u16, row: u16) -> (u16, u16) {
        (col.saturating_sub(self.x), row.saturating_sub(self.y))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divider {
    pub path: Vec<PanePathStep>,
    pub direction: SplitDirection,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

pub enum Pane {
    Leaf {
        session: SessionId,
    },
    Split {
        direction: SplitDirection,
        ratio_per_mille: u16,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl Pane {
    pub fn new(session: SessionId) -> Self {
        Self::Leaf { session }
    }

    pub fn contains_session(&self, target: SessionId) -> bool {
        match self {
            Self::Leaf { session } => *session == target,
            Self::Split { first, second, .. } => {
                first.contains_session(target) || second.contains_session(target)
            }
        }
    }

    pub fn split(
        &mut self,
        target: SessionId,
        direction: SplitDirection,
        session: SessionId,
    ) -> bool {
        match self {
            Self::Leaf { session: current } if *current == target => {
                let existing = *current;
                *self = Self::Split {
                    direction,
                    ratio_per_mille: 500,
                    first: Box::new(Self::new(existing)),
                    second: Box::new(Self::new(session)),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split(target, direction, session) || second.split(target, direction, session)
            }
        }
    }

    pub fn sessions(&self) -> Vec<SessionId> {
        let mut sessions = Vec::new();
        self.collect_sessions(&mut sessions);
        sessions
    }

    pub fn first_session(&self) -> SessionId {
        match self {
            Self::Leaf { session, .. } => *session,
            Self::Split { first, .. } => first.first_session(),
        }
    }

    pub fn remove_session(self, target: SessionId) -> Option<Self> {
        match self {
            Self::Leaf { session, .. } => (session != target).then_some(Self::Leaf { session }),
            Self::Split {
                direction,
                ratio_per_mille,
                first,
                second,
            } => match (first.remove_session(target), second.remove_session(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    direction,
                    ratio_per_mille,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            },
        }
    }

    pub fn layout(
        &self,
        area: PaneGeometry,
        path: &mut Vec<PanePathStep>,
        panes: &mut Vec<(SessionId, PaneGeometry)>,
        dividers: &mut Vec<Divider>,
    ) {
        match self {
            Self::Leaf { session, .. } => panes.push((*session, area)),
            Self::Split {
                direction,
                ratio_per_mille,
                first,
                second,
            } => {
                let (first_area, second_area, _) =
                    split_geometry(area, *direction, *ratio_per_mille);
                dividers.push(match direction {
                    SplitDirection::Horizontal => Divider {
                        path: path.clone(),
                        direction: *direction,
                        x: area.x,
                        y: second_area.y,
                        cols: area.cols,
                        rows: 0,
                    },
                    SplitDirection::Vertical => Divider {
                        path: path.clone(),
                        direction: *direction,
                        x: second_area.x,
                        y: area.y,
                        cols: 0,
                        rows: area.rows,
                    },
                });
                path.push(PanePathStep::First);
                first.layout(first_area, path, panes, dividers);
                path.pop();
                path.push(PanePathStep::Second);
                second.layout(second_area, path, panes, dividers);
                path.pop();
            }
        }
    }

    pub fn resize_split_delta(
        &mut self,
        path: &[PanePathStep],
        area: PaneGeometry,
        direction: SplitDirection,
        delta_first: i16,
    ) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                direction: split_direction,
                ratio_per_mille,
                first,
                second,
            } => {
                let (first_area, second_area, first_size) =
                    split_geometry(area, *split_direction, *ratio_per_mille);
                if let Some((step, rest)) = path.split_first() {
                    return match step {
                        PanePathStep::First => {
                            first.resize_split_delta(rest, first_area, direction, delta_first)
                        }
                        PanePathStep::Second => {
                            second.resize_split_delta(rest, second_area, direction, delta_first)
                        }
                    };
                }
                if *split_direction != direction {
                    return false;
                }
                let total = split_axis_size(area, direction);
                let next = clamp_first_size(i32::from(first_size) + i32::from(delta_first), total);
                *ratio_per_mille = ratio_from_first_size(next, total);
                true
            }
        }
    }

    pub fn resize_split_by_position(
        &mut self,
        path: &[PanePathStep],
        area: PaneGeometry,
        direction: SplitDirection,
        position: f32,
    ) -> bool {
        match self {
            Self::Leaf { .. } => false,
            Self::Split {
                direction: split_direction,
                ratio_per_mille,
                first,
                second,
            } => {
                let (first_area, second_area, _) =
                    split_geometry(area, *split_direction, *ratio_per_mille);
                if let Some((step, rest)) = path.split_first() {
                    return match step {
                        PanePathStep::First => {
                            first.resize_split_by_position(rest, first_area, direction, position)
                        }
                        PanePathStep::Second => {
                            second.resize_split_by_position(rest, second_area, direction, position)
                        }
                    };
                }
                if *split_direction != direction {
                    return false;
                }
                let total = split_axis_size(area, direction);
                let offset = match direction {
                    SplitDirection::Horizontal => position - f32::from(area.y),
                    SplitDirection::Vertical => position - f32::from(area.x),
                };
                let next = clamp_first_size(offset.round() as i32, total);
                *ratio_per_mille = ratio_from_first_size(next, total);
                true
            }
        }
    }

    pub fn find_resize_target(
        &self,
        target: SessionId,
        direction: SplitDirection,
        path: &mut Vec<PanePathStep>,
    ) -> Option<Vec<PanePathStep>> {
        match self {
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => {
                if first.contains_session(target) {
                    path.push(PanePathStep::First);
                    let found = first.find_resize_target(target, direction, path);
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                if second.contains_session(target) {
                    path.push(PanePathStep::Second);
                    let found = second.find_resize_target(target, direction, path);
                    path.pop();
                    if found.is_some() {
                        return found;
                    }
                }
                matches!(
                    self,
                    Self::Split {
                        direction: split_direction,
                        ..
                    } if *split_direction == direction
                        && (first.contains_session(target) || second.contains_session(target))
                )
                .then(|| path.clone())
            }
        }
    }

    fn collect_sessions(&self, sessions: &mut Vec<SessionId>) {
        match self {
            Self::Leaf { session, .. } => sessions.push(*session),
            Self::Split { first, second, .. } => {
                first.collect_sessions(sessions);
                second.collect_sessions(sessions);
            }
        }
    }
}

pub struct Tab {
    root: Option<Pane>,
    active_session: Option<SessionId>,
}

impl Tab {
    pub fn new(session: SessionId) -> Self {
        Self {
            root: Some(Pane::new(session)),
            active_session: Some(session),
        }
    }

    pub fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }

    pub fn set_active_session(&mut self, session: SessionId) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        if !root.contains_session(session) {
            return false;
        }
        self.active_session = Some(session);
        true
    }

    pub fn split_active(&mut self, direction: SplitDirection, session: SessionId) -> bool {
        let Some(active_session) = self.active_session else {
            return false;
        };
        let Some(root) = self.root.as_mut() else {
            return false;
        };
        if !root.split(active_session, direction, session) {
            return false;
        }
        self.active_session = Some(session);
        true
    }

    pub fn focus_next(&mut self) -> Option<SessionId> {
        let sessions = self.sessions();
        let active = self.active_session?;
        if sessions.len() <= 1 {
            return Some(active);
        }
        let current = sessions
            .iter()
            .position(|session| *session == active)
            .unwrap_or(0);
        let next = sessions[(current + 1) % sessions.len()];
        self.active_session = Some(next);
        Some(next)
    }

    pub fn remove_session(&mut self, session: SessionId) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        if !root.contains_session(session) {
            return false;
        }

        let root = self.root.take().unwrap();
        self.root = root.remove_session(session);
        self.active_session = match self.root.as_ref() {
            Some(root) if self.active_session == Some(session) => Some(root.first_session()),
            Some(_) => self.active_session,
            None => None,
        };
        true
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn sessions(&self) -> Vec<SessionId> {
        self.root.as_ref().map(Pane::sessions).unwrap_or_default()
    }

    pub fn layout(&self, area: PaneGeometry) -> Vec<(SessionId, PaneGeometry)> {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        if let Some(root) = self.root.as_ref() {
            root.layout(area, &mut Vec::new(), &mut panes, &mut dividers);
        }
        panes
    }

    pub fn dividers(&self, area: PaneGeometry) -> Vec<Divider> {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        if let Some(root) = self.root.as_ref() {
            root.layout(area, &mut Vec::new(), &mut panes, &mut dividers);
        }
        dividers
    }

    pub fn resize_active_split(
        &mut self,
        area: PaneGeometry,
        direction: SplitDirection,
        delta_first: i16,
    ) -> bool {
        let Some(active_session) = self.active_session else {
            return false;
        };
        let Some(root) = self.root.as_mut() else {
            return false;
        };
        let Some(path) = root.find_resize_target(active_session, direction, &mut Vec::new()) else {
            return false;
        };
        root.resize_split_delta(&path, area, direction, delta_first)
    }

    pub fn resize_split_by_position(
        &mut self,
        area: PaneGeometry,
        path: &[PanePathStep],
        direction: SplitDirection,
        position: f32,
    ) -> bool {
        let Some(root) = self.root.as_mut() else {
            return false;
        };
        root.resize_split_by_position(path, area, direction, position)
    }
}

fn split_geometry(
    area: PaneGeometry,
    direction: SplitDirection,
    ratio_per_mille: u16,
) -> (PaneGeometry, PaneGeometry, u16) {
    match direction {
        SplitDirection::Horizontal => {
            let first_rows = first_split_size(area.rows, ratio_per_mille);
            let second_rows = area.rows.saturating_sub(first_rows);
            (
                PaneGeometry {
                    rows: first_rows,
                    ..area
                },
                PaneGeometry {
                    y: area.y.saturating_add(first_rows),
                    rows: second_rows,
                    ..area
                },
                first_rows,
            )
        }
        SplitDirection::Vertical => {
            let first_cols = first_split_size(area.cols, ratio_per_mille);
            let second_cols = area.cols.saturating_sub(first_cols);
            (
                PaneGeometry {
                    cols: first_cols,
                    ..area
                },
                PaneGeometry {
                    x: area.x.saturating_add(first_cols),
                    cols: second_cols,
                    ..area
                },
                first_cols,
            )
        }
    }
}

fn split_axis_size(area: PaneGeometry, direction: SplitDirection) -> u16 {
    match direction {
        SplitDirection::Horizontal => area.rows,
        SplitDirection::Vertical => area.cols,
    }
}

fn first_split_size(total: u16, ratio_per_mille: u16) -> u16 {
    if total <= 1 {
        return total;
    }
    let size = ((u32::from(total) * u32::from(ratio_per_mille)) + 500) / 1000;
    clamp_first_size(size.cast_signed(), total)
}

fn clamp_first_size(size: i32, total: u16) -> u16 {
    if total <= 1 {
        return total;
    }
    size.clamp(1, i32::from(total) - 1) as u16
}

fn ratio_from_first_size(first_size: u16, total: u16) -> u16 {
    if total <= 1 {
        return 500;
    }
    (((u32::from(first_size) * 1000) + (u32::from(total) / 2)) / u32::from(total)).clamp(1, 999)
        as u16
}

pub struct TerminalSession {
    pub _id: SessionId,
    pub pty: Pty,
    pub vt: vt100::Parser<CB>,
    pub cursor_style: CursorState,
    pub title: String,
    pub mouse_pressed_button: Option<MouseButton>,
    pub last_mouse_cell: Option<(u16, u16)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseAction {
    Press,
    Release,
    Motion,
}

fn control_key_byte(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::KeyA => Some(0x01),
        KeyCode::KeyB => Some(0x02),
        KeyCode::KeyC => Some(0x03),
        KeyCode::KeyD => Some(0x04),
        KeyCode::KeyE => Some(0x05),
        KeyCode::KeyF => Some(0x06),
        KeyCode::KeyG => Some(0x07),
        KeyCode::KeyH => Some(0x08),
        KeyCode::KeyI => Some(0x09),
        KeyCode::KeyJ => Some(0x0a),
        KeyCode::KeyK => Some(0x0b),
        KeyCode::KeyL => Some(0x0c),
        KeyCode::KeyM => Some(0x0d),
        KeyCode::KeyN => Some(0x0e),
        KeyCode::KeyO => Some(0x0f),
        KeyCode::KeyP => Some(0x10),
        KeyCode::KeyQ => Some(0x11),
        KeyCode::KeyR => Some(0x12),
        KeyCode::KeyS => Some(0x13),
        KeyCode::KeyT => Some(0x14),
        KeyCode::KeyU => Some(0x15),
        KeyCode::KeyV => Some(0x16),
        KeyCode::KeyW => Some(0x17),
        KeyCode::KeyX => Some(0x18),
        KeyCode::KeyY => Some(0x19),
        KeyCode::KeyZ => Some(0x1a),
        _ => None,
    }
}

impl TerminalSession {
    pub fn uses_local_scrollback(&self) -> bool {
        let screen = self.vt.screen();
        !screen.alternate_screen() && screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None
    }

    pub fn reset_scrollback(&mut self) -> bool {
        let screen = self.vt.screen();
        if screen.scrollback() == 0 {
            return false;
        }
        self.vt.screen_mut().set_scrollback(0);
        true
    }

    pub fn scroll_scrollback(&mut self, delta_lines: i32) -> bool {
        if !self.uses_local_scrollback() || delta_lines == 0 {
            return false;
        }

        let current = self.vt.screen().scrollback();
        let next = if delta_lines.is_positive() {
            current.saturating_add(delta_lines as usize)
        } else {
            current.saturating_sub(delta_lines.unsigned_abs() as usize)
        };
        self.vt.screen_mut().set_scrollback(next);
        self.vt.screen().scrollback() != current
    }

    fn interrupt_pty_data(&mut self, data: &[u8]) {
        let Ok(str) = std::str::from_utf8(data) else {
            return;
        };
        for m in cansi::parse(str) {
            match m.text {
                "\x1b[1 q" | "\x1b[2 q" => {
                    self.cursor_style = CursorState::Block;
                }
                "\x1b[3 q" | "\x1b[4 q" => {
                    self.cursor_style = CursorState::Underline;
                }
                "\x1b[0 q" | "\x1b[5 q" | "\x1b[6 q" => {
                    self.cursor_style = CursorState::Bar;
                }
                _ => {}
            }
        }
    }

    pub fn handle_key_press(&mut self, event: &KeyEvent, mods: ModifiersState, is_csi: Option<u8>) {
        if event.state != ElementState::Pressed {
            return;
        }

        if let PhysicalKey::Code(code) = event.physical_key
            && mods.control_key()
            && let Some(byte) = control_key_byte(code)
        {
            self.pty.add_bytes([byte]);
            return;
        }
        let is_app_cursor_mode = self.vt.screen().application_cursor();
        let _is_keypad_mode = self.vt.screen().application_keypad();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.pty.add_bytes([0x1b]),
            Key::Named(NamedKey::Enter) => self.pty.add_bytes(b"\r"),
            Key::Named(NamedKey::Backspace) => self.pty.add_bytes([0x7f]),
            Key::Named(NamedKey::Tab) => {
                if mods.shift_key() {
                    self.pty.add_bytes(b"\x1b[Z");
                } else {
                    self.pty.add_bytes(b"\t");
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.pty.add_cursor_key(is_csi, b'A', is_app_cursor_mode);
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.pty.add_cursor_key(is_csi, b'B', is_app_cursor_mode);
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.pty.add_cursor_key(is_csi, b'C', is_app_cursor_mode);
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.pty.add_cursor_key(is_csi, b'D', is_app_cursor_mode);
            }
            Key::Named(NamedKey::Home) => self.pty.add_csi_key(is_csi, b'H'),
            Key::Named(NamedKey::End) => self.pty.add_csi_key(is_csi, b'F'),
            Key::Named(NamedKey::Insert) => self.pty.add_csi_tilde(is_csi, 2),
            Key::Named(NamedKey::Delete) => self.pty.add_csi_tilde(is_csi, 3),
            Key::Named(NamedKey::PageUp) => self.pty.add_csi_tilde(is_csi, 5),
            Key::Named(NamedKey::PageDown) => self.pty.add_csi_tilde(is_csi, 6),
            Key::Named(NamedKey::F1) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{m}P").as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOP");
                }
            }
            Key::Named(NamedKey::F2) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{m}Q").as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOQ");
                }
            }
            Key::Named(NamedKey::F3) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{m}R").as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOR");
                }
            }
            Key::Named(NamedKey::F4) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{m}S").as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOS");
                }
            }
            Key::Named(NamedKey::F5) => self.pty.add_csi_tilde(is_csi, 15),
            Key::Named(NamedKey::F6) => self.pty.add_csi_tilde(is_csi, 17),
            Key::Named(NamedKey::F7) => self.pty.add_csi_tilde(is_csi, 18),
            Key::Named(NamedKey::F8) => self.pty.add_csi_tilde(is_csi, 19),
            Key::Named(NamedKey::F9) => self.pty.add_csi_tilde(is_csi, 20),
            Key::Named(NamedKey::F10) => self.pty.add_csi_tilde(is_csi, 21),
            Key::Named(NamedKey::F11) => self.pty.add_csi_tilde(is_csi, 23),
            Key::Named(NamedKey::F12) => self.pty.add_csi_tilde(is_csi, 24),
            _ => {
                if let Some(text) = &event.text {
                    if mods.alt_key() {
                        self.pty.add_bytes([0x1b]);
                    }
                    self.pty.add_bytes(text.as_bytes());
                }
            }
        }
    }

    pub fn handle_mouse_button(
        &mut self,
        button: MouseButton,
        state: ElementState,
        mods: ModifiersState,
        col: u16,
        row: u16,
    ) {
        let mode = self.vt.screen().mouse_protocol_mode();
        if mode == vt100::MouseProtocolMode::None {
            self.mouse_pressed_button = None;
            return;
        }

        match state {
            ElementState::Pressed => {
                self.mouse_pressed_button = Some(button);
                self.send_mouse_event(MouseAction::Press, Some(button), mods, col, row);
                self.last_mouse_cell = Some((col, row));
            }
            ElementState::Released => {
                self.send_mouse_event(MouseAction::Release, Some(button), mods, col, row);
                self.mouse_pressed_button = None;
                self.last_mouse_cell = Some((col, row));
            }
        }
    }

    pub fn handle_mouse_move(&mut self, mods: ModifiersState, col: u16, row: u16) {
        let mode = self.vt.screen().mouse_protocol_mode();
        if mode == vt100::MouseProtocolMode::None {
            return;
        }

        if self.last_mouse_cell == Some((col, row)) {
            return;
        }

        let button = match mode {
            vt100::MouseProtocolMode::ButtonMotion => {
                let Some(button) = self.mouse_pressed_button else {
                    return;
                };
                button
            }
            vt100::MouseProtocolMode::AnyMotion => {
                self.mouse_pressed_button.unwrap_or(MouseButton::Other(0))
            }
            _ => return,
        };

        self.send_mouse_event(MouseAction::Motion, Some(button), mods, col, row);
        self.last_mouse_cell = Some((col, row));
    }

    pub fn handle_mouse_wheel(&mut self, lines: f32, mods: ModifiersState, col: u16, row: u16) {
        let mode = self.vt.screen().mouse_protocol_mode();
        if mode == vt100::MouseProtocolMode::None || lines == 0.0 {
            return;
        }

        let mut button_code = if lines > 0.0 { 64u8 } else { 65u8 };
        if mods.shift_key() {
            button_code += 4;
        }
        if mods.alt_key() {
            button_code += 8;
        }
        if mods.control_key() {
            button_code += 16;
        }
        self.send_mouse_sequence(button_code, false, col, row);
        self.last_mouse_cell = Some((col, row));
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    fn send_mouse_event(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        mods: ModifiersState,
        col: u16,
        row: u16,
    ) {
        let mode = self.vt.screen().mouse_protocol_mode();
        match action {
            MouseAction::Press if mode == vt100::MouseProtocolMode::None => return,
            MouseAction::Release
                if !matches!(
                    mode,
                    vt100::MouseProtocolMode::PressRelease
                        | vt100::MouseProtocolMode::ButtonMotion
                        | vt100::MouseProtocolMode::AnyMotion
                ) =>
            {
                return;
            }
            MouseAction::Motion
                if !matches!(
                    mode,
                    vt100::MouseProtocolMode::ButtonMotion | vt100::MouseProtocolMode::AnyMotion
                ) =>
            {
                return;
            }
            _ => {}
        }

        let mut code = match action {
            MouseAction::Release => 3u8,
            MouseAction::Motion => {
                let base = match button {
                    Some(MouseButton::Left) => 0u8,
                    Some(MouseButton::Middle) => 1u8,
                    Some(MouseButton::Right) => 2u8,
                    _ => 3u8,
                };
                base + 32
            }
            MouseAction::Press => match button {
                Some(MouseButton::Left) => 0u8,
                Some(MouseButton::Middle) => 1u8,
                Some(MouseButton::Right) => 2u8,
                _ => return,
            },
        };

        if mods.shift_key() {
            code += 4;
        }
        if mods.alt_key() {
            code += 8;
        }
        if mods.control_key() {
            code += 16;
        }

        self.send_mouse_sequence(code, action == MouseAction::Release, col, row);
    }

    fn send_mouse_sequence(&mut self, code: u8, release: bool, col: u16, row: u16) {
        let encoding = self.vt.screen().mouse_protocol_encoding();
        match encoding {
            vt100::MouseProtocolEncoding::Sgr => {
                let terminator = if release { 'm' } else { 'M' };
                let seq = format!("\x1b[<{code};{col};{row}{terminator}");
                self.pty.add_bytes(seq.as_bytes());
            }
            vt100::MouseProtocolEncoding::Default | vt100::MouseProtocolEncoding::Utf8 => {
                let Some(cb) = code.checked_add(32) else {
                    return;
                };
                let Some(cx) = u8::try_from(col).ok().and_then(|v| v.checked_add(32)) else {
                    return;
                };
                let Some(cy) = u8::try_from(row).ok().and_then(|v| v.checked_add(32)) else {
                    return;
                };
                self.pty.add_bytes([0x1b, b'[', b'M', cb, cx, cy]);
            }
        }
    }
}
