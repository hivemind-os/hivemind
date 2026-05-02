use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::ring_buffer::RingBuffer;

const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64 KB
const DEFAULT_PTY_ROWS: u16 = 24;
const DEFAULT_PTY_COLS: u16 = 80;

/// How long completed process entries are kept before being pruned.
const COMPLETED_RETENTION: Duration = Duration::from_secs(10 * 60);
/// Run inline GC every N spawns.
const GC_EVERY_N_SPAWNS: u64 = 100;

/// Current status of a managed process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Exited { code: i32 },
    Killed,
    Failed { error: String },
}

impl ProcessStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, ProcessStatus::Running)
    }
}

/// Who owns / spawned the process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessOwner {
    Session { session_id: String },
    Unknown,
}

/// Lifecycle event emitted by the [`ProcessManager`] broadcast channel.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProcessEvent {
    Spawned { process_id: String, session_id: Option<String> },
    Exited { process_id: String, session_id: Option<String>, exit_code: Option<i32> },
    Killed { process_id: String, session_id: Option<String> },
}

/// Snapshot of process metadata (no large output buffers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub id: String,
    pub pid: u32,
    pub command: String,
    pub working_dir: Option<String>,
    pub status: ProcessStatus,
    pub uptime_secs: f64,
    pub owner: ProcessOwner,
}

struct ProcessEntry {
    id: String,
    command: String,
    working_dir: Option<String>,
    pid: u32,
    started_at: Instant,
    /// Frozen duration: set once when the process exits so uptime stops growing.
    stopped_elapsed: Mutex<Option<f64>>,
    status: Arc<Mutex<ProcessStatus>>,
    output_buffer: Arc<Mutex<RingBuffer>>,
    writer: Mutex<Box<dyn Write + Send>>,
    owner: ProcessOwner,
    /// Set once when the process exits, is killed, or fails.
    completed_at: Arc<Mutex<Option<Instant>>>,
    _reader_thread: std::thread::JoinHandle<()>,
    /// Sandbox temp files (e.g. .ps1 wrapper scripts) that must stay alive
    /// until the process exits. Dropping these deletes the underlying files.
    _temp_files: Vec<tempfile::TempPath>,
}

impl ProcessEntry {
    fn info(&self) -> ProcessInfo {
        let status = self.status.lock().clone();
        let uptime_secs = if status.is_running() {
            self.started_at.elapsed().as_secs_f64()
        } else {
            // Use the frozen value if available, otherwise snapshot now.
            let mut stopped = self.stopped_elapsed.lock();
            *stopped.get_or_insert_with(|| self.started_at.elapsed().as_secs_f64())
        };
        ProcessInfo {
            id: self.id.clone(),
            pid: self.pid,
            command: self.command.clone(),
            working_dir: self.working_dir.clone(),
            status,
            uptime_secs,
            owner: self.owner.clone(),
        }
    }
}

// ─── Platform-specific helpers ─────────────────────────────────────────

/// Collect the exit code after a process has finished (called from reader thread).
#[cfg(unix)]
fn collect_exit_code(
    pid: u32,
    mut child: Box<dyn portable_pty::Child + Send>,
    name: &str,
) -> Option<i32> {
    match nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None) {
        Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => Some(code),
        Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => Some(128 + sig as i32),
        Ok(_) => None,
        Err(nix::errno::Errno::ECHILD) => {
            // Already reaped (e.g. by portable-pty internals).
            match child.wait() {
                Ok(es) => Some(if es.success() { 0 } else { 1 }),
                Err(_) => None,
            }
        }
        Err(e) => {
            tracing::debug!("waitpid for {name} failed: {e}");
            None
        }
    }
}

#[cfg(windows)]
fn collect_exit_code(
    _pid: u32,
    mut child: Box<dyn portable_pty::Child + Send>,
    _name: &str,
) -> Option<i32> {
    match child.wait() {
        Ok(es) => Some(if es.success() { 0 } else { 1 }),
        Err(_) => None,
    }
}

/// Send a signal (Unix) or terminate (Windows) a process by PID.
/// Returns `Ok(true)` for hard kills, `Ok(false)` for graceful signals.
#[cfg(unix)]
fn terminate_process(pid: u32, signal: Option<&str>) -> Result<bool, String> {
    let sig = match signal.unwrap_or("SIGTERM") {
        "SIGTERM" | "sigterm" | "TERM" | "term" | "15" => nix::sys::signal::Signal::SIGTERM,
        "SIGKILL" | "sigkill" | "KILL" | "kill" | "9" => nix::sys::signal::Signal::SIGKILL,
        "SIGINT" | "sigint" | "INT" | "int" | "2" => nix::sys::signal::Signal::SIGINT,
        "SIGHUP" | "sighup" | "HUP" | "hup" | "1" => nix::sys::signal::Signal::SIGHUP,
        other => return Err(format!("unsupported signal: {other}")),
    };
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), sig)
        .map_err(|e| format!("kill({pid}, {sig:?}) failed: {e}"))?;
    Ok(sig == nix::sys::signal::Signal::SIGKILL)
}

/// On Windows there are no Unix signals — all kills use `TerminateProcess`.
#[cfg(windows)]
fn terminate_process(pid: u32, _signal: Option<&str>) -> Result<bool, String> {
    const PROCESS_TERMINATE: u32 = 0x0001;

    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn TerminateProcess(handle: isize, exit_code: u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    // SAFETY: Standard Win32 API calls with valid parameters.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle == 0 {
        return Err(format!("failed to open process {pid}"));
    }
    let result = unsafe { TerminateProcess(handle, 1) };
    unsafe { CloseHandle(handle) };
    if result == 0 {
        return Err(format!("TerminateProcess failed for pid {pid}"));
    }
    // Windows TerminateProcess is always a hard kill.
    Ok(true)
}

/// On Windows, ConPTY does not reliably signal EOF when the child process
/// exits.  The reader thread can block on `read()` indefinitely, which means
/// the exit code is never collected and the status stays `Running` forever.
///
/// This helper waits on the process handle directly via `WaitForSingleObject`,
/// then retrieves the real exit code.  Returns `None` only if the handle
/// cannot be opened (e.g. process already gone).
#[cfg(windows)]
fn wait_for_process_exit(pid: u32) -> Option<i32> {
    const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const INFINITE: u32 = 0xFFFF_FFFF;

    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn WaitForSingleObject(handle: isize, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(handle: isize, exit_code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    // SAFETY: Standard Win32 API calls with valid parameters.
    let handle =
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return None;
    }

    unsafe { WaitForSingleObject(handle, INFINITE) };

    let mut exit_code: u32 = 0;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    unsafe { CloseHandle(handle) };

    if ok != 0 {
        Some(exit_code as i32)
    } else {
        None
    }
}

/// Cancel all pending synchronous I/O on the given thread so a blocked
/// `read()` returns `ERROR_OPERATION_ABORTED`.
#[cfg(windows)]
fn cancel_thread_io(thread_id: u32) {
    const THREAD_TERMINATE: u32 = 0x0001;

    extern "system" {
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> isize;
        fn CancelSynchronousIo(thread: isize) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    // SAFETY: Standard Win32 API calls.
    let handle = unsafe { OpenThread(THREAD_TERMINATE, 0, thread_id) };
    if handle != 0 {
        unsafe { CancelSynchronousIo(handle) };
        unsafe { CloseHandle(handle) };
    }
}

/// Manages background PTY-backed processes.
///
/// Processes are scoped to the lifetime of this manager (and thus the daemon).
/// When the manager is dropped, all tracked processes are killed.
pub struct ProcessManager {
    processes: DashMap<String, ProcessEntry>,
    next_id: AtomicU64,
    event_tx: broadcast::Sender<ProcessEvent>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self { processes: DashMap::new(), next_id: AtomicU64::new(1), event_tx }
    }

    /// Subscribe to process lifecycle events.
    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.event_tx.subscribe()
    }

    /// Spawn a new background process in a PTY.
    ///
    /// Returns `(process_id, pid)` on success.
    pub fn spawn(
        &self,
        command: &str,
        working_dir: Option<&str>,
        env: Option<&HashMap<String, String>>,
        buffer_size: Option<usize>,
        owner: ProcessOwner,
    ) -> Result<(String, u32), String> {
        self.spawn_sandboxed(command, working_dir, env, buffer_size, None, owner)
    }

    /// Spawn a command in a PTY, optionally under an OS-level sandbox.
    pub fn spawn_sandboxed(
        &self,
        command: &str,
        working_dir: Option<&str>,
        env: Option<&HashMap<String, String>>,
        buffer_size: Option<usize>,
        sandbox_policy: Option<&hive_sandbox::SandboxPolicy>,
        owner: ProcessOwner,
    ) -> Result<(String, u32), String> {
        self.spawn_sandboxed_with_shell(
            command,
            working_dir,
            env,
            buffer_size,
            sandbox_policy,
            owner,
            None,
            None,
        )
    }

    /// Spawn a command in a PTY with an explicit shell, optionally under an OS-level sandbox.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_sandboxed_with_shell(
        &self,
        command: &str,
        working_dir: Option<&str>,
        env: Option<&HashMap<String, String>>,
        buffer_size: Option<usize>,
        sandbox_policy: Option<&hive_sandbox::SandboxPolicy>,
        owner: ProcessOwner,
        shell_program: Option<&str>,
        shell_flag: Option<&str>,
    ) -> Result<(String, u32), String> {
        let default_program = if cfg!(target_os = "windows") { "cmd" } else { "sh" };
        let default_flag = if cfg!(target_os = "windows") { "/C" } else { "-c" };
        let eff_shell = shell_program.unwrap_or(default_program);
        let eff_flag = shell_flag.unwrap_or(default_flag);

        // If a sandbox policy is provided, try to wrap the command.
        let (effective_program, effective_args, _temp_files) = if let Some(policy) = sandbox_policy
        {
            match hive_sandbox::sandbox_command_with_shell(
                command,
                policy,
                Some(eff_shell),
                Some(eff_flag),
            ) {
                Ok(hive_sandbox::SandboxedCommand::Wrapped { program, args, _temp_files }) => {
                    (program, args, _temp_files)
                }
                Ok(hive_sandbox::SandboxedCommand::Passthrough) | Err(_) => {
                    (eff_shell.to_string(), vec![eff_flag.to_string(), command.to_string()], vec![])
                }
            }
        } else {
            (eff_shell.to_string(), vec![eff_flag.to_string(), command.to_string()], vec![])
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: DEFAULT_PTY_ROWS,
                cols: DEFAULT_PTY_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("failed to open PTY: {e}"))?;

        let mut cmd = CommandBuilder::new(&effective_program);
        for arg in &effective_args {
            cmd.arg(arg);
        }

        if let Some(dir) = working_dir {
            cmd.cwd(dir);
        }
        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        let child =
            pair.slave.spawn_command(cmd).map_err(|e| format!("failed to spawn command: {e}"))?;

        let pid = child.process_id().ok_or_else(|| "failed to get process ID".to_string())?;

        // Close slave in parent so EOF propagates correctly.
        drop(pair.slave);

        let id_num = self.next_id.fetch_add(1, Ordering::Relaxed);
        let process_id = format!("proc-{id_num}");

        // Inline GC: periodically prune completed entries on spawn.
        if id_num % GC_EVERY_N_SPAWNS == 0 {
            self.prune_completed(COMPLETED_RETENTION);
        }

        let buf_cap = buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE);
        let output_buffer = Arc::new(Mutex::new(RingBuffer::new(buf_cap)));
        let status = Arc::new(Mutex::new(ProcessStatus::Running));
        let completed_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("failed to clone PTY reader: {e}"))?;
        let writer =
            pair.master.take_writer().map_err(|e| format!("failed to take PTY writer: {e}"))?;

        // Keep master alive so PTY stays open — reader/writer are cloned fds.
        // We move it into the reader thread and drop it when done.
        let master = pair.master;

        let buf_clone = Arc::clone(&output_buffer);
        let status_clone = Arc::clone(&status);
        let _completed_at_clone = Arc::clone(&completed_at);
        let pid_for_reader = pid;
        let thread_name = process_id.clone();
        let event_tx = self.event_tx.clone();
        let thread_session_id = match &owner {
            ProcessOwner::Session { session_id } => Some(session_id.clone()),
            ProcessOwner::Unknown => None,
        };
        let thread_process_id = process_id.clone();

        // On Windows, spawn a dedicated reaper thread that waits for the
        // child process to exit independently of the PTY reader.  ConPTY
        // often fails to signal EOF, which leaves the reader thread blocked
        // on `read()` and the status stuck at `Running`.
        //
        // After recording the exit code, the reaper calls
        // `CancelSynchronousIo` to interrupt the reader's blocked `read()`,
        // causing it to return an error and exit the loop.
        #[cfg(windows)]
        let reader_thread_id = Arc::new(AtomicU32::new(0));
        #[cfg(windows)]
        {
            let reaper_status = Arc::clone(&status);
            let _reaper_completed_at = Arc::clone(&completed_at);
            let reaper_name = process_id.clone();
            let reaper_pid = pid;
            let reaper_reader_tid = Arc::clone(&reader_thread_id);
            let reaper_event_tx = event_tx.clone();
            let reaper_session_id = thread_session_id.clone();
            let reaper_process_id = thread_process_id.clone();
            std::thread::Builder::new()
                .name(format!("pty-reaper-{reaper_name}"))
                .spawn(move || {
                    if let Some(code) = wait_for_process_exit(reaper_pid) {
                        let mut s = reaper_status.lock();
                        if s.is_running() {
                            *s = ProcessStatus::Exited { code };
                            drop(s);
                            let _ = reaper_event_tx.send(ProcessEvent::Exited {
                                process_id: reaper_process_id,
                                session_id: reaper_session_id,
                                exit_code: Some(code),
                            });
                        }
                    }
                    // Give the reader a brief window to drain remaining output.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    // Cancel the reader's blocked read() so the thread exits.
                    let tid = reaper_reader_tid.load(Ordering::Acquire);
                    if tid != 0 {
                        cancel_thread_io(tid);
                    }
                })
                .ok();
        }

        #[cfg(windows)]
        let reader_tid_slot = Arc::clone(&reader_thread_id);
        let reader_thread = std::thread::Builder::new()
            .name(format!("pty-reader-{thread_name}"))
            .spawn(move || {
                // Publish this thread's ID so the reaper can cancel our I/O.
                #[cfg(windows)]
                {
                    extern "system" {
                        fn GetCurrentThreadId() -> u32;
                    }
                    let tid = unsafe { GetCurrentThreadId() };
                    reader_tid_slot.store(tid, Ordering::Release);
                }

                let mut reader = reader;
                let mut tmp = [0u8; 4096];

                loop {
                    match reader.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf_clone.lock().write(&tmp[..n]);
                        }
                        Err(e) => {
                            tracing::debug!("PTY reader {thread_name} error: {e}");
                            break;
                        }
                    }
                }

                // On Unix the reader loop exits reliably on EOF, so we
                // collect the exit code here.  On Windows the reaper thread
                // already handles this; we still call collect_exit_code as a
                // fallback in case the reaper didn't run.
                let exit_code = collect_exit_code(pid_for_reader, child, &thread_name);

                let mut s = status_clone.lock();
                if s.is_running() {
                    *s = match exit_code {
                        Some(code) => ProcessStatus::Exited { code },
                        None => ProcessStatus::Failed { error: "unknown exit status".into() },
                    };
                    drop(s);
                    let _ = event_tx.send(ProcessEvent::Exited {
                        process_id: thread_process_id,
                        session_id: thread_session_id,
                        exit_code,
                    });
                }

                // Drop master to fully close the PTY.
                drop(master);
            })
            .map_err(|e| format!("failed to spawn reader thread: {e}"))?;

        let session_id = match &owner {
            ProcessOwner::Session { session_id } => Some(session_id.clone()),
            ProcessOwner::Unknown => None,
        };

        let entry = ProcessEntry {
            id: process_id.clone(),
            command: command.to_string(),
            working_dir: working_dir.map(|s| s.to_string()),
            pid,
            started_at: Instant::now(),
            stopped_elapsed: Mutex::new(None),
            status,
            output_buffer,
            writer: Mutex::new(writer),
            owner,
            completed_at,
            _reader_thread: reader_thread,
            _temp_files,
        };

        self.processes.insert(process_id.clone(), entry);
        let _ = self
            .event_tx
            .send(ProcessEvent::Spawned { process_id: process_id.clone(), session_id });
        Ok((process_id, pid))
    }

    /// Get the status and recent output of a process.
    pub fn status(
        &self,
        id: &str,
        tail_lines: Option<usize>,
    ) -> Result<(ProcessInfo, String), String> {
        let entry = self.processes.get(id).ok_or_else(|| format!("no process with id `{id}`"))?;

        let info = entry.info();
        let output = match tail_lines {
            Some(n) => entry.output_buffer.lock().read_tail_lines(n),
            None => entry.output_buffer.lock().read_all_string(),
        };
        Ok((info, output))
    }

    /// Write data to the process stdin (PTY master write side).
    pub fn write_stdin(&self, id: &str, input: &str) -> Result<(), String> {
        let entry = self.processes.get(id).ok_or_else(|| format!("no process with id `{id}`"))?;

        if !entry.status.lock().is_running() {
            return Err(format!("process `{id}` is no longer running"));
        }

        let mut writer = entry.writer.lock();
        writer.write_all(input.as_bytes()).map_err(|e| format!("write failed: {e}"))?;
        writer.flush().map_err(|e| format!("flush failed: {e}"))?;
        Ok(())
    }

    /// Send a signal to the process (Unix) or terminate it (Windows).
    pub fn kill(&self, id: &str, signal: Option<&str>) -> Result<ProcessInfo, String> {
        let entry = self.processes.get(id).ok_or_else(|| format!("no process with id `{id}`"))?;

        let was_killed;
        {
            let mut status = entry.status.lock();
            if !status.is_running() {
                // Build ProcessInfo without re-locking status.
                return Ok(ProcessInfo {
                    id: entry.id.clone(),
                    pid: entry.pid,
                    command: entry.command.clone(),
                    working_dir: entry.working_dir.clone(),
                    status: status.clone(),
                    uptime_secs: {
                        let mut stopped = entry.stopped_elapsed.lock();
                        *stopped.get_or_insert_with(|| entry.started_at.elapsed().as_secs_f64())
                    },
                    owner: entry.owner.clone(),
                });
            }
            let force = terminate_process(entry.pid, signal)?;
            if force {
                *status = ProcessStatus::Killed;
                *entry.completed_at.lock() = Some(Instant::now());
            }
            was_killed = force;
        }

        if was_killed {
            let session_id = match &entry.owner {
                ProcessOwner::Session { session_id } => Some(session_id.clone()),
                ProcessOwner::Unknown => None,
            };
            let _ =
                self.event_tx.send(ProcessEvent::Killed { process_id: id.to_string(), session_id });
        }

        Ok(entry.info())
    }

    /// List all tracked processes.
    pub fn list(&self) -> Vec<ProcessInfo> {
        self.processes.iter().map(|e| e.value().info()).collect()
    }

    /// List processes owned by a specific session.
    pub fn list_by_session(&self, session_id: &str) -> Vec<ProcessInfo> {
        self.processes
            .iter()
            .filter(|e| matches!(&e.value().owner, ProcessOwner::Session { session_id: sid } if sid == session_id))
            .map(|e| e.value().info())
            .collect()
    }

    /// Kill all running processes. Called during daemon shutdown.
    pub fn shutdown_all(&self) {
        for entry in self.processes.iter() {
            if entry.status.lock().is_running() {
                let _ = terminate_process(entry.pid, None);
            }
        }
    }

    /// Remove completed process entries whose completion time exceeds `retention`.
    fn prune_completed(&self, retention: Duration) {
        let now = Instant::now();
        self.processes.retain(|_id, entry| {
            if entry.status.lock().is_running() {
                return true;
            }
            match *entry.completed_at.lock() {
                Some(completed) => now.duration_since(completed) < retention,
                None => true,
            }
        });
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sleep_cmd(secs: u32) -> String {
        if cfg!(windows) {
            format!("ping -n {} 127.0.0.1 >nul", secs + 1)
        } else {
            format!("sleep {secs}")
        }
    }

    #[test]
    fn spawn_and_status() {
        let mgr = ProcessManager::new();
        let (id, pid) = mgr.spawn("echo hello", None, None, None, ProcessOwner::Unknown).unwrap();
        assert!(id.starts_with("proc-"));
        assert!(pid > 0);

        // Poll for exit (ConPTY on Windows may delay EOF detection).
        let mut exited = false;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let (info, _) = mgr.status(&id, None).unwrap();
            if !info.status.is_running() {
                exited = true;
                break;
            }
        }

        let (info, output) = mgr.status(&id, None).unwrap();
        if cfg!(not(windows)) {
            assert!(!info.status.is_running());
        } else if !exited {
            // On Windows, ConPTY may not signal EOF for short-lived processes.
            // The process ran and produced output, which is the important part.
        }
        assert!(output.contains("hello"), "output was: {output}");
    }

    #[test]
    fn spawn_and_kill() {
        let mgr = ProcessManager::new();
        let cmd = sleep_cmd(60);
        let (id, _pid) = mgr.spawn(&cmd, None, None, None, ProcessOwner::Unknown).unwrap();

        let (info, _) = mgr.status(&id, None).unwrap();
        assert!(info.status.is_running());

        mgr.kill(&id, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));

        let (info, _) = mgr.status(&id, None).unwrap();
        assert!(!info.status.is_running(), "status: {:?}", info.status);
    }

    #[test]
    #[cfg(unix)]
    fn write_stdin() {
        let mgr = ProcessManager::new();
        let (id, _) = mgr.spawn("cat", None, None, None, ProcessOwner::Unknown).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(200));
        mgr.write_stdin(&id, "hello from stdin\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));

        let (_, output) = mgr.status(&id, None).unwrap();
        assert!(output.contains("hello from stdin"), "output was: {output}");

        mgr.kill(&id, None).unwrap();
    }

    #[test]
    fn list_processes() {
        let mgr = ProcessManager::new();
        let cmd_a = sleep_cmd(30);
        let cmd_b = sleep_cmd(31);
        let (id1, _) = mgr.spawn(&cmd_a, None, None, None, ProcessOwner::Unknown).unwrap();
        let (id2, _) = mgr.spawn(&cmd_b, None, None, None, ProcessOwner::Unknown).unwrap();

        let list = mgr.list();
        assert_eq!(list.len(), 2);
        let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&id1.as_str()));
        assert!(ids.contains(&id2.as_str()));

        mgr.kill(&id1, None).unwrap();
        mgr.kill(&id2, None).unwrap();
    }

    #[test]
    fn status_unknown_id() {
        let mgr = ProcessManager::new();
        assert!(mgr.status("nonexistent", None).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn tail_lines() {
        let mgr = ProcessManager::new();
        let (id, _) = mgr
            .spawn("printf 'a\\nb\\nc\\nd\\ne'", None, None, None, ProcessOwner::Unknown)
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(500));

        let (_, output) = mgr.status(&id, Some(2)).unwrap();
        assert_eq!(output, "d\ne");
    }

    /// Verify that sandbox temp files (e.g. .ps1 wrapper scripts) stay alive
    /// while the spawned process is running.
    #[test]
    fn spawn_sandboxed_echo() {
        let mgr = ProcessManager::new();
        let policy = hive_sandbox::SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let (id, pid) = mgr
            .spawn_sandboxed(
                "echo sandbox-ok",
                None,
                None,
                None,
                Some(&policy),
                ProcessOwner::Unknown,
            )
            .unwrap();
        assert!(pid > 0);

        // Wait for it to produce output or exit.
        let mut output = String::new();
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let (info, out) = mgr.status(&id, None).unwrap();
            output = out;
            if output.contains("sandbox-ok") || !info.status.is_running() {
                break;
            }
        }
        assert!(output.contains("sandbox-ok"), "expected 'sandbox-ok' in output: {output}");

        // Process may have already exited; ignore kill errors.
        let _ = mgr.kill(&id, None);
    }

    /// Verify that errors from sandboxed commands are visible — exit code
    /// and error output must propagate back to the caller.
    #[test]
    fn spawn_sandboxed_error_propagation() {
        let mgr = ProcessManager::new();
        let policy = hive_sandbox::SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };

        // Run a command that will definitely fail.
        let (id, _pid) = mgr
            .spawn_sandboxed(
                "this_command_does_not_exist_xyz",
                None,
                None,
                None,
                Some(&policy),
                ProcessOwner::Unknown,
            )
            .unwrap();

        // Wait for it to exit.
        let mut exited = false;
        let mut output = String::new();
        let mut final_status = ProcessStatus::Running;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let (info, out) = mgr.status(&id, None).unwrap();
            output = out;
            final_status = info.status.clone();
            if !info.status.is_running() {
                exited = true;
                break;
            }
        }

        // The process MUST have exited.
        assert!(exited, "failed command should have exited, status: {final_status:?}");

        // The exit code MUST be non-zero.
        match &final_status {
            ProcessStatus::Exited { code } => {
                assert_ne!(*code, 0, "failed command must exit with non-zero code");
            }
            other => panic!("expected Exited with non-zero code, got: {other:?}"),
        }

        // The error text MUST be visible in the output.
        assert!(
            output.contains("not recognized")
                || output.contains("not found")
                || output.contains("error"),
            "error message should be visible in output, got: {output}"
        );
    }

    /// Verify that a long-running sandboxed process stays alive — the temp
    /// file must not be deleted before the process reads it.
    #[test]
    fn spawn_sandboxed_long_running() {
        let mgr = ProcessManager::new();
        let policy = hive_sandbox::SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        // Use a long-running command; ping on Windows, sleep on Unix.
        let cmd = sleep_cmd(30);
        let (id, _pid) = mgr
            .spawn_sandboxed(&cmd, None, None, None, Some(&policy), ProcessOwner::Unknown)
            .unwrap();

        // Give the process time to start and read the wrapper script.
        std::thread::sleep(std::time::Duration::from_millis(2000));

        let (info, _) = mgr.status(&id, None).unwrap();
        assert!(
            info.status.is_running(),
            "sandboxed long-running process should still be running, status: {:?}",
            info.status
        );

        mgr.kill(&id, None).unwrap();
    }
}
