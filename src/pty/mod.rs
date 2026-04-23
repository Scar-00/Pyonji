use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use vswhom::VsFindResult;
use winit::event_loop::EventLoopProxy;

use crate::terminal::SessionId;

pub struct Pty {
    master: Box<dyn MasterPty>,
    writer: Box<dyn Write + Send>,
    process_id: Option<u32>,
    start_dir: Option<PathBuf>,
    program_name: String,
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

        let program_name = String::from("cmd.exe");
        let mut cmd = CommandBuilder::new(&program_name);
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
        let process_id = child.process_id();
        let start_dir = path
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok());

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
            process_id,
            start_dir,
            program_name,
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

    pub fn current_dir(&self) -> Option<PathBuf> {
        self.live_current_dir().or_else(|| self.start_dir.clone())
    }

    pub fn program_name(&self) -> &str {
        &self.program_name
    }

    #[cfg(windows)]
    fn live_current_dir(&self) -> Option<PathBuf> {
        use std::{
            mem::{size_of, zeroed},
            ptr::null_mut,
        };

        use ntapi::{
            ntpebteb::PEB,
            ntpsapi::{
                NtQueryInformationProcess, ProcessBasicInformation, PROCESS_BASIC_INFORMATION,
            },
            ntrtl::RTL_USER_PROCESS_PARAMETERS,
        };
        use winapi::um::{
            handleapi::CloseHandle,
            memoryapi::ReadProcessMemory,
            processthreadsapi::OpenProcess,
            winnt::{PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ},
        };

        let pid = self.process_id?;
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
            if process.is_null() {
                return None;
            }

            let mut basic_info: PROCESS_BASIC_INFORMATION = zeroed();
            let status = NtQueryInformationProcess(
                process,
                ProcessBasicInformation,
                &mut basic_info as *mut _ as *mut _,
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                null_mut(),
            );
            if status < 0 {
                CloseHandle(process);
                return None;
            }

            let mut peb: PEB = zeroed();
            let mut bytes_read = 0usize;
            if ReadProcessMemory(
                process,
                basic_info.PebBaseAddress as *const _,
                &mut peb as *mut _ as *mut _,
                size_of::<PEB>(),
                &mut bytes_read,
            ) == 0
            {
                CloseHandle(process);
                return None;
            }

            let mut params: RTL_USER_PROCESS_PARAMETERS = zeroed();
            if ReadProcessMemory(
                process,
                peb.ProcessParameters as *const _,
                &mut params as *mut _ as *mut _,
                size_of::<RTL_USER_PROCESS_PARAMETERS>(),
                &mut bytes_read,
            ) == 0
            {
                CloseHandle(process);
                return None;
            }

            let path = read_remote_unicode_string(process, params.CurrentDirectory.DosPath);
            CloseHandle(process);
            path
        }
    }

    #[cfg(not(windows))]
    fn live_current_dir(&self) -> Option<PathBuf> {
        None
    }
}

#[cfg(windows)]
fn read_remote_unicode_string(
    process: winapi::shared::ntdef::HANDLE,
    value: winapi::shared::ntdef::UNICODE_STRING,
) -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use winapi::um::memoryapi::ReadProcessMemory;

    if value.Buffer.is_null() || value.Length == 0 {
        return None;
    }

    let mut bytes_read = 0usize;
    let mut buffer = vec![0u16; (value.Length / 2) as usize];
    unsafe {
        if ReadProcessMemory(
            process,
            value.Buffer as *const _,
            buffer.as_mut_ptr() as *mut _,
            value.Length as usize,
            &mut bytes_read,
        ) == 0
        {
            return None;
        }
    }
    Some(PathBuf::from(OsString::from_wide(&buffer)))
}
