use std::{
    env,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process, thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sysinfo::{Pid, Process, Signal, System};

use crate::infrastructure::directories::ResolvedPaths;

const LOCK_FILENAME: &str = ".bot.lock";
const WAIT_INTERVAL: Duration = Duration::from_millis(500);
const MAX_WAIT: Duration = Duration::from_secs(20);
const UNIQUE_PROCESS_NAME: &str = "fg_spam_guard";

#[derive(Debug)]
pub struct InstanceGuard {
    file: File,
    path: PathBuf,
}

impl InstanceGuard {
    pub fn acquire(paths: &ResolvedPaths) -> Result<Self> {
        if skip_guard() {
            tracing::warn!(
                target: "lifecycle",
                "process guard skipped because SKIP_PROCESS_GUARD=1"
            );
        } else {
            set_process_name();
            terminate_conflicting_instances()?;
        }

        let lock_path = paths.data_dir.join(LOCK_FILENAME);
        fs::create_dir_all(&paths.data_dir)
            .with_context(|| format!("failed to ensure data dir {}", paths.data_dir.display()))?;

        let start = Instant::now();
        loop {
            let mut file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&lock_path)
                .with_context(|| format!("failed to open lock file {}", lock_path.display()))?;

            match file.try_lock_exclusive() {
                Ok(()) => {
                    write_lock_info(&mut file, process::id())?;
                    tracing::info!(
                        target: "lifecycle",
                        pid = process::id(),
                        path = %lock_path.display(),
                        "acquired bot runtime lock"
                    );
                    return Ok(Self {
                        file,
                        path: lock_path.clone(),
                    });
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    handle_existing_instance(&lock_path)?;
                }
                Err(err) => return Err(err.into()),
            }

            if start.elapsed() > MAX_WAIT {
                return Err(anyhow!(
                    "another instance is still shutting down; waited {:?}",
                    MAX_WAIT
                ));
            }

            drop(file);
            thread::sleep(WAIT_INTERVAL);
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        if let Err(err) = fs::remove_file(&self.path) {
            if err.kind() != ErrorKind::NotFound {
                tracing::warn!(
                    target: "lifecycle",
                    path = %self.path.display(),
                    error = %err,
                    "failed to remove lock file on shutdown"
                );
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    started_at: i64,
}

fn write_lock_info(file: &mut File, pid: u32) -> Result<()> {
    let info = LockInfo {
        pid,
        started_at: Utc::now().timestamp_millis(),
    };
    let payload = serde_json::to_vec(&info)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&payload)?;
    file.sync_all()?;
    Ok(())
}

fn handle_existing_instance(lock_path: &Path) -> Result<()> {
    if let Some(info) = read_lock_info(lock_path)? {
        if info.pid == process::id() {
            // Another thread in same process? treat as already running.
            return Err(anyhow!(
                "another bot instance appears to be running in this process (pid {})",
                info.pid
            ));
        }

        if terminate_process(info.pid)? {
            tracing::warn!(
                target: "lifecycle",
                pid = info.pid,
                "terminated previous bot instance to acquire lock"
            );
        } else if !is_process_alive(info.pid) {
            // stale lock: try removing file.
            let _ = fs::remove_file(lock_path);
        }
    } else {
        // no info, offer to remove stale file
        let _ = fs::remove_file(lock_path);
    }
    Ok(())
}

fn read_lock_info(lock_path: &Path) -> Result<Option<LockInfo>> {
    match fs::read_to_string(lock_path) {
        Ok(contents) => {
            if contents.trim().is_empty() {
                return Ok(None);
            }
            match serde_json::from_str(&contents) {
                Ok(info) => Ok(Some(info)),
                Err(err) => {
                    tracing::warn!(
                        target: "lifecycle",
                        path = %lock_path.display(),
                        error = %err,
                        "failed to parse lock file metadata"
                    );
                    Ok(None)
                }
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn terminate_process(pid: u32) -> Result<bool> {
    if !is_process_alive(pid) {
        return Ok(false);
    }

    let sys_pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_process(sys_pid);

    let mut killed = false;
    if let Some(process) = system.process(sys_pid) {
        if process.kill_with(Signal::Term).unwrap_or(false) {
            killed = true;
        }
    }

    thread::sleep(Duration::from_secs(1));
    system.refresh_process(sys_pid);

    if let Some(process) = system.process(sys_pid) {
        if process.kill() {
            killed = true;
        }
    }

    Ok(killed)
}

fn is_process_alive(pid: u32) -> bool {
    let sys_pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_process(sys_pid);
    system.process(sys_pid).is_some()
}

fn terminate_conflicting_instances() -> Result<()> {
    let mut system = System::new();
    system.refresh_processes();

    let current_pid = process::id();
    let current_exe = env::current_exe().ok();
    let mut killed_any = false;

    for (pid, proc_info) in system.processes() {
        let pid_u32 = pid.as_u32();
        if pid_u32 == current_pid {
            continue;
        }
        if matches_signature(proc_info, current_exe.as_deref()) {
            if terminate_process(pid_u32)? {
                killed_any = true;
                tracing::warn!(
                    target: "lifecycle",
                    pid = pid_u32,
                    name = proc_info.name(),
                    "terminated conflicting bot instance"
                );
            }
        }
    }

    if killed_any {
        thread::sleep(WAIT_INTERVAL);
    }

    Ok(())
}

fn matches_signature(process: &Process, current_exe: Option<&Path>) -> bool {
    if process.name() == UNIQUE_PROCESS_NAME {
        return true;
    }
    if let Some(exe) = current_exe {
        if let Some(proc_exe) = process.exe() {
            if proc_exe == exe {
                return true;
            }
        }
        if let Some(filename) = exe.file_name().and_then(|os| os.to_str()) {
            if process.name() == filename {
                return true;
            }
        }
    }
    false
}

fn set_process_name() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use libc::prctl;
        let mut name = [0u8; 16];
        let bytes = UNIQUE_PROCESS_NAME.as_bytes();
        let len = bytes.len().min(15);
        name[..len].copy_from_slice(&bytes[..len]);
        unsafe {
            prctl(libc::PR_SET_NAME, name.as_ptr() as i64, 0, 0, 0);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        if let Ok(c_str) = CString::new(UNIQUE_PROCESS_NAME) {
            unsafe {
                libc::pthread_setname_np(c_str.as_ptr());
            }
        }
    }
}

fn skip_guard() -> bool {
    matches!(
        env::var("SKIP_PROCESS_GUARD")
            .ok()
            .map(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true")),
        Some(true)
    )
}
