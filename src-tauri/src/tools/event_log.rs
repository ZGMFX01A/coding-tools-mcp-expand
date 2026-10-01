//! Opt-in, local metadata journal. Arguments, file contents and outputs never enter records.
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub struct ToolEventJournal {
    directory: PathBuf,
    _writer: File,
    state: Mutex<bool>,
    max_bytes: u64,
}
static JOURNAL: OnceLock<Option<ToolEventJournal>> = OnceLock::new();

fn warn() {
    eprintln!("coding-tools-mcp: local event journal unavailable; recording disabled. Check permissions, writer ownership and disk space.");
}
pub fn configured() -> Option<&'static ToolEventJournal> {
    JOURNAL
        .get_or_init(|| {
            std::env::var_os("CODING_TOOLS_MCP_EVENT_LOG_DIR")
                .filter(|p| !p.is_empty())
                .and_then(|p| match ToolEventJournal::open(Path::new(&p), 1_048_576) {
                    Ok(journal) => Some(journal),
                    Err(_) => {
                        warn();
                        None
                    }
                })
        })
        .as_ref()
}
pub fn observe(name: &str, call: impl FnOnce() -> Value) -> Value {
    let journal = configured();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let start = Instant::now();
    // Unknown names supplied by a client must not become a data channel into the journal.
    let safe_name = if super::registry::is_allowed_tool(name) {
        name
    } else {
        "external_or_unknown_tool"
    };
    if let Some(j) = journal {
        j.record(&json!({"event":"tool_started","call_id":id,"tool":safe_name}));
    }
    let result = call();
    let payload = result.get("structuredContent").unwrap_or(&result);
    if let Some(j) = journal {
        let code = payload["error"]["code"].as_str().filter(|code| {
            code.len() <= 64
                && code
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit())
        });
        j.record(&json!({"event":"tool_finished","call_id":id,"tool":safe_name,"duration_ms":start.elapsed().as_millis(),
            "ok":payload["ok"].as_bool().unwrap_or(result["isError"]!=true),"error_code":code,
            "operation_outcome":super::reliability::operation_outcome(payload),"idempotent_replay":payload["idempotent_replay"]==true}));
    }
    result
}
pub fn observe_result(
    name: &str,
    call: impl FnOnce() -> Result<Value, String>,
) -> Result<Value, String> {
    let mut error = None;
    let value = observe(name, || match call() {
        Ok(value) => value,
        Err(message) => {
            error = Some(message);
            json!({"ok":false,"error":{"code":"EXTERNAL_TOOL_ERROR"}})
        }
    });
    match error {
        Some(message) => Err(message),
        None => Ok(value),
    }
}

impl ToolEventJournal {
    pub fn open(directory: &Path, max_bytes: u64) -> std::io::Result<Self> {
        if !directory.is_absolute() || max_bytes < 2 {
            return Err(invalid());
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
        check_private(&fs::symlink_metadata(directory)?, true)?;
        let lock = open_private(&directory.join("journal.lock"))?;
        lock.try_lock_exclusive()?;
        for index in 0..=3 {
            let path = directory.join(if index == 0 {
                "events.jsonl".into()
            } else {
                format!("events.jsonl.{index}")
            });
            match fs::symlink_metadata(path) {
                Ok(meta) => {
                    check_private(&meta, false)?;
                    if meta.len() > max_bytes {
                        return Err(invalid());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Self {
            directory: directory.into(),
            _writer: lock,
            state: Mutex::new(false),
            max_bytes,
        })
    }
    pub fn record(&self, event: &Value) {
        let mut disabled = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *disabled {
            return;
        }
        let result = (|| -> std::io::Result<()> {
            check_private(&fs::symlink_metadata(&self.directory)?, true)?;
            let mut event = event.clone();
            event["timestamp_ms"] = json!(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis());
            let mut encoded = serde_json::to_vec(&event)?;
            encoded.push(b'\n');
            if encoded.len() > 4096 || encoded.len() as u64 > self.max_bytes {
                return Err(invalid());
            }
            let path = self.directory.join("events.jsonl");
            let mut file = open_private(&path)?;
            let size = file.metadata()?.len();
            let mut separator = false;
            if size > 0 {
                file.seek(SeekFrom::End(-1))?;
                let mut byte = [0];
                file.read_exact(&mut byte)?;
                separator = byte[0] != b'\n';
            }
            if size + encoded.len() as u64 + u64::from(separator) > self.max_bytes {
                drop(file);
                for index in (1..=3).rev() {
                    let source = self.directory.join(if index == 1 {
                        "events.jsonl".into()
                    } else {
                        format!("events.jsonl.{}", index - 1)
                    });
                    let dest = self.directory.join(format!("events.jsonl.{index}"));
                    if let Ok(meta) = fs::symlink_metadata(&source) {
                        check_private(&meta, false)?;
                        match fs::symlink_metadata(&dest) {
                            Ok(meta) => check_private(&meta, false)?,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => return Err(e),
                        }
                        super::patch::replace_file(&source, &dest)?;
                    }
                }
                file = open_private(&path)?;
                separator = false;
            }
            if separator {
                file.write_all(b"\n")?;
            }
            file.write_all(&encoded)?;
            file.flush()?;
            Ok(())
        })();
        if result.is_err() {
            *disabled = true;
            warn();
        }
    }
    pub fn enabled(&self) -> bool {
        !*self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}
fn invalid() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "Invalid journal storage",
    )
}
fn check_private(meta: &fs::Metadata, directory: bool) -> std::io::Result<()> {
    if meta.file_type().is_symlink()
        || (directory && !meta.is_dir())
        || (!directory && !meta.is_file())
    {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::getuid() }
            || meta.mode() & 0o077 != 0
            || (!directory && meta.nlink() != 1)
        {
            return Err(invalid());
        }
    }
    Ok(())
}
fn open_private(path: &Path) -> std::io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(meta) => check_private(&meta, false)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path)?;
    check_private(&file.metadata()?, false)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_is_bounded_exclusive_and_recovers_partial_final_records() {
        let dir = tempfile::tempdir().unwrap();
        let journal = ToolEventJournal::open(dir.path(), 512).unwrap();
        assert!(ToolEventJournal::open(dir.path(), 512).is_err());
        for i in 0..30 {
            journal.record(&json!({"event":"tool_finished","index":i}));
        }
        assert!(journal.enabled());
        for entry in fs::read_dir(dir.path()).unwrap().flatten() {
            assert!(entry.metadata().unwrap().len() <= 512);
        }
        drop(journal);
        let path = dir.path().join("events.jsonl");
        let mut file = open_private(&path).unwrap();
        file.write_all(b"partial").unwrap();
        drop(file);
        let journal = ToolEventJournal::open(dir.path(), 512).unwrap();
        journal.record(&json!({"event":"tool_started"}));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("partial\n{") || !text.contains("partial"));
    }
}
