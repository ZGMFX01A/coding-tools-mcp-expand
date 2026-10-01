use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdin};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::tools::workspace::{tool_ok, WorkspaceError};
use serde_json::{json, Value};

const SESSION_STREAM_TOTAL_BUDGET_BYTES: usize = 1_048_576;
const SESSION_HEAD_DIVISOR: usize = 8;
const SESSION_HEAD_BUFFER_BYTES: usize = SESSION_STREAM_TOTAL_BUDGET_BYTES / SESSION_HEAD_DIVISOR;
const SESSION_TAIL_BUFFER_BYTES: usize =
    SESSION_STREAM_TOTAL_BUDGET_BYTES - SESSION_HEAD_BUFFER_BYTES;

pub struct SessionStore {
    slots: Arc<tokio::sync::Semaphore>,
    sessions: Mutex<HashMap<String, Arc<ExecSession>>>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(16)),
        }
    }
}
impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve(&self) -> Result<tokio::sync::OwnedSemaphorePermit, WorkspaceError> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| WorkspaceError::Tool {
                code: "COMMAND_LIMIT",
                message: "At most 16 commands may run concurrently; wait for an active command"
                    .into(),
                category: "runtime",
                retryable: true,
            })
    }
    fn prune(sessions: &mut HashMap<String, Arc<ExecSession>>) {
        sessions.retain(|_, session| {
            session
                .finished_at
                .lock()
                .expect("finish lock")
                .is_none_or(|at| at.elapsed().as_secs() < 300)
        });
        let mut completed = sessions
            .iter()
            .filter_map(|(id, session)| {
                session
                    .finished_at
                    .lock()
                    .expect("finish lock")
                    .map(|at| (id.clone(), at))
            })
            .collect::<Vec<_>>();
        completed.sort_by_key(|v| v.1);
        let excess = completed.len().saturating_sub(32);
        for (id, _) in completed.into_iter().take(excess) {
            sessions.remove(&id);
        }
    }
    pub fn insert(&self, session: ExecSession) -> Arc<ExecSession> {
        let arc = Arc::new(session);
        let mut sessions = self.sessions.lock().expect("sessions lock");
        Self::prune(&mut sessions);
        sessions.insert(arc.session_id.clone(), arc.clone());
        arc
    }
    pub fn get(&self, session_id: &str) -> Result<Arc<ExecSession>, WorkspaceError> {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        Self::prune(&mut sessions);
        sessions.get(session_id).cloned().ok_or_else(|| WorkspaceError::ToolDetails {
            code:"SESSION_NOT_FOUND",message:format!("Session not found: {session_id}; completed output is retained for 300 seconds and the last 32 commands"),category:"not_found",retryable:false,
            details:json!({"completed_command_ttl_seconds":300,"max_retained_output_commands":32,"recovery_hint":"Start the command again and use its returned session_id"})
        })
    }
    pub fn remove(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("sessions lock")
            .remove(session_id);
    }
}

pub struct ExecSession {
    pub session_id: String,
    pub(crate) child: AsyncMutex<Child>,
    pub stdin: AsyncMutex<Option<ChildStdin>>,
    stdin_open: Mutex<bool>,
    interactive: bool,
    stdout: Mutex<Vec<u8>>,
    stderr: Mutex<Vec<u8>>,
    stdout_head: Mutex<Vec<u8>>,
    stderr_head: Mutex<Vec<u8>>,
    stdout_total: Mutex<usize>,
    stderr_total: Mutex<usize>,
    stdout_dropped_bytes: Mutex<usize>,
    stderr_dropped_bytes: Mutex<usize>,
    pub started_at: Instant,
    finished_at: Mutex<Option<Instant>>,
    spawn_permit: Mutex<Option<tokio::sync::OwnedSemaphorePermit>>,
    command_temp: Mutex<Option<super::mutation::CommandTempDir>>,
    workspace_may_have_changed: Mutex<bool>,
    pub exit_code: Mutex<Option<i32>>,
    exited: AtomicBool,
    termination_reason: Mutex<Option<String>>,
    reader_tasks: AsyncMutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
}

impl ExecSession {
    pub fn new(child: Child) -> Self {
        Self::new_with_mode(child, false)
    }

    pub fn new_with_mode(mut child: Child, interactive: bool) -> Self {
        let session_id = Uuid::new_v4().to_string();
        let stdin = child.stdin.take();
        let stdin_open = stdin.is_some();
        Self {
            session_id,
            child: AsyncMutex::new(child),
            stdin: AsyncMutex::new(stdin),
            stdin_open: Mutex::new(stdin_open),
            interactive,
            stdout: Mutex::new(Vec::new()),
            stderr: Mutex::new(Vec::new()),
            stdout_head: Mutex::new(Vec::new()),
            stderr_head: Mutex::new(Vec::new()),
            stdout_total: Mutex::new(0),
            stderr_total: Mutex::new(0),
            stdout_dropped_bytes: Mutex::new(0),
            stderr_dropped_bytes: Mutex::new(0),
            started_at: Instant::now(),
            finished_at: Mutex::new(None),
            spawn_permit: Mutex::new(None),
            command_temp: Mutex::new(None),
            workspace_may_have_changed: Mutex::new(true),
            exit_code: Mutex::new(None),
            exited: AtomicBool::new(false),
            termination_reason: Mutex::new(None),
            reader_tasks: AsyncMutex::new(Vec::new()),
        }
    }

    pub(crate) fn attach_permit(&self, permit: tokio::sync::OwnedSemaphorePermit) {
        *self.spawn_permit.lock().expect("permit lock") = Some(permit);
    }
    pub(crate) fn attach_command_temp(&self, directory: Option<super::mutation::CommandTempDir>) {
        *self.command_temp.lock().expect("command temp lock") = directory;
    }
    pub(crate) fn set_mutation_capability(&self, allowed: bool) {
        *self
            .workspace_may_have_changed
            .lock()
            .expect("mutation lock") = allowed;
    }
    pub async fn spawn_readers(self: &Arc<Self>) {
        let stdout = {
            let mut guard = self.child.lock().await;
            guard.stdout.take()
        };
        let stderr = {
            let mut guard = self.child.lock().await;
            guard.stderr.take()
        };
        if let Some(stream) = stdout {
            let session = Arc::clone(self);
            let task = tauri::async_runtime::spawn(async move {
                session.read_stream(stream, true).await;
            });
            self.reader_tasks.lock().await.push(task);
        }
        if let Some(stream) = stderr {
            let session = Arc::clone(self);
            let task = tauri::async_runtime::spawn(async move {
                session.read_stream(stream, false).await;
            });
            self.reader_tasks.lock().await.push(task);
        }
    }

    pub async fn wait_for_readers(&self) {
        let mut tasks = self.reader_tasks.lock().await;
        while let Some(task) = tasks.pop() {
            let _ = tokio::time::timeout(std::time::Duration::from_millis(500), task).await;
        }
    }

    async fn read_stream<T>(&self, mut stream: T, is_stdout: bool)
    where
        T: tokio::io::AsyncRead + Unpin,
    {
        let mut buf = [0u8; 4096];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    if is_stdout {
                        {
                            let mut head = self.stdout_head.lock().expect("stdout_head lock");
                            let head_cap = SESSION_HEAD_BUFFER_BYTES.saturating_sub(head.len());
                            if head_cap > 0 {
                                head.extend_from_slice(&chunk[..head_cap.min(chunk.len())]);
                            }
                        }
                        let mut data = self.stdout.lock().expect("stdout lock");
                        data.extend_from_slice(chunk);
                        *self.stdout_total.lock().expect("stdout_total lock") += n;
                        let dropped = trim_buffer(&mut data, SESSION_TAIL_BUFFER_BYTES);
                        if dropped > 0 {
                            *self
                                .stdout_dropped_bytes
                                .lock()
                                .expect("stdout_dropped lock") += dropped;
                        }
                    } else {
                        {
                            let mut head = self.stderr_head.lock().expect("stderr_head lock");
                            let head_cap = SESSION_HEAD_BUFFER_BYTES.saturating_sub(head.len());
                            if head_cap > 0 {
                                head.extend_from_slice(&chunk[..head_cap.min(chunk.len())]);
                            }
                        }
                        let mut data = self.stderr.lock().expect("stderr lock");
                        data.extend_from_slice(chunk);
                        *self.stderr_total.lock().expect("stderr_total lock") += n;
                        let dropped = trim_buffer(&mut data, SESSION_TAIL_BUFFER_BYTES);
                        if dropped > 0 {
                            *self
                                .stderr_dropped_bytes
                                .lock()
                                .expect("stderr_dropped lock") += dropped;
                        }
                    }
                }
                Err(_) => break,
            }
        }
    }

    pub async fn kill_and_wait(&self) {
        let (pid, status) = {
            let mut child = self.child.lock().await;
            let pid = child.id();
            let _ = child.start_kill();
            let status = child.wait().await.ok();
            (pid, status)
        };
        #[cfg(target_os = "windows")]
        if let Some(pid) = pid {
            let _ = crate::platform::platform().terminate_process_tree(pid);
        }
        if let Some(status) = status {
            self.record_exit_status(status);
        }
    }

    pub async fn refresh_status(&self) {
        let mut child = self.child.lock().await;
        if let Ok(Some(status)) = child.try_wait() {
            self.record_exit_status(status);
        }
    }

    fn record_exit_status(&self, status: std::process::ExitStatus) {
        *self.exit_code.lock().expect("exit_code lock") = status.code();
        self.exited.store(true, Ordering::Release);
        self.finished_at
            .lock()
            .expect("finish lock")
            .get_or_insert_with(Instant::now);
        self.spawn_permit.lock().expect("permit lock").take();
        self.command_temp.lock().expect("command temp lock").take();
        *self.stdin_open.lock().expect("stdin_open lock") = false;
        let mut reason = self.termination_reason.lock().expect("termination lock");
        if reason.is_none() {
            let natural_reason = "exited";
            #[cfg(unix)]
            let natural_reason = {
                use std::os::unix::process::ExitStatusExt;
                if status.signal().is_some() {
                    "crashed"
                } else {
                    natural_reason
                }
            };
            *reason = Some(natural_reason.into());
        }
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub fn mark_termination_reason(&self, reason: &str) {
        *self.termination_reason.lock().expect("termination lock") = Some(reason.to_string());
    }

    pub(crate) fn mark_stdin_closed(&self) {
        *self.stdin_open.lock().expect("stdin_open lock") = false;
    }

    pub async fn is_running(&self) -> bool {
        self.refresh_status().await;
        !self.has_exited()
    }

    pub fn retained_stream_bytes(&self, stream: &str) -> (Vec<u8>, usize) {
        match stream {
            "stderr" => {
                let data = self.stderr.lock().expect("stderr lock").clone();
                let total = *self.stderr_total.lock().expect("stderr_total lock");
                (data, total)
            }
            _ => {
                let data = self.stdout.lock().expect("stdout lock").clone();
                let total = *self.stdout_total.lock().expect("stdout_total lock");
                (data, total)
            }
        }
    }

    pub fn snapshot(&self, max_output_bytes: usize) -> Value {
        let stdout_tail = self.stdout.lock().expect("stdout lock").clone();
        let stderr_tail = self.stderr.lock().expect("stderr lock").clone();
        let stdout_head = self.stdout_head.lock().expect("stdout_head lock").clone();
        let stderr_head = self.stderr_head.lock().expect("stderr_head lock").clone();
        let stdout_total = *self.stdout_total.lock().expect("stdout_total lock");
        let stderr_total = *self.stderr_total.lock().expect("stderr_total lock");
        let stdout_dropped = *self
            .stdout_dropped_bytes
            .lock()
            .expect("stdout_dropped lock");
        let stderr_dropped = *self
            .stderr_dropped_bytes
            .lock()
            .expect("stderr_dropped lock");

        let stdout = truncate_head_tail(
            &stdout_head,
            &stdout_tail,
            stdout_total,
            stdout_dropped,
            max_output_bytes,
        );
        let stderr = truncate_head_tail(
            &stderr_head,
            &stderr_tail,
            stderr_total,
            stderr_dropped,
            max_output_bytes,
        );
        let exit_code = *self.exit_code.lock().expect("exit_code lock");
        let termination_reason = self
            .termination_reason
            .lock()
            .expect("termination lock")
            .clone();
        let status = if self.has_exited() {
            "exited"
        } else {
            "running"
        };
        let reason = termination_reason.as_deref().unwrap_or("running");
        let command_ok = match reason {
            "exited" => Some(exit_code.is_some_and(|code| code == 0)),
            "running" => None,
            _ => Some(false),
        };
        json!({
            "session_id": self.session_id,
            "workspace_may_have_changed": *self.workspace_may_have_changed.lock().expect("mutation lock"),
            "interactive": self.interactive,
            "stdin_open": *self.stdin_open.lock().expect("stdin_open lock"),
            "status": status,
            "termination_reason": reason,
            "operation_outcome": match reason { "running"=>"running", "timeout"=>"timeout", "spawn_failed"=>"spawn_error", "killed"|"crashed"=>"signal", _ if exit_code==Some(0)=>"exited_0", _=>"exited_nonzero" },
            "recoverable": matches!(reason, "timeout" | "killed" | "spawn_failed" | "server_restart"),
            "suggestion": match reason {
                "timeout" => "读取保留输出，调整 timeout_ms 后重试",
                "killed" => "确认终止原因后重新执行命令",
                "exited" => "检查 exit_code 和 stderr",
                "crashed" => "检查 stderr 后重试或恢复工作区",
                _ => "继续读取 session 或等待进程结束",
            },
            "exit_code": exit_code,
            "transport_ok": true,
            "command_ok": command_ok,
            "stdout": stdout.content,
            "stderr": stderr.content,
            "stdout_truncated": stdout.truncated,
            "stderr_truncated": stderr.truncated,
            "stdout_dropped_bytes": stdout_dropped,
            "stderr_dropped_bytes": stderr_dropped,
            "stdout_total_bytes": stdout_total,
            "stderr_total_bytes": stderr_total,
            "elapsed_ms": self.started_at.elapsed().as_millis(),
            "output_refs": {
                "stdout": format!("session:{}:stdout", self.session_id),
                "stderr": format!("session:{}:stderr", self.session_id)
            }
        })
    }
}

fn trim_buffer(buf: &mut Vec<u8>, limit: usize) -> usize {
    if buf.len() > limit {
        let drop = buf.len() - limit;
        buf.drain(..drop);
        drop
    } else {
        0
    }
}

struct Truncated {
    content: String,
    truncated: bool,
}

fn truncate_head_tail(
    head: &[u8],
    tail: &[u8],
    total_bytes: usize,
    dropped_bytes: usize,
    max_bytes: usize,
) -> Truncated {
    if total_bytes <= max_bytes && dropped_bytes == 0 {
        return Truncated {
            content: String::from_utf8_lossy(tail).into_owned(),
            truncated: false,
        };
    }

    if max_bytes == 0 {
        return Truncated {
            content: String::new(),
            truncated: total_bytes > 0,
        };
    }

    let head_budget = (max_bytes / SESSION_HEAD_DIVISOR).max(1).min(head.len());
    let remaining_budget = max_bytes.saturating_sub(head_budget);
    let tail_take = remaining_budget.min(tail.len());

    if head_budget == 0 || head.is_empty() {
        let take = tail.len().min(max_bytes);
        return Truncated {
            content: String::from_utf8_lossy(&tail[tail.len().saturating_sub(take)..]).into_owned(),
            truncated: true,
        };
    }

    let head_part = String::from_utf8_lossy(&head[..head_budget]);
    let tail_start = tail.len().saturating_sub(tail_take);
    let tail_part = String::from_utf8_lossy(&tail[tail_start..]);

    let omitted_bytes = total_bytes.saturating_sub(head_budget + tail_take);
    let content = if omitted_bytes > 0 {
        format!("{head_part}\n[... omitted {omitted_bytes} bytes ...]\n{tail_part}")
    } else {
        format!("{head_part}{tail_part}")
    };

    Truncated {
        content,
        truncated: true,
    }
}

pub fn read_output(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    let output_ref = args
        .get("output_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("output_ref is required"))?;
    let parts: Vec<&str> = output_ref.split(':').collect();
    if parts.len() != 3 || parts[0] != "session" {
        return Err(WorkspaceError::invalid_argument(
            "output_ref must look like session:<id>:stdout, session:<id>:stderr, or session:<id>:full",
        ));
    }
    let session_id = parts[1];
    let ref_stream = parts[2];
    if ref_stream != "stdout" && ref_stream != "stderr" && ref_stream != "full" {
        return Err(WorkspaceError::invalid_argument(
            "output_ref stream must be stdout, stderr, or full",
        ));
    }
    let session = store.get(session_id)?;
    tauri::async_runtime::block_on(session.refresh_status());

    let requested_stream = args.get("stream").and_then(Value::as_str).unwrap_or("");
    let stream = if ref_stream == "stdout" || ref_stream == "stderr" {
        ref_stream
    } else if requested_stream == "stdout" || requested_stream == "stderr" {
        requested_stream
    } else {
        "stdout"
    };

    let (data, total_stream_bytes) = session.retained_stream_bytes(stream);
    let requested_offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(4096)
        .clamp(1, 1_048_576) as usize;
    let buffer_offset = requested_offset.min(data.len());
    let chunk = &data[buffer_offset..data.len().min(buffer_offset + limit)];
    let next_offset = if buffer_offset + chunk.len() < total_stream_bytes {
        Some((buffer_offset + chunk.len()) as u64)
    } else {
        None
    };
    let has_more = next_offset.is_some();
    let mut payload = json!({
        "output_ref": output_ref,
        "session_id": session_id,
        "termination_reason": session.snapshot(1)["termination_reason"],
        "operation_outcome": session.snapshot(1)["operation_outcome"],
        "exit_code": session.snapshot(1)["exit_code"],
        "workspace_may_have_changed": session.snapshot(1)["workspace_may_have_changed"],
        "stream_output_ref": format!("session:{session_id}:{stream}"),
        "stream": stream,
        "offset": buffer_offset,
        "requested_offset": requested_offset,
        "limit": limit,
        "content": String::from_utf8_lossy(chunk),
        "next_offset": next_offset,
        "total_retained_bytes": data.len(),
        "total_stream_bytes": total_stream_bytes,
        "truncated": has_more,
        "has_more": has_more,
        "warnings": if ref_stream == "full" {
            vec!["legacy full output_ref defaults to stdout; use output_refs for stable stream paging"]
        } else {
            Vec::<&str>::new()
        }
    });
    if let Some(next) = next_offset {
        payload["continuation"] = json!({
            "tool": "read_output",
            "arguments": {
                "output_ref": format!("session:{session_id}:{stream}"),
                "offset": next
            }
        });
    }

    Ok(tool_ok(payload))
}

pub fn write_stdin(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("session_id is required"))?;
    let session = store.get(session_id)?;
    let chars = args.get("chars").and_then(Value::as_str).unwrap_or("");
    let max_output_bytes = args
        .get("max_output_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(65_536) as usize;

    let running = tauri::async_runtime::block_on(session.is_running());
    if !running {
        if !chars.is_empty() {
            return Err(WorkspaceError::Tool {
                code: "SESSION_CLOSED",
                message: "Session is closed; stdin write blocked.".into(),
                category: "runtime",
                retryable: false,
            });
        }
        return Ok(tool_ok(session.snapshot(max_output_bytes)));
    }

    if !chars.is_empty() {
        let mut stdin_guard = tauri::async_runtime::block_on(session.stdin.lock());
        let stdin = stdin_guard.as_mut().ok_or_else(|| WorkspaceError::Tool {
            code: "SESSION_CLOSED",
            message: "Session stdin is closed.".into(),
            category: "runtime",
            retryable: false,
        })?;
        use tokio::io::AsyncWriteExt;
        tauri::async_runtime::block_on(async {
            stdin
                .write_all(chars.as_bytes())
                .await
                .map_err(|_| WorkspaceError::Tool {
                    code: "SESSION_CLOSED",
                    message: "Session stdin is closed.".into(),
                    category: "runtime",
                    retryable: false,
                })
        })?;
        let _ = tauri::async_runtime::block_on(stdin.flush());
    }

    let yield_ms = args
        .get("yield_time_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000)
        .min(30_000);
    std::thread::sleep(std::time::Duration::from_millis(yield_ms));
    tauri::async_runtime::block_on(session.refresh_status());
    Ok(tool_ok(session.snapshot(max_output_bytes)))
}

pub fn kill_session(store: &SessionStore, args: &Value) -> Result<Value, WorkspaceError> {
    let session_id = args
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkspaceError::invalid_argument("session_id is required"))?;
    let session = store.get(session_id)?;
    let max_output_bytes = args
        .get("max_output_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(65_536) as usize;
    let wait_ms = args
        .get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5000)
        .min(30_000);
    let signal = args.get("signal").and_then(Value::as_str).unwrap_or("TERM");

    let running = tauri::async_runtime::block_on(session.is_running());
    let mut killed = false;
    let mut status = "exited";
    let mut evicted = true;

    if running {
        session.mark_termination_reason("killed");
        tauri::async_runtime::block_on(async {
            let pid = {
                let child = session.child.lock().await;
                child.id()
            };
            if let Some(pid) = pid {
                send_session_signal(pid, signal);
            } else {
                let mut child = session.child.lock().await;
                let _ = child.start_kill();
            }
            let _ = tokio::time::timeout(std::time::Duration::from_millis(wait_ms), async {
                let mut child = session.child.lock().await;
                let _ = child.wait().await;
            })
            .await;
        });
        tauri::async_runtime::block_on(session.refresh_status());
        if tauri::async_runtime::block_on(session.is_running()) {
            status = "terminating";
            evicted = false;
        } else {
            killed = true;
            status = "killed";
        }
    }

    let mut payload = session.snapshot(max_output_bytes);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("killed".into(), json!(killed));
        obj.insert("status".into(), json!(status));
        obj.insert("evicted".into(), json!(evicted));
        if status == "terminating" {
            obj.insert(
                "warnings".into(),
                json!(["Process did not exit after kill; session retained for retry"]),
            );
        }
    }

    if evicted {
        // Completed output remains available through the retention window.
    }

    Ok(tool_ok(payload))
}

#[cfg(unix)]
fn send_session_signal(pid: u32, signal: &str) {
    let sig = match signal {
        "KILL" => libc::SIGKILL,
        "INT" => libc::SIGINT,
        _ => libc::SIGTERM,
    };
    unsafe {
        libc::kill(pid as i32, sig);
    }
}

#[cfg(windows)]
fn send_session_signal(pid: u32, _signal: &str) {
    let _ = crate::platform::platform().terminate_process_tree(pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_not_truncated() {
        let head = b"hello";
        let tail = b"hello";
        let res = truncate_head_tail(head, tail, 5, 0, 100);
        assert!(!res.truncated);
        assert_eq!(res.content, "hello");
    }

    #[test]
    fn large_output_preserves_head_and_tail() {
        let head = b"HEAD_START_1234567890";
        let tail = b"TAIL_END_0987654321";
        let total = 1000;
        let dropped = 900;
        let res = truncate_head_tail(head, tail, total, dropped, 20);
        assert!(res.truncated);
        assert!(res.content.starts_with("HE"));
        assert!(res.content.ends_with("0987654321") || res.content.contains("TAIL"));
        assert!(res.content.contains("[... omitted "));
    }

    #[test]
    fn trim_buffer_tracks_dropped_bytes() {
        let mut buf = vec![0u8; 150];
        let dropped = trim_buffer(&mut buf, 100);
        assert_eq!(dropped, 50);
        assert_eq!(buf.len(), 100);
    }

    fn spawn_dummy_child() -> tokio::process::Child {
        #[cfg(windows)]
        let mut cmd = tokio::process::Command::new("cmd");
        #[cfg(windows)]
        cmd.args(["/c", "exit 0"]);

        #[cfg(not(windows))]
        let mut cmd = tokio::process::Command::new("sh");
        #[cfg(not(windows))]
        cmd.args(["-c", "exit 0"]);

        cmd.spawn().expect("dummy child")
    }

    #[tokio::test]
    async fn single_stream_retained_bytes_within_budget() {
        let dummy = spawn_dummy_child();
        let session = ExecSession::new(dummy);

        // 模拟写入 3MB 的 stdout 数据流
        let chunk = vec![b'A'; 4096];
        for _ in 0..768 {
            session.read_stream(&chunk[..], true).await;
        }

        let head_len = session.stdout_head.lock().unwrap().len();
        let tail_len = session.stdout.lock().unwrap().len();
        let total_retained = head_len + tail_len;

        assert_eq!(head_len, SESSION_HEAD_BUFFER_BYTES);
        assert_eq!(tail_len, SESSION_TAIL_BUFFER_BYTES);
        assert_eq!(total_retained, SESSION_STREAM_TOTAL_BUDGET_BYTES);
        assert!(total_retained <= SESSION_STREAM_TOTAL_BUDGET_BYTES);
    }

    #[tokio::test]
    async fn dual_streams_within_total_budget() {
        let dummy = spawn_dummy_child();
        let session = ExecSession::new(dummy);

        let chunk = vec![b'B'; 4096];
        for _ in 0..768 {
            session.read_stream(&chunk[..], true).await;
            session.read_stream(&chunk[..], false).await;
        }

        let stdout_retained =
            session.stdout_head.lock().unwrap().len() + session.stdout.lock().unwrap().len();
        let stderr_retained =
            session.stderr_head.lock().unwrap().len() + session.stderr.lock().unwrap().len();
        let total_process_retained = stdout_retained + stderr_retained;

        assert_eq!(stdout_retained, SESSION_STREAM_TOTAL_BUDGET_BYTES);
        assert_eq!(stderr_retained, SESSION_STREAM_TOTAL_BUDGET_BYTES);
        assert_eq!(
            total_process_retained,
            SESSION_STREAM_TOTAL_BUDGET_BYTES * 2
        );
    }
}
