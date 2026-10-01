use std::{
    io::{Read as _, Write},
    net::IpAddr,
    path::Path,
};

use crate::terminal::SessionId;
use anyhow::{Context, Result};
use async_channel::Sender;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

#[derive(Debug, Clone)]
pub struct SshConnection {
    pub name: String,
    pub user_name: String,
    pub ip: IpAddr,
}

pub struct Pty {
    master: Box<dyn MasterPty>,
    writer: std::sync::mpsc::Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller>,
}

pub enum Event {
    Closed(SessionId),
    Data(SessionId, Vec<u8>),
    ConfigChanged,
    LuaPrint(String),
}

impl Pty {
    pub fn new(
        rows: u16,
        cols: u16,
        tx: Sender<Event>,
        id: SessionId,
        path: Option<&Path>,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new(Self::get_shell());
        if let Some(path) = path {
            cmd.cwd(path);
        }
        Self::spawn(rows, cols, tx, id, cmd)
    }

    pub fn new_remote(
        rows: u16,
        cols: u16,
        tx: Sender<Event>,
        id: SessionId,
        ssh: &SshConnection,
    ) -> Result<Self> {
        let mut cmd = CommandBuilder::new("ssh");
        cmd.arg(format!("{}@{}", ssh.user_name, ssh.ip));
        Self::spawn(rows, cols, tx, id, cmd)
    }

    fn spawn(
        rows: u16,
        cols: u16,
        tx: Sender<Event>,
        id: SessionId,
        mut cmd: CommandBuilder,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;
        for (key, value) in std::env::vars_os() {
            cmd.env(key, value);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn terminal process")?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let mut writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;
        let (write_tx, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(bytes) = write_rx.recv() {
                if let Err(error) = writer.write_all(&bytes) {
                    tracing::error!(%error, %id, "PTY write failed");
                    break;
                }
            }
        });
        // Only the reader closes a session, after all output has been delivered.
        // Reap the process separately: descendants may keep the PTY open after it exits.
        std::thread::spawn(move || {
            if let Err(error) = child.wait() {
                tracing::warn!(%error, %id, "PTY process wait failed");
            }
        });
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx
                            .send_blocking(Event::Data(id, buf[..n].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            _ = tx.send_blocking(Event::Closed(id));
        });
        Ok(Self {
            master: pair.master,
            writer: write_tx,
            killer,
        })
    }

    pub fn add_bytes(&mut self, buf: impl AsRef<[u8]>) {
        if self.writer.send(buf.as_ref().to_vec()).is_err() {
            tracing::error!("PTY writer disconnected");
        }
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
            self.add_bytes(format!("\x1b[{byte};{m}~").as_bytes());
        } else {
            self.add_bytes(format!("\x1b[{byte}~").as_bytes());
        }
    }

    pub fn add_cursor_key(&mut self, csi_param: Option<u8>, byte: u8, app_cursor: bool) {
        if app_cursor && csi_param.is_none() {
            self.add_bytes([0x1b, b'O', byte]);
        } else {
            self.add_csi_key(csi_param, byte);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
    }

    pub fn kill(&mut self) {
        _ = self.killer.kill();
    }

    fn get_shell() -> String {
        fn from_env() -> Option<String> {
            use std::env;
            let shell = cfg_select! {
                unix => env::var_os("SHELL")?,
                windows => env::var_os("COMSPEC")?,
            };

            Some(shell.to_string_lossy().into_owned())
        }
        from_env().unwrap_or_else(|| {
            cfg_select! {
                unix => "bash",
                windows => "cmd.exe",
            }
            .to_string()
        })
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        _ = self.killer.kill();
    }
}
