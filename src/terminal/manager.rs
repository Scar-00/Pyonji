use anyhow::Result;
use async_channel::Sender;
use mlua::prelude::*;
use std::{collections::HashMap, path::Path};

use crate::{
    pty::{Event, Pty, SshConnection},
    renderer::{Color, DEFAULT_BACKGROUND, DEFAULT_FOREGROUND},
    terminal::{CursorState, TerminalSession},
};
use vt100::Callbacks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromLua for SessionId {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        Ok(Self(u64::from_lua(value, lua)?))
    }
}

/// Parsed from a command line, where a session is written as its bare number.
impl std::str::FromStr for SessionId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.trim().parse()?))
    }
}

impl IntoLua for SessionId {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        self.0.into_lua(lua)
    }
}

/// Pending input writes past which query replies are dropped.
const MAX_QUEUED_INPUT: usize = 1024;

pub struct CB {
    id: SessionId,
    proxy: Sender<Event>,
    /// Replies to queries (DSR, DA, ...) go back to the program as input.
    pty: Sender<Vec<u8>>,
}

impl CB {
    fn parse_title(title: &str) -> Option<String> {
        if let Some((_, program)) = title.split_once('-') {
            Some(program.trim().to_string())
        } else {
            let path = Path::new(title);
            path.file_name().map(|file| file.display().to_string())
        }
    }

    fn reply(&self, reply: String) {
        // A program that floods queries without reading its input would
        // otherwise grow the unbounded input queue forever.
        if self.pty.len() < MAX_QUEUED_INPUT {
            _ = self.pty.force_send(reply.into_bytes());
        }
    }

    /// Answer OSC color queries with the colors the renderer draws, so
    /// programs like Neovim can pick a light or dark theme. Colors can't be
    /// changed, so set requests are ignored.
    fn osc_reply(params: &[&[u8]]) -> Option<String> {
        let reply = |code: &str, color: Color| {
            let [r, g, b, _] = color.inner();
            // xterm's 16-bit form: each byte repeated, e.g. 0xc6 -> c6c6.
            format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\")
        };
        let replies: String = match params {
            // OSC 4: `index;?` pairs, any number of them.
            [b"4", pairs @ ..] => pairs
                .as_chunks::<2>()
                .0
                .iter()
                .filter(|[_, spec]| *spec == b"?")
                .filter_map(|[index, _]| std::str::from_utf8(index).ok()?.parse::<u8>().ok())
                .map(|idx| reply(&format!("4;{idx}"), vt100::Color::Idx(idx).into()))
                .collect(),
            // OSC 10/11: each further `?` queries the next code, so
            // `10;?;?` asks for foreground and background.
            [code @ (b"10" | b"11"), specs @ ..] => {
                let first = if *code == b"10" { 10 } else { 11 };
                specs
                    .iter()
                    .zip(first..)
                    .take_while(|&(spec, _)| *spec == b"?")
                    .filter_map(|(_, code)| match code {
                        10 => Some(reply("10", DEFAULT_FOREGROUND)),
                        11 => Some(reply("11", DEFAULT_BACKGROUND)),
                        _ => None,
                    })
                    .collect()
            }
            _ => return None,
        };
        (!replies.is_empty()).then_some(replies)
    }

    /// Answer CSI queries. Programs block until these replies arrive: ConPTY
    /// waits on the cursor position at startup, and Neovim waits on DA1 as the
    /// sentinel for its other probes.
    fn csi_reply(
        screen: &vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) -> Option<String> {
        let param = |i: usize| params.get(i).and_then(|p| p.first()).copied().unwrap_or(0);
        // One-based cursor position. vt100 parks the cursor one past the last
        // column while a wrap is pending; terminals report the last column.
        // Reports are absolute even under origin mode (DECOM): vt100 keeps
        // that mode and the scroll margins private.
        let cursor = || {
            let (row, col) = screen.cursor_position();
            let cols = screen.size().1;
            (row + 1, (col + 1).min(cols))
        };
        match (i1, i2, c) {
            // DSR: operating status and cursor position.
            (None, None, 'n') => match param(0) {
                5 => Some("\x1b[0n".into()),
                6 => {
                    let (row, col) = cursor();
                    Some(format!("\x1b[{row};{col}R"))
                }
                _ => None,
            },
            // DECXCPR: cursor position, DEC form, always on page 1.
            (Some(b'?'), None, 'n') if param(0) == 6 => {
                let (row, col) = cursor();
                Some(format!("\x1b[?{row};{col};1R"))
            }
            // DA1: VT220 with ANSI color.
            (None, None, 'c') if param(0) == 0 => Some("\x1b[?62;22c".into()),
            // DA2: terminal type, firmware version, ROM cartridge.
            (Some(b'>'), None, 'c') if param(0) == 0 => {
                Some(format!("\x1b[>0;{};0c", Self::version_number()))
            }
            // XTVERSION.
            (Some(b'>'), None, 'q') if param(0) == 0 => {
                Some(format!("\x1bP>|pyonji {}\x1b\\", env!("CARGO_PKG_VERSION")))
            }
            // XTWINOPS 18: text area size in characters.
            (None, None, 't') if param(0) == 18 => {
                let (rows, cols) = screen.size();
                Some(format!("\x1b[8;{rows};{cols}t"))
            }
            // DECRQM for ANSI modes; none are tracked.
            (Some(b'$'), None, 'p') => Some(format!("\x1b[{};0$y", param(0))),
            // DECRQM for DEC private modes.
            (Some(b'?'), Some(b'$'), 'p') => {
                let mode = param(0);
                let state = match Self::dec_mode(screen, mode) {
                    Some(true) => 1,
                    Some(false) => 2,
                    None => 0,
                };
                Some(format!("\x1b[?{mode};{state}$y"))
            }
            _ => None,
        }
    }

    /// State of the DEC private modes vt100 tracks; `None` for the rest.
    fn dec_mode(screen: &vt100::Screen, mode: u16) -> Option<bool> {
        use vt100::{MouseProtocolEncoding as Encoding, MouseProtocolMode as Mouse};
        let mouse = screen.mouse_protocol_mode();
        let encoding = screen.mouse_protocol_encoding();
        Some(match mode {
            1 => screen.application_cursor(),
            9 => mouse == Mouse::Press,
            25 => !screen.hide_cursor(),
            47 | 1049 => screen.alternate_screen(),
            1000 => mouse == Mouse::PressRelease,
            1002 => mouse == Mouse::ButtonMotion,
            1003 => mouse == Mouse::AnyMotion,
            1005 => encoding == Encoding::Utf8,
            1006 => encoding == Encoding::Sgr,
            2004 => screen.bracketed_paste(),
            _ => return None,
        })
    }

    /// `major.minor.patch` packed as `MMmmpp`, the way xterm-likes report DA2.
    fn version_number() -> u32 {
        let part = |s: &str| s.parse::<u32>().unwrap_or(0);
        part(env!("CARGO_PKG_VERSION_MAJOR")) * 10000
            + part(env!("CARGO_PKG_VERSION_MINOR")) * 100
            + part(env!("CARGO_PKG_VERSION_PATCH"))
    }
}

impl Callbacks for CB {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {}

    fn visual_bell(&mut self, _: &mut vt100::Screen) {}

    fn resize(&mut self, _: &mut vt100::Screen, _request: (u16, u16)) {}

    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, _icon_name: &[u8]) {}

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let Ok(title) = std::str::from_utf8(title) else {
            return;
        };
        let Some(title) = Self::parse_title(title) else {
            return;
        };
        _ = self
            .proxy
            .send_blocking(Event::ProgramChanged((self.id, title)));
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], _data: &[u8]) {}

    fn paste_from_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8]) {}

    fn unhandled_char(&mut self, _: &mut vt100::Screen, _c: char) {}

    fn unhandled_control(&mut self, _: &mut vt100::Screen, _b: u8) {}

    fn unhandled_escape(&mut self, _: &mut vt100::Screen, _: Option<u8>, _: Option<u8>, _: u8) {}

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if let Some(reply) = Self::csi_reply(screen, i1, i2, params, c) {
            self.reply(reply);
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let Some(reply) = Self::osc_reply(params) {
            self.reply(reply);
        }
    }
}

pub struct SessionManager {
    current_id: u64,
    sessions: HashMap<SessionId, TerminalSession>,
    proxy: Sender<Event>,
}

impl SessionManager {
    pub fn new(proxy: Sender<Event>) -> Self {
        Self {
            current_id: 0,
            sessions: HashMap::new(),
            proxy,
        }
    }

    pub fn create_session(
        &mut self,
        rows: u16,
        cols: u16,
        path: Option<&Path>,
    ) -> Result<SessionId> {
        let id = SessionId(self.current_id);
        self.current_id += 1;
        let pty = Pty::new(rows, cols, self.proxy.clone(), id, path)?;
        let writer = pty.writer();
        self.sessions.insert(
            id,
            TerminalSession {
                _id: id,
                pty,
                vt: vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    2000,
                    CB {
                        id,
                        proxy: self.proxy.clone(),
                        pty: writer,
                    },
                ),
                cursor_style: CursorState::Bar,
                title: "cmd".into(),
                custom_title: None,
                mouse_pressed_button: None,
                last_mouse_cell: None,
                selection: None,
            },
        );
        Ok(id)
    }

    #[allow(dead_code)]
    pub fn create_remote_session(
        &mut self,
        rows: u16,
        cols: u16,
        conn: &SshConnection,
    ) -> Result<SessionId> {
        let id = SessionId(self.current_id);
        self.current_id += 1;
        let pty = Pty::new_remote(rows, cols, self.proxy.clone(), id, conn)?;
        let writer = pty.writer();
        self.sessions.insert(
            id,
            TerminalSession {
                _id: id,
                pty,
                vt: vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    2000,
                    CB {
                        id,
                        proxy: self.proxy.clone(),
                        pty: writer,
                    },
                ),
                cursor_style: CursorState::Bar,
                title: "ssh".into(),
                custom_title: None,
                mouse_pressed_button: None,
                last_mouse_cell: None,
                selection: None,
            },
        );
        Ok(id)
    }

    pub fn remove_session(&mut self, id: SessionId) {
        self.sessions.remove(&id);
    }

    pub fn update_session(&mut self, id: SessionId, data: &[u8]) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        // Grid coordinates stop referring to the selected text after output changes.
        // Clearing prevents Copy from silently copying a different command result.
        session.selection = None;
        session.interrupt_pty_data(data);
        session.vt.process(data);
    }

    pub fn send_text(&mut self, id: SessionId, text: &str) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        session.pty.add_bytes(text.as_bytes());
    }

    pub fn session(&self, id: SessionId) -> Option<&TerminalSession> {
        self.sessions.get(&id)
    }

    pub fn session_mut(&mut self, id: SessionId) -> Option<&mut TerminalSession> {
        self.sessions.get_mut(&id)
    }

    pub fn resize_session(&mut self, id: SessionId, rows: u16, cols: u16) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        session.selection = None;
        session.pty.resize(rows, cols);
        session.vt.screen_mut().set_size(rows, cols);
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replies_to(input: &[u8]) -> Vec<String> {
        let (proxy, _events) = async_channel::unbounded();
        let (pty, replies) = async_channel::unbounded();
        let cb = CB {
            id: SessionId(0),
            proxy,
            pty,
        };
        let mut vt = vt100::Parser::new_with_callbacks(24, 80, 0, cb);
        vt.process(input);
        std::iter::from_fn(|| replies.try_recv().ok())
            .map(|reply| String::from_utf8(reply).unwrap())
            .collect()
    }

    #[test]
    fn cursor_position_reports_are_one_based() {
        assert_eq!(replies_to(b"\x1b[6n"), ["\x1b[1;1R"]);
        assert_eq!(replies_to(b"\x1b[5;10H\x1b[6n"), ["\x1b[5;10R"]);
        assert_eq!(replies_to(b"\x1b[3;4H\x1b[?6n"), ["\x1b[?3;4;1R"]);
        assert_eq!(replies_to(b"\x1b[5n"), ["\x1b[0n"]);
    }

    #[test]
    fn a_pending_wrap_reports_the_last_column() {
        let mut input = b"\x1b[3;1H".to_vec();
        input.extend([b'x'; 80]);
        input.extend(b"\x1b[6n\x1b[?6n");
        assert_eq!(replies_to(&input), ["\x1b[3;80R", "\x1b[?3;80;1R"]);
    }

    #[test]
    fn query_floods_cannot_grow_the_input_queue_unbounded() {
        let replies = replies_to(&b"\x1b[5n".repeat(MAX_QUEUED_INPUT + 100));
        assert_eq!(replies.len(), MAX_QUEUED_INPUT);
    }

    #[test]
    fn device_attribute_queries_are_answered() {
        assert_eq!(replies_to(b"\x1b[c"), ["\x1b[?62;22c"]);
        assert_eq!(replies_to(b"\x1b[0c"), ["\x1b[?62;22c"]);
        let [da2] = &replies_to(b"\x1b[>c")[..] else {
            panic!("expected one DA2 reply");
        };
        assert!(da2.starts_with("\x1b[>0;") && da2.ends_with(";0c"));
        let [version] = &replies_to(b"\x1b[>q")[..] else {
            panic!("expected one XTVERSION reply");
        };
        assert!(version.starts_with("\x1bP>|pyonji ") && version.ends_with("\x1b\\"));
    }

    #[test]
    fn mode_and_size_reports_reflect_the_screen() {
        assert_eq!(replies_to(b"\x1b[?2004$p"), ["\x1b[?2004;2$y"]);
        assert_eq!(replies_to(b"\x1b[?2004h\x1b[?2004$p"), ["\x1b[?2004;1$y"]);
        assert_eq!(replies_to(b"\x1b[?2026$p"), ["\x1b[?2026;0$y"]);
        // vt100 ignores 1047, so it must not be reported as recognized.
        assert_eq!(replies_to(b"\x1b[?1049h\x1b[?1047$p"), ["\x1b[?1047;0$y"]);
        assert_eq!(replies_to(b"\x1b[4$p"), ["\x1b[4;0$y"]);
        assert_eq!(replies_to(b"\x1b[18t"), ["\x1b[8;24;80t"]);
    }

    #[test]
    fn color_queries_report_the_rendered_colors() {
        assert_eq!(
            replies_to(b"\x1b]10;?\x07\x1b]11;?\x1b\\"),
            [
                "\x1b]10;rgb:c6c6/d0d0/f5f5\x1b\\",
                "\x1b]11;rgb:1818/1818/1818\x1b\\"
            ]
        );
        // A further `?` queries the next code.
        assert_eq!(
            replies_to(b"\x1b]10;?;?\x07"),
            ["\x1b]10;rgb:c6c6/d0d0/f5f5\x1b\\\x1b]11;rgb:1818/1818/1818\x1b\\"]
        );
        // Palette entries come from the same lookup the renderer uses.
        let [r, g, b, _] = Color::from(vt100::Color::Idx(1)).inner();
        assert_eq!(
            replies_to(b"\x1b]4;1;?;196;?\x07"),
            [format!(
                "\x1b]4;1;rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\\
                 \x1b]4;196;rgb:ffff/0000/0000\x1b\\"
            )]
        );
    }

    #[test]
    fn color_changes_and_other_oscs_get_no_reply() {
        assert!(
            replies_to(
                b"\x1b]11;#ffffff\x07\x1b]4;1;rgb:ff/00/00\x07\x1b]12;?\x07\x1b]4;300;?\x07"
            )
            .is_empty()
        );
    }

    #[test]
    fn other_sequences_get_no_reply() {
        assert!(replies_to(b"\x1b[2 q\x1b[?u\x1b[1c\x1b[14t").is_empty());
    }
}
