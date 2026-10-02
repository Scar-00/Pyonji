use std::{
    io::{Read as _, Write},
    net::IpAddr,
    path::Path,
};

use crate::terminal::SessionId;
use anyhow::{Context, Result};
use async_channel::Sender;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

#[derive(Debug, Clone)]
pub struct SshConnection {
    pub name: String,
    pub user_name: String,
    pub ip: IpAddr,
}

pub struct Pty {
    master: Box<dyn MasterPty>,
    writer: Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller>,
}

pub enum Event {
    Closed(SessionId),
    Data(SessionId, Vec<u8>),
    ProgramChanged((SessionId, String)),
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
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open pty pair")?;

        let program_name = Self::get_shell();
        let mut cmd = CommandBuilder::new(program_name);
        if let Some(path) = path {
            cmd.cwd(path);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        std::env::vars_os().for_each(|var| {
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

        let killer = child.clone_killer();

        std::thread::spawn({
            let tx = tx.clone();
            move || {
                _ = child.wait();
                _ = tx.send_blocking(Event::Closed(id));
            }
        });

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        _ = tx.send_blocking(Event::Closed(id));
                        break;
                    }
                    Ok(n) => {
                        _ = tx.send_blocking(Event::Data(id, buf[..n].to_vec()));
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer: Self::spawn_writer(writer),
            killer,
        })
    }

    pub fn new_remote(
        rows: u16,
        cols: u16,
        tx: Sender<Event>,
        id: SessionId,
        ssh: &SshConnection,
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

        let program_name = "ssh";
        let mut cmd = CommandBuilder::new(program_name);
        cmd.arg(format!("{}@{}", ssh.user_name, ssh.ip));
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        std::env::vars_os().for_each(|var| {
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

        let killer = child.clone_killer();

        std::thread::spawn({
            let tx = tx.clone();
            move || {
                _ = child.wait();
                _ = tx.send_blocking(Event::Closed(id));
            }
        });

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        _ = tx.send_blocking(Event::Closed(id));
                        break;
                    }
                    Ok(n) => {
                        _ = tx.send_blocking(Event::Data(id, buf[..n].to_vec()));
                    }
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer: Self::spawn_writer(writer),
            killer,
        })

        /*const SSH_SOCKET: Token = Token(0);
        const COMMAND_WAKER: Token = Token(1);

        let stream = TcpStream::connect(("192.168.178.20", 22))?;
        let readiness = stream.try_clone()?;
        readiness.set_nonblocking(true);
        let mut session = Session::new()?;
        session.set_tcp_stream(stream);
        session.handshake()?;

        session.userauth_password("ive", "FifI-0808")?;
        if !session.authenticated() {
            anyhow::bail!("failed to auth session");
        }
        session.set_blocking(false);

        let mut channel = session.channel_session()?;

        channel.request_pty(
            "xterm-256color",
            None,
            Some((u32::from(cols), u32::from(rows), 0, 0)),
        )?;

        channel.handle_extended_data(ExtendedData::Merge)?;
        channel.shell()?;

        let mut mio_socket = MioTcpStream::from_std(readiness);
        let mut poll = Poll::new()?;

        poll.registry().register(
            &mut mio_socket,
            SSH_SOCKET,
            Interest::READABLE,
        )?;

        let waker = Arc::new(Waker::new(
            poll.registry(),
            COMMAND_WAKER,
        )?);


        let mut waiter = channel.clone();
        let mut reader = channel.clone();

        std::thread::spawn({
            let tx = tx.clone();
            move || {
                _ = dbg!(waiter.wait_eof());
                _ = dbg!(waiter.wait_close());
                println!("closed");
                _ = tx.send_event(Event::Closed(id));
            }
        });
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        _ = tx.send_event(Event::Closed(id));
                        break;
                    }
                    Ok(n) => {
                        println!("data = {:?}", str::from_utf8(&buf[..n]));
                        _ = tx.send_event(Event::Data(id, buf[..n].to_vec()));
                    }
                }
            }
        });*/
    }

    fn spawn_writer(mut writer: Box<dyn Write + Send>) -> Sender<Vec<u8>> {
        // Keep input ordered without dropping bytes when the PTY stops reading.
        let (tx, rx) = async_channel::unbounded::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(buf) = rx.recv_blocking() {
                if writer.write_all(&buf).is_err() {
                    break;
                }
            }
        });
        tx
    }

    pub fn add_bytes(&mut self, buf: impl AsRef<[u8]>) {
        _ = self.writer.force_send(buf.as_ref().to_vec());
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

    pub fn resize(&mut self, rows: u16, cols: u16) {
        _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
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

            Path::new(&shell)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        sync::mpsc::{self, Receiver, Sender},
        time::Duration,
    };

    struct BlockingWriter {
        started: Sender<()>,
        release: Receiver<()>,
        finished: Sender<Vec<u8>>,
        bytes: Vec<u8>,
    }

    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.bytes.is_empty() {
                self.started.send(()).unwrap();
                self.release.recv().unwrap();
            }
            // Force write_all to retry partial writes.
            let len = buf.len().min(2);
            self.bytes.extend_from_slice(&buf[..len]);
            Ok(len)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for BlockingWriter {
        fn drop(&mut self) {
            let _ = self.finished.send(std::mem::take(&mut self.bytes));
        }
    }

    #[test]
    fn blocked_writer_preserves_queued_input_and_drains_on_close() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let tx = Pty::spawn_writer(Box::new(BlockingWriter {
            started: started_tx,
            release: release_rx,
            finished: finished_tx,
            bytes: Vec::new(),
        }));

        assert_eq!(tx.force_send(b"first".to_vec()).unwrap(), None);
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // Input must enqueue immediately even while the writer is blocked.
        assert_eq!(tx.force_send(b"second".to_vec()).unwrap(), None);
        assert_eq!(tx.force_send(b"third".to_vec()).unwrap(), None);
        for byte in 0..=u8::MAX {
            assert_eq!(tx.force_send(vec![byte]).unwrap(), None);
        }
        drop(tx);
        release_tx.send(()).unwrap();

        let mut expected = b"firstsecondthird".to_vec();
        expected.extend(0..=u8::MAX);
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected
        );
    }

    struct FailingWriter(Sender<()>);

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for FailingWriter {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn writer_exits_on_write_error_while_sender_is_alive() {
        let (finished_tx, finished_rx) = mpsc::channel();
        let tx = Pty::spawn_writer(Box::new(FailingWriter(finished_tx)));

        tx.force_send(b"input".to_vec()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(tx);
    }
}
