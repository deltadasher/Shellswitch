use crate::model::{Backend, Candidate};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::MetadataExt,
    },
    path::PathBuf,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub start: u64,
    pub boot: String,
}
#[derive(Debug)]
pub struct Process {
    pub identity: Identity,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub group: u32,
    pub state: char,
}
pub fn inspect(pid: u32) -> Option<Process> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    if fs::metadata(&root).ok()?.uid() != unsafe { libc::geteuid() } {
        return None;
    }
    let stat = fs::read_to_string(root.join("stat")).ok()?;
    let parts: Vec<_> = stat
        .get(stat.rfind(')')? + 2..)?
        .split_whitespace()
        .collect();
    let start = parts.get(19)?.parse().ok()?;
    let argv = fs::read(root.join("cmdline"))
        .ok()?
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    Some(Process {
        identity: Identity {
            pid,
            start,
            boot: fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .ok()?
                .trim()
                .into(),
        },
        argv,
        cwd: fs::read_link(root.join("cwd")).ok()?,
        group: parts.get(2)?.parse().ok()?,
        state: parts.first()?.chars().next()?,
    })
}
pub fn all() -> Vec<Process> {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok().and_then(inspect))
        .collect()
}
pub fn alive(id: &Identity) -> bool {
    inspect(id.pid).is_some_and(|p| {
        p.identity.start == id.start
            && p.identity.boot == id.boot
            && p.state != 'Z'
            && p.state != 'X'
    })
}
fn same_path(a: &str, b: &str) -> bool {
    fs::canonicalize(a)
        .ok()
        .zip(fs::canonicalize(b).ok())
        .is_some_and(|(a, b)| a == b)
}
pub fn same_session(pid: u32) -> bool {
    let Ok(bytes) = fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let get = |key: &str| {
        bytes.split(|b| *b == 0).find_map(|v| {
            v.strip_prefix(format!("{key}=").as_bytes())
                .map(|v| String::from_utf8_lossy(v).into_owned())
        })
    };
    let key = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "WAYLAND_DISPLAY"
    } else {
        "DISPLAY"
    };
    if let Ok(current) = std::env::var(key)
        && get(key).as_deref() != Some(&current)
    {
        return false;
    }
    for key in ["XDG_RUNTIME_DIR", "XDG_SESSION_ID"] {
        if let (Ok(current), Some(other)) = (std::env::var(key), get(key))
            && current != other
        {
            return false;
        }
    }
    true
}
pub fn matches(c: &Candidate, p: &Process) -> bool {
    if !same_session(p.identity.pid) {
        return false;
    }
    let Backend::Process { argv, .. } = &c.backend else {
        return false;
    };
    if argv.is_empty() || p.argv.is_empty() {
        return false;
    }
    if c.framework == "Quickshell" {
        let exe = PathBuf::from(&p.argv[0]);
        if !exe
            .file_name()
            .is_some_and(|n| n == "qs" || n == "quickshell")
        {
            return false;
        }
        if p.argv
            .iter()
            .any(|a| ["ipc", "list", "kill", "log"].contains(&a.as_str()))
        {
            return false;
        }
        let selected = p.argv.windows(2).find_map(|a| {
            if a[0] == "-p" || a[0] == "--path" {
                let q = PathBuf::from(&a[1]);
                Some(if q.is_absolute() { q } else { p.cwd.join(q) })
            } else if a[0] == "-c" || a[0] == "--config" {
                Some(
                    crate::discovery::config_home()
                        .join("quickshell")
                        .join(&a[1]),
                )
            } else {
                None
            }
        });
        // Environment config overrides and deprecated manifests are not guessed.
        let env = fs::read(format!("/proc/{}/environ", p.identity.pid)).unwrap_or_default();
        if selected.is_none()
            && (env
                .split(|b| *b == 0)
                .any(|v| v.starts_with(b"QS_CONFIG_") || v.starts_with(b"QS_MANIFEST="))
                || p.argv.len() > 1)
        {
            return false;
        }
        let selected =
            selected.unwrap_or_else(|| crate::discovery::config_home().join("quickshell"));
        let selected = if selected.is_dir() {
            selected.join("shell.qml")
        } else {
            selected
        };
        return same_path(&selected.to_string_lossy(), &c.source.to_string_lossy());
    }
    argv.len() == p.argv.len()
        && (argv[0] == p.argv[0] || same_path(&argv[0], &p.argv[0]))
        && argv[1..] == p.argv[1..]
}
pub fn annotate(candidates: &mut [Candidate]) {
    let ps = all();
    for c in candidates {
        if !matches!(c.backend, Backend::Review { .. }) {
            c.running_pids = ps
                .iter()
                .filter(|p| matches(c, p))
                .map(|p| p.identity.pid)
                .collect();
        } else {
            c.running_pids
                .retain(|pid| ps.iter().any(|p| p.identity.pid == *pid));
        }
    }
}
fn pidfd(id: &Identity) -> Result<OwnedFd> {
    if !alive(id) {
        bail!("Process {} is gone or changed identity", id.pid);
    }
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, id.pid, 0) } as i32;
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if !alive(id) {
        bail!("Process identity changed while opening pidfd");
    }
    Ok(fd)
}
fn signal(fd: &OwnedFd, sig: i32) -> Result<()> {
    let r = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            sig,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if r < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(e.into());
        }
    }
    Ok(())
}
pub fn stop(id: &Identity, owned_group: bool) -> Result<()> {
    if !alive(id) {
        return Ok(());
    }
    let leader = pidfd(id)?;
    // Open stable handles before signalling; never signal names or a recycled PID.
    let children: Vec<_> = if owned_group {
        all()
            .iter()
            .filter(|p| p.group == id.pid && p.identity.pid != id.pid)
            .filter_map(|p| pidfd(&p.identity).ok())
            .collect()
    } else {
        vec![]
    };
    for fd in &children {
        signal(fd, libc::SIGTERM)?;
    }
    signal(&leader, libc::SIGTERM)?;
    for _ in 0..30 {
        if !alive(id) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for fd in &children {
        signal(fd, libc::SIGKILL)?;
    }
    signal(&leader, libc::SIGKILL)?;
    Ok(())
}
pub fn kill_exact(id: &Identity) -> Result<()> {
    if !alive(id) {
        return Ok(());
    }
    let fd = pidfd(id)?;
    signal(&fd, libc::SIGKILL)?;
    for _ in 0..100 {
        if !alive(id) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    anyhow::bail!("Owned process {} did not exit", id.pid)
}
pub fn adoptable(c: &Candidate, pid: u32) -> Result<Identity> {
    let p = inspect(pid).context("Process unavailable or not owned by this user")?;
    if !matches(c, &p) {
        bail!("PID does not match this candidate's exact entrypoint");
    }
    let cg = fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
    if cg
        .split('/')
        .any(|s| s.trim().ends_with(".service") && !s.starts_with("user@"))
    {
        bail!("Process belongs to a service; use a systemd manifest to avoid supervisor respawns");
    }
    Ok(p.identity)
}
static STOP_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn request_stop(_: i32) {
    STOP_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
}
/// A persistent owner for foreground processes. Keeps its group ID alive during
/// cleanup even if a launcher exits, so rollback cannot lose background children.
pub fn supervise(argv: &[String], ticket: Option<&std::path::Path>) -> Result<()> {
    use std::{process::Command, sync::atomic::Ordering, time::Duration};
    if argv.is_empty() {
        bail!("Empty supervised command");
    }
    let me = inspect(std::process::id()).context("Cannot inspect supervisor")?;
    if me.group != std::process::id() {
        bail!("Supervisor must run in its own process group");
    }
    unsafe {
        libc::signal(
            libc::SIGTERM,
            request_stop as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            request_stop as *const () as libc::sighandler_t,
        );
    }
    if let Some(ticket) = ticket {
        crate::ownership::save_json(&ticket.with_extension("pid"), &me.identity)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !ticket.is_file() {
            if STOP_REQUESTED.load(Ordering::Relaxed) {
                return Ok(());
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "Launch journal acknowledgement timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        anyhow::ensure!(
            std::fs::read(ticket)? == b"go",
            "Invalid launch acknowledgement"
        );
    }
    let mut child = Command::new(&argv[0]).args(&argv[1..]).spawn()?;
    let result = loop {
        if STOP_REQUESTED.load(Ordering::Relaxed) {
            break Ok(());
        }
        if let Some(status) = child.try_wait()? {
            break if status.success() {
                Ok(())
            } else {
                Err(anyhow::anyhow!("Shell exited: {status}"))
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // All members still share our live group identity. Escaping setsid/double-fork
    // daemons are intentionally unsupported; declare their real service instead.
    let members: Vec<_> = all()
        .iter()
        .filter(|p| p.group == me.identity.pid && p.identity.pid != me.identity.pid)
        .filter_map(|p| pidfd(&p.identity).ok())
        .collect();
    for fd in &members {
        signal(fd, libc::SIGTERM)?;
    }
    std::thread::sleep(Duration::from_millis(100));
    for fd in &members {
        signal(fd, libc::SIGKILL)?;
    }
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_recycled_identity() {
        let mut id = inspect(std::process::id()).unwrap().identity;
        id.start += 1;
        assert!(!alive(&id));
        assert!(pidfd(&id).is_err());
    }
    #[test]
    fn reads_own_identity() {
        assert!(alive(&inspect(std::process::id()).unwrap().identity));
    }
}
