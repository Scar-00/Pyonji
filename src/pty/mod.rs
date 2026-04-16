use std::{ffi::OsString, io::Write, path::Path};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use vswhom::VsFindResult;
use winit::event_loop::EventLoopProxy;

use crate::terminal::SessionId;

pub struct Pty {
    master: Box<dyn MasterPty>,
    writer: Box<dyn Write + Send>,
}

pub enum Event {
    Closed(SessionId),
    Data(SessionId, Vec<u8>),
}

impl Pty {
    pub fn new(
        rows: u16,
        cols: u16,
        tx: EventLoopProxy<Event>,
        id: SessionId,
        path: Option<&Path>,
    ) -> Result<Self> {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open pty pair")?;

        let mut cmd = CommandBuilder::new("cmd.exe");
        if let Some(path) = path {
            cmd.cwd(path);
        }
        cmd.env("TERM", "xterm-256color");
        let vs = VsFindResult::search();
        std::env::vars_os().for_each(|mut var| {
            if var.0 == "PATH" {
                if let Some(ref vs) = vs {
                    vs.windows_sdk_um_library_path.as_ref().map(|path| {
                        var.1.push(OsString::from(format!(";{}", path.display())));
                    });
                    vs.windows_sdk_ucrt_library_path.as_ref().map(|path| {
                        var.1.push(OsString::from(format!(";{}", path.display())));
                    });
                    vs.windows_sdk_root.as_ref().map(|path| {
                        var.1.push(OsString::from(format!(";{}", path.display())));
                    });
                    vs.vs_exe_path.as_ref().map(|path| {
                        var.1.push(OsString::from(format!(";{}", path.display())));
                    });
                }
            }
            cmd.env(var.0, var.1);
        });

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn cmd.exe")?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        std::thread::spawn({
            let tx = tx.clone();
            move || {
                _ = child.wait();
                _ = tx.send_event(Event::Closed(id));
            }
        });

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        _ = tx.send_event(Event::Closed(id));
                        break;
                    }
                    Ok(n) => {
                        _ = tx.send_event(Event::Data(id, buf[..n].to_vec()));
                    }
                    Err(_) => {
                        _ = tx.send_event(Event::Closed(id));
                        break;
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
        })
    }

    pub fn add_bytes(&mut self, buf: impl AsRef<[u8]>) {
        _ = self.writer.write(buf.as_ref());
    }

    pub fn add_csi_key(&mut self, csi_param: Option<u8>, byte: u8) {
        if let Some(m) = csi_param {
            self.add_bytes(format!("\x1b[1;{}{}", m, byte as char).as_bytes());
        } else {
            self.add_bytes([0x1b, b'[', byte]);
        }
    }

    pub fn add_csi_tilde(&mut self, csi_param: Option<u8>, byte: u8) {
        if let Some(m) = csi_param {
            self.add_bytes(format!("\x1b[{};{}~", byte, m).as_bytes());
        } else {
            self.add_bytes(format!("\x1b[{}~", byte).as_bytes());
        }
    }

    pub fn add_cursor_key(&mut self, csi_param: Option<u8>, byte: u8, app_cursor: bool) {
        if app_cursor && csi_param.is_none() {
            self.add_bytes([0x1b, b'O', byte]);
        } else {
            self.add_csi_key(csi_param, byte);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}
