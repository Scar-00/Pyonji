use std::io;
use tracing_core::callsite::Callsite as _;
use tracing_subscriber::fmt::MakeWriter;

use async_channel::{Receiver, Sender};

#[cfg(feature = "install")]
const LOG_FILE: &str = "C:/dev/learning/pyonji/pyonji.log";

#[cfg(feature = "install")]
pub fn init() {
    use crate::config;
    use std::{
        backtrace::Backtrace,
        fs::File,
        io::Write,
        panic,
        path::{Path, PathBuf},
    };

    let log_path = config::util::config_path();
    let log_path = log_path
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|path| path.join("pyonji.log"))
        .unwrap_or(PathBuf::from(LOG_FILE));

    let previous_hook = panic::take_hook();

    panic::set_hook(Box::new(move |info| {
        if let Ok(mut fd) = File::create(log_path.clone()) {
            let backtrace = Backtrace::force_capture();
            _ = fd.write_fmt(format_args!("{info}\nstack backtrace:\n{backtrace}"));
        }
        previous_hook(info);
    }));
}

pub struct LogEvent {
    pub level: tracing::Level,
    pub line: String,
}

pub struct LogEmitter {
    tx: Sender<LogEvent>,
    rx: Receiver<LogEvent>,
}

impl LogEmitter {
    pub fn new() -> Self {
        let (tx, rx) = async_channel::bounded(128);
        Self { tx, rx }
    }

    pub async fn recv(&self) -> Option<LogEvent> {
        self.rx.recv().await.ok()
    }
}

pub struct TracingLogSubscriber {
    tx: Sender<LogEvent>,
}

impl TracingLogSubscriber {
    pub fn new(emitter: &LogEmitter) -> Self {
        Self {
            tx: emitter.tx.clone(),
        }
    }
}

pub struct ChannelWriter {
    tx: Sender<LogEvent>,
    level: tracing::Level,
}

impl io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let line = String::from_utf8_lossy(buf).trim_end().to_string();
        let _ = self.tx.force_send(LogEvent {
            level: self.level,
            line,
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for TracingLogSubscriber {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
            level: tracing::Level::INFO,
        }
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        ChannelWriter {
            tx: self.tx.clone(),
            level: *meta.level(),
        }
    }
}

/// Fire-and-forget logging for `Result`s: `result.log()` emits a
/// `tracing` event on `Err` and drops both `Ok` and `Err`.
///
/// Unlike a plain `tracing::error!` wrapper (which would attribute the event
/// to this module), the event is dispatched with metadata built from the
/// *caller's* location, so target/file/line point at the `.log()` call site.
#[allow(unused)]
pub trait ResultLogExt {
    fn log(self);
    fn log_msg(self, msg: &str);
    fn warn(self);
    fn warn_msg(self, msg: &str);
}

/// Field names for the dynamically-dispatched events below.
static RESULT_LOG_FIELD_NAMES: &[&str] = &["message", "error"];

/// A callsite allocated once per distinct `.log()` call location. `tracing`
/// requires metadata to be `'static`, so one of these is leaked per call
/// site (not per call) and cached globally.
struct CallerCallsite {
    meta: std::sync::OnceLock<&'static tracing_core::Metadata<'static>>,
}

impl tracing_core::callsite::Callsite for CallerCallsite {
    fn set_interest(&self, _: tracing_core::subscriber::Interest) {}
    fn metadata(&self) -> &tracing_core::Metadata<'_> {
        self.meta.get().expect("CallerCallsite metadata not set")
    }
}

fn callsite_cache() -> &'static std::sync::Mutex<
    std::collections::HashMap<(&'static str, u32, bool), &'static CallerCallsite>,
> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<(&'static str, u32, bool), &'static CallerCallsite>,
        >,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Derive a `module_path`-style target (`pyonji::renderer::glyph`) from a
/// caller file inside this package; falls back to the crate name.
fn target_for_caller(file: &'static str) -> &'static str {
    const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
    const CRATE_NAME: &str = env!("CARGO_CRATE_NAME");
    // Absolute path inside this package (`/home/.../pyonji/src/foo.rs`) or
    // the relative form rustc sometimes emits (`src/foo.rs`).
    let rel = match file
        .strip_prefix(MANIFEST_DIR)
        .and_then(|p| p.strip_prefix(['/', '\\']))
    {
        Some(p) => p.strip_prefix("src/"),
        None => file.strip_prefix("./").unwrap_or(file).strip_prefix("src/"),
    }
    .and_then(|p| p.strip_suffix(".rs"));
    let Some(rel) = rel else {
        return CRATE_NAME;
    };
    let mut parts: Vec<&str> = rel.split(['/', '\\']).collect();
    if parts.last() == Some(&"mod") {
        parts.pop();
    }
    if parts == ["main"] || parts == ["lib"] {
        return CRATE_NAME;
    }
    // Leaked once per distinct call site (entries are cached globally).
    Box::leak(format!("{CRATE_NAME}::{}", parts.join("::")).into_boxed_str())
}

fn metadata_for_caller(
    level: tracing_core::Level,
    loc: &'static std::panic::Location<'static>,
) -> &'static tracing_core::Metadata<'static> {
    let is_error = level == tracing_core::Level::ERROR;
    let mut cache = callsite_cache().lock().unwrap();
    if let Some(cs) = cache.get(&(loc.file(), loc.line(), is_error)) {
        return cs.metadata();
    }
    let target = target_for_caller(loc.file());
    let name: &'static str =
        Box::leak(format!("event {}:{}", loc.file(), loc.line()).into_boxed_str());
    let cs: &'static CallerCallsite = Box::leak(Box::new(CallerCallsite {
        meta: std::sync::OnceLock::new(),
    }));
    let fieldset = tracing_core::field::FieldSet::new(
        RESULT_LOG_FIELD_NAMES,
        tracing_core::identify_callsite!(cs),
    );
    let meta: &'static tracing_core::Metadata<'static> =
        Box::leak(Box::new(tracing_core::Metadata::new(
            name,
            target,
            level,
            Some(loc.file()),
            Some(loc.line()),
            Some(target),
            fieldset,
            tracing_core::metadata::Kind::EVENT,
        )));
    cs.meta.set(meta).expect("fresh callsite");
    cache.insert((loc.file(), loc.line(), is_error), cs);
    meta
}

fn emit_at_caller(
    level: tracing_core::Level,
    loc: &'static std::panic::Location<'static>,
    message: String,
    error: String,
) {
    let meta = metadata_for_caller(level, loc);
    let fields = meta.fields();
    let message_field = fields.field("message").expect("message field");
    let error_field = fields.field("error").expect("error field");
    // `message` renders as the event message; `error` as a structured field.
    let values = [
        (
            &message_field,
            Some(&message as &dyn tracing_core::field::Value),
        ),
        (
            &error_field,
            Some(&error as &dyn tracing_core::field::Value),
        ),
    ];
    tracing_core::dispatcher::get_default(|dispatch| {
        if dispatch.enabled(meta) {
            dispatch.event(&tracing_core::Event::new(meta, &fields.value_set(&values)));
        }
    });
}

impl<T, E> ResultLogExt for Result<T, E>
where
    E: std::fmt::Display + std::fmt::Debug,
{
    #[track_caller]
    fn log(self) {
        if let Err(error) = self {
            emit_at_caller(
                tracing_core::Level::ERROR,
                std::panic::Location::caller(),
                format!("unhandled error: {error:?}"),
                error.to_string(),
            );
        }
    }

    #[track_caller]
    fn log_msg(self, msg: &str) {
        if let Err(error) = self {
            emit_at_caller(
                tracing_core::Level::ERROR,
                std::panic::Location::caller(),
                format!("{msg}: {error:?}"),
                error.to_string(),
            );
        }
    }

    #[track_caller]
    fn warn(self) {
        if let Err(error) = self {
            emit_at_caller(
                tracing_core::Level::WARN,
                std::panic::Location::caller(),
                format!("unhandled error: {error:?}"),
                error.to_string(),
            );
        }
    }

    #[track_caller]
    fn warn_msg(self, msg: &str) {
        if let Err(error) = self {
            emit_at_caller(
                tracing_core::Level::WARN,
                std::panic::Location::caller(),
                format!("{msg}: {error:?}"),
                error.to_string(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tracing_core::{Event, Metadata, subscriber::Subscriber};

    struct CapturedEvent {
        level: tracing_core::Level,
        target: String,
        file: Option<String>,
        line: Option<u32>,
        message: String,
    }

    #[derive(Default)]
    struct MessageVisitor {
        message: Option<String>,
    }

    impl tracing_core::field::Visit for MessageVisitor {
        fn record_debug(
            &mut self,
            field: &tracing_core::field::Field,
            value: &dyn std::fmt::Debug,
        ) {
            if field.name() == "message" {
                self.message = Some(format!("{value:?}"));
            }
        }
    }

    #[test]
    fn result_log_emits_error() {
        use std::sync::Arc;
        let events: Arc<Mutex<Vec<CapturedEvent>>> = Arc::new(Mutex::new(Vec::new()));
        struct Shared(Arc<Mutex<Vec<CapturedEvent>>>);
        impl Subscriber for Shared {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing_core::span::Attributes<'_>) -> tracing_core::span::Id {
                tracing_core::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing_core::span::Id, _: &tracing_core::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing_core::span::Id, _: &tracing_core::span::Id) {}
            fn event(&self, event: &Event<'_>) {
                let mut visitor = MessageVisitor::default();
                event.record(&mut visitor);
                self.0.lock().unwrap().push(CapturedEvent {
                    level: *event.metadata().level(),
                    target: event.metadata().target().to_string(),
                    file: event.metadata().file().map(str::to_string),
                    line: event.metadata().line(),
                    message: visitor.message.unwrap_or_default(),
                });
            }
            fn enter(&self, _: &tracing_core::span::Id) {}
            fn exit(&self, _: &tracing_core::span::Id) {}
        }
        let call_line = std::cell::Cell::new(0u32);
        tracing::subscriber::with_default(Shared(events.clone()), || {
            let ok: anyhow::Result<()> = Ok(());
            ok.log(); // must not emit
            call_line.set(line!() + 1);
            let err: anyhow::Result<()> = Err(anyhow::anyhow!("demo boom"));
            err.log_msg("demo context");
        });
        let call_line = call_line.get();
        {
            let guard = events.lock().unwrap();
            assert_eq!(guard.len(), 1, "expected exactly one event");
            let ev = &guard[0];
            assert_eq!(ev.level, tracing_core::Level::ERROR);
            // Attributed to the caller (this test in src/logging.rs), not to the
            // helper's own module path alone: target derives from the caller file.
            assert_eq!(ev.target, "pyonji::logging");
            assert!(
                ev.file
                    .as_deref()
                    .is_some_and(|f| f.ends_with("src/logging.rs")),
                "file: {:?}",
                ev.file
            );
            assert_eq!(
                ev.line,
                Some(call_line + 1),
                "event line should be the .log_msg() call"
            );
            assert!(ev.message.contains("demo boom"), "message: {}", ev.message);
            assert!(
                ev.message.contains("demo context"),
                "message: {}",
                ev.message
            );
        }

        // Same for the warn variants.
        tracing::subscriber::with_default(Shared(events.clone()), || {
            let w: anyhow::Result<()> = Err(anyhow::anyhow!("demo warn"));
            w.warn_msg("demo warn context");
        });
        let guard = events.lock().unwrap();
        assert_eq!(guard.len(), 2, "expected error + warn events");
        assert_eq!(guard[1].level, tracing_core::Level::WARN);
        assert!(
            guard[1].message.contains("demo warn"),
            "message: {}",
            guard[1].message
        );
    }
}
