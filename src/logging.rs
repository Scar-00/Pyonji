use std::{backtrace::Backtrace, fs::File, io::Write, panic};

const LOG_FILE: &str = "C:/dev/learning/pyonji/pyonji.log";

pub fn init() {
    let previous_hook = panic::take_hook();

    panic::set_hook(Box::new(move |info| {
        if let Ok(mut fd) = File::create(LOG_FILE) {
            let backtrace = Backtrace::force_capture();
            _ = fd.write_fmt(format_args!("{info}\nstack backtrace:\n{backtrace}"));
        }
        previous_hook(info);
    }));
}
