mod opener;
mod palette;
mod releases;
mod sessions;

use std::io::{self, Write};

use crate::{
    overlay::{
        opener::{OpenerState, OpenerView},
        palette::{Arg, Cmd, CmdPalleteState, CmdPalleteView},
        releases::{ReleasesState, ReleasesView},
        sessions::{SessionsState, SessionsView},
    },
    renderer::{BackgroundRenderer, TerminalRenderer},
    App,
};
use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    style::{
        Attribute as CrosstermAttribute, Color as CrosstermColor, Colors as CrosstermColors, Print,
        SetAttribute, SetBackgroundColor, SetColors, SetForegroundColor,
    },
    terminal, Command,
};
use ratatui::{
    backend::{ClearType, WindowSize},
    crossterm::cursor::MoveTo,
    prelude::*,
    widgets::*,
};
use unicode_segmentation::UnicodeSegmentation as _;
use vt100::Parser;
use wgpu::{Device, Queue, RenderPass, TextureFormat};
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, KeyEvent},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
};

macro_rules! queue {
    ($writer:expr $(, $command:expr)* $(,)?) => {{
        Ok($writer.by_ref())
            $(.and_then(|writer| write_command_ansi(writer, $command)))*
            .map(|_| ())
    }}
}

macro_rules! execute {
    ($writer:expr $(, $command:expr)* $(,)?) => {{
        Ok($writer.by_ref())
            $(.and_then(|writer| write_command_ansi(writer, $command)))*
            .map(|_| ())
    }}
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Screen {
    CmdPalette,
    Sessions,
    Releases,
    Opener,
}

pub struct Overlay {
    terminal: Terminal<VtBackend>,
    size: [f32; 2],
    shown: bool,
    screen: Screen,

    release_state: ReleasesState,

    cmd_palette_state: CmdPalleteState,

    session_state: SessionsState,

    opener_state: OpenerState,
}

impl Overlay {
    pub fn handle_input(&mut self, app: &mut App, event: &KeyEvent) -> bool {
        let mods = &app.modifiers;
        let PhysicalKey::Code(code) = event.physical_key else {
            return false;
        };
        if event.state != ElementState::Pressed {
            return false;
        }
        match code {
            KeyCode::Escape => {
                self.toggle();
                app.request_redraw();
                return true;
            }
            KeyCode::KeyS if mods.contains(ModifiersState::CONTROL) => {
                self.screen = Screen::Sessions;
                app.request_redraw();
                return true;
            }
            KeyCode::KeyC if mods.contains(ModifiersState::CONTROL) => {
                self.screen = Screen::CmdPalette;
                app.request_redraw();
                return true;
            }
            KeyCode::KeyR if mods.contains(ModifiersState::CONTROL) => {
                self.screen = Screen::Releases;
                app.request_redraw();
                return true;
            }
            KeyCode::KeyO if mods.contains(ModifiersState::CONTROL) => {
                self.screen = Screen::Opener;
                app.request_redraw();
                return true;
            }
            _ => {}
        }
        match self.screen {
            Screen::CmdPalette => {
                if let Some(action) = self.cmd_palette_state.handle_events(app, code, event) {
                    action(self, app);
                    self.toggle();
                }
            }
            Screen::Sessions => {
                if self.session_state.handle_events(app, code) {
                    self.toggle();
                }
            }
            Screen::Releases => self.release_state.handle_events(app, code),
            Screen::Opener => self.opener_state.handle_events(app, code),
        }
        true
    }

    pub fn draw(&mut self, app: &mut App) -> Result<()> {
        self.terminal.draw(|frame| {
            let area = frame.area();
            let block = Block::default()
                .borders(Borders::all())
                .border_type(BorderType::Rounded);
            let block = match self.screen {
                Screen::Sessions => block.title("Sessions"),
                Screen::Releases => block.title("Releases"),
                Screen::Opener => block.title(format!(
                    "Opener - {}",
                    self.opener_state.explorer.cwd().display()
                )),
                Screen::CmdPalette => block,
            };
            let inner = block.inner(area);
            frame.render_widget(block, area);

            match self.screen {
                Screen::CmdPalette => {
                    frame.render_stateful_widget(
                        CmdPalleteView::default(),
                        inner,
                        &mut self.cmd_palette_state,
                    );
                }
                Screen::Sessions => {
                    frame.render_stateful_widget(
                        SessionsView::new(app),
                        inner,
                        &mut self.session_state,
                    );
                }
                Screen::Releases => {
                    frame.render_stateful_widget(
                        ReleasesView::default(),
                        inner,
                        &mut self.release_state,
                    );
                }
                Screen::Opener => {
                    frame.render_stateful_widget(
                        OpenerView::default(),
                        inner,
                        &mut self.opener_state,
                    );
                }
            }
        })?;
        Ok(())
    }
}

impl Overlay {
    const OVERLAY_SIZE_RATIO: [u32; 2] = [3, 2];
    const RELATIVE_POSITION: [f32; 2] = [2.0, 2.5];

    pub fn new(
        app: &App,
        size: PhysicalSize<u32>,
        font_size: f32,
        line_height: f32,
    ) -> Result<Self> {
        let size = PhysicalSize::new(
            size.width / Self::OVERLAY_SIZE_RATIO[0],
            size.height / Self::OVERLAY_SIZE_RATIO[1],
        );
        let rows = (size.height as f32 / line_height) as u16;
        let cols = (size.width as f32 / (font_size / 2.0)) as u16;
        let terminal = Terminal::new(VtBackend::new(rows, cols))?;
        let commands = [
            Cmd::new("close", [Arg::new("tab")], |_, _, _| {}),
            Cmd::new("next", [], |_, app, _| {
                app.switch_tab(app.next_tab_index());
                app.request_redraw();
            }),
            Cmd::new("prev", [], |_, app, _| {
                app.switch_tab(app.previous_tab_index());
                app.request_redraw();
            }),
            Cmd::new("switch", [Arg::new("tab")], |_, app, [tab]| {
                let Ok(tab) = tab.parse::<usize>() else {
                    return;
                };
                if tab > 9 || tab == 0 {
                    return;
                }
                app.switch_tab(tab - 1);
                app.request_redraw();
            }),
            Cmd::new("sessions", [], |this, app, _| {
                this.screen = Screen::Sessions;
                this.toggle();
                app.request_redraw();
            }),
            Cmd::new("releases", [], |this, app, _| {
                this.screen = Screen::Releases;
                this.toggle();
                app.request_redraw();
            }),
        ];

        Ok(Self {
            terminal,
            size: [size.width as f32, size.height as f32],
            shown: false,
            screen: Screen::CmdPalette,

            release_state: ReleasesState::new(app),

            cmd_palette_state: CmdPalleteState::new(commands),

            session_state: SessionsState::new(),

            opener_state: OpenerState::new(),
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>, font_size: f32, line_height: f32) {
        let size = PhysicalSize::new(
            size.width / Self::OVERLAY_SIZE_RATIO[0],
            size.height / Self::OVERLAY_SIZE_RATIO[1],
        );
        let rows = (size.height as f32 / line_height) as u16;
        let cols = (size.width as f32 / (font_size / 2.0)) as u16;
        self.terminal
            .backend_mut()
            .writer
            .screen_mut()
            .set_size(rows, cols);
        self.size = [size.width as f32, size.height as f32];
    }

    pub fn toggle(&mut self) {
        self.shown = !self.shown;
    }

    pub fn shown(&self) -> bool {
        self.shown
    }
}

//  NOTE: maybe avoid owning the renderers and simply reuse exsisting ones
//  need to enable depth buffer for this
pub struct OverlayRenderer {
    background_renderer: BackgroundRenderer,
    terminal_renderer: TerminalRenderer,
    font_size: f32,
    line_height: f32,
}

impl OverlayRenderer {
    pub fn new(
        device: &Device,
        font_family: Option<&str>,
        font_size: f32,
        line_height: f32,
        format: TextureFormat,
    ) -> Self {
        Self {
            background_renderer: BackgroundRenderer::new(device, format),
            terminal_renderer: TerminalRenderer::new(device, font_family, font_size, format),
            font_size,
            line_height,
        }
    }

    pub fn render(
        &mut self,
        size: PhysicalSize<u32>,
        device: &Device,
        queue: &Queue,
        pass: &mut RenderPass,
        overlay: &Overlay,
    ) {
        use crate::renderer::Color;

        let screen = overlay.terminal.backend().writer.screen();
        let screen_size = [size.width as f32, size.height as f32];
        let [w, h] = [
            self.font_size / size.width as f32,
            (self.line_height * 2.0) / size.height as f32,
        ];
        let [x_off, y_off] = Self::offset(screen_size, overlay.size);
        let (rows, cols) = screen.size();
        for row in 0..rows {
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let fg_color = match cell.fgcolor() {
                    vt100::Color::Default => Color::rgb(0xc6, 0xd0, 0xf5),
                    x => Color::from(x),
                };
                let bg_color = Color::from(cell.bgcolor());
                let x = (self.font_size / 2.0 * f32::from(col)) + x_off;
                let y = (self.line_height * f32::from(row) + 1.0) + y_off;
                {
                    let [x, y] = Self::ndc(size, [x, y]);
                    let bg_color = if cell.inverse() { fg_color } else { bg_color };
                    self.background_renderer
                        .add_rect(x, y, w, h, bg_color.to_linear());
                }
                let fg_color = if cell.inverse() { bg_color } else { fg_color };
                let contents = cell.contents();
                let bold = cell.bold();
                #[allow(clippy::if_not_else)]
                for cluster in contents.graphemes(true) {
                    if cluster.len() != 1 {
                        self.terminal_renderer.add_cluster(
                            queue,
                            [x, y],
                            screen_size,
                            cluster,
                            fg_color,
                            bold,
                        );
                    } else {
                        for ch in cluster.chars() {
                            self.terminal_renderer.add_glyph(
                                queue,
                                [x, y],
                                screen_size,
                                ch,
                                fg_color,
                                bold,
                            );
                        }
                    }
                }
            }
        }
        if !screen.hide_cursor() && screen.scrollback() == 0 {
            let (row, col) = screen.cursor_position();
            let x = (self.font_size / 2.0 * f32::from(col)) + x_off;
            let y = (self.line_height * f32::from(row) + 1.0) + y_off;
            let [x, y] = Self::ndc(size, [x, y]);
            let [w, h] = [
                (self.font_size) / size.width as f32,
                (self.line_height * 2.0) / size.height as f32,
            ];
            self.background_renderer
                .add_rect(x, y, w, h, [0.78, 0.82, 0.96, 0.45]);
        }
        self.background_renderer.render(device, queue, pass);
        self.terminal_renderer.render(device, queue, pass);
    }

    fn offset(screen_size: [f32; 2], area_size: [f32; 2]) -> [f32; 2] {
        [
            screen_size[0] / Overlay::RELATIVE_POSITION[0] - area_size[0] / 2.0,
            screen_size[1] / Overlay::RELATIVE_POSITION[1] - area_size[1] / 2.0,
        ]
    }

    fn ndc(size: PhysicalSize<u32>, pos: [f32; 2]) -> [f32; 2] {
        let [x, y] = pos;
        let nx = (x / size.width as f32) * 2.0 - 1.0;
        let ny = 1.0 - (y / size.height as f32) * 2.0;
        [nx, ny]
    }
}

struct VtBackend {
    writer: Parser,
}

impl VtBackend {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            writer: Parser::new(rows, cols, 2000),
        }
    }
}

impl Backend for VtBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut last_pos: Option<Position> = None;
        for (x, y, cell) in content {
            // Move the cursor if the previous location was not (x - 1, y)
            if !matches!(last_pos, Some(p) if x == p.x + 1 && y == p.y) {
                queue!(self.writer, MoveTo(x, y))?;
            }
            last_pos = Some(Position { x, y });
            if cell.modifier != modifier {
                let diff = ModifierDiff {
                    from: modifier,
                    to: cell.modifier,
                };
                diff.queue(&mut self.writer)?;
                modifier = cell.modifier;
            }
            if cell.fg != fg || cell.bg != bg {
                queue!(
                    self.writer,
                    SetColors(CrosstermColors::new(
                        cell.fg.into_crossterm(),
                        cell.bg.into_crossterm(),
                    ))
                )?;
                fg = cell.fg;
                bg = cell.bg;
            }

            queue!(self.writer, Print(cell.symbol()))?;
        }
        queue!(
            self.writer,
            SetForegroundColor(CrosstermColor::Reset),
            SetBackgroundColor(CrosstermColor::Reset),
            SetAttribute(CrosstermAttribute::Reset),
        )
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(self.writer, Show)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        let (row, col) = self.writer.screen().cursor_position();
        Ok(Position { x: col, y: row })
        /*crossterm::cursor::position()
        .map(|(x, y)| Position { x, y })
        .map_err(io::Error::other)*/
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let Position { x, y } = position.into();
        execute!(self.writer, MoveTo(x, y))
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        execute!(
            self.writer,
            crossterm::terminal::Clear(match clear_type {
                ClearType::All => crossterm::terminal::ClearType::All,
                ClearType::AfterCursor => crossterm::terminal::ClearType::FromCursorDown,
                ClearType::BeforeCursor => crossterm::terminal::ClearType::FromCursorUp,
                ClearType::CurrentLine => crossterm::terminal::ClearType::CurrentLine,
                ClearType::UntilNewLine => crossterm::terminal::ClearType::UntilNewLine,
            })
        )
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            queue!(self.writer, Print("\n"))?;
        }
        self.writer.flush()
    }

    fn size(&self) -> io::Result<Size> {
        let (rows, cols) = self.writer.screen().size();
        Ok(Size {
            width: cols,
            height: rows,
        })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let (rows, cols) = self.writer.screen().size();
        let crossterm::terminal::WindowSize { width, height, .. } =
            terminal::window_size().unwrap();
        Ok(WindowSize {
            columns_rows: Size {
                width: cols,
                height: rows,
            },
            pixels: Size { width, height },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W>(self, mut w: W) -> io::Result<()>
    where
        W: io::Write,
    {
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CrosstermAttribute::NoReverse))?;
        }

        let reset_intensity = removed.contains(Modifier::BOLD) || removed.contains(Modifier::DIM);
        if reset_intensity {
            // Bold and Dim are both reset by applying the Normal intensity
            queue!(w, SetAttribute(CrosstermAttribute::NormalIntensity))?;

            // The remaining Bold and Dim attributes must be
            // reapplied after the intensity reset above.
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CrosstermAttribute::Dim))?;
            }

            if self.to.contains(Modifier::BOLD) {
                queue!(w, SetAttribute(CrosstermAttribute::Bold))?;
            }
        }

        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CrosstermAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CrosstermAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CrosstermAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::HIDDEN) {
            queue!(w, SetAttribute(CrosstermAttribute::NoHidden))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CrosstermAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CrosstermAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) && !reset_intensity {
            queue!(w, SetAttribute(CrosstermAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CrosstermAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CrosstermAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) && !reset_intensity {
            queue!(w, SetAttribute(CrosstermAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CrosstermAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::HIDDEN) {
            queue!(w, SetAttribute(CrosstermAttribute::Hidden))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CrosstermAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CrosstermAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

pub trait IntoCrossterm<C> {
    fn into_crossterm(self) -> C;
}

impl IntoCrossterm<CrosstermColor> for Color {
    fn into_crossterm(self) -> CrosstermColor {
        match self {
            Self::Reset => CrosstermColor::Reset,
            Self::Black => CrosstermColor::Black,
            Self::Red => CrosstermColor::DarkRed,
            Self::Green => CrosstermColor::DarkGreen,
            Self::Yellow => CrosstermColor::DarkYellow,
            Self::Blue => CrosstermColor::DarkBlue,
            Self::Magenta => CrosstermColor::DarkMagenta,
            Self::Cyan => CrosstermColor::DarkCyan,
            Self::Gray => CrosstermColor::Grey,
            Self::DarkGray => CrosstermColor::DarkGrey,
            Self::LightRed => CrosstermColor::Red,
            Self::LightGreen => CrosstermColor::Green,
            Self::LightBlue => CrosstermColor::Blue,
            Self::LightYellow => CrosstermColor::Yellow,
            Self::LightMagenta => CrosstermColor::Magenta,
            Self::LightCyan => CrosstermColor::Cyan,
            Self::White => CrosstermColor::White,
            Self::Indexed(i) => CrosstermColor::AnsiValue(i),
            Self::Rgb(r, g, b) => CrosstermColor::Rgb { r, g, b },
        }
    }
}

fn write_command_ansi<W: std::io::Write, C: Command>(io: &mut W, command: C) -> io::Result<&mut W> {
    struct Adapter<T> {
        inner: T,
        res: io::Result<()>,
    }

    impl<T: Write> std::fmt::Write for Adapter<T> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.inner.write_all(s.as_bytes()).map_err(|e| {
                self.res = Err(e);
                std::fmt::Error
            })
        }
    }

    let mut adapter = Adapter {
        inner: io,
        res: Ok(()),
    };

    command
        .write_ansi(&mut adapter)
        .map_err(|std::fmt::Error| match adapter.res {
            Ok(()) => panic!(
                "<{}>::write_ansi incorrectly errored",
                std::any::type_name::<C>()
            ),
            Err(e) => e,
        })
        .map(|()| adapter.inner)
}
