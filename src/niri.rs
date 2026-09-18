//! Explicit reload plus a fresh ConfigLoaded acknowledgement. The initial event
//! describes a previous load and must never be mistaken for our own result.
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::Read,
    os::fd::AsRawFd,
    path::Path,
    process::{Child, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};
struct Events {
    child: Child,
    out: ChildStdout,
    pending: Vec<u8>,
}
impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Events {
    fn connect() -> Result<Self> {
        let mut child = Command::new("niri")
            .args(["msg", "--json", "event-stream"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let out = child.stdout.take().context("No Niri event stream")?;
        let fd = out.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self {
            child,
            out,
            pending: vec![],
        })
    }
    fn config_loaded(&mut self) -> Result<bool> {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            while let Some(index) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.pending.drain(..=index).collect();
                let v: serde_json::Value = serde_json::from_slice(&line)?;
                if let Some(failed) = v
                    .get("ConfigLoaded")
                    .and_then(|e| e.get("failed"))
                    .and_then(|v| v.as_bool())
                {
                    return Ok(!failed);
                }
            }
            ensure!(
                Instant::now() < end,
                "Timed out waiting for a fresh Niri ConfigLoaded event"
            );
            let mut buf = [0u8; 4096];
            match self.out.read(&mut buf) {
                Ok(0) => bail!("Niri event stream closed"),
                Ok(n) => self.pending.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => return Err(e.into()),
            };
            ensure!(
                self.pending.len() < 2 * 1024 * 1024,
                "Niri event buffer exceeded bound"
            );
        }
    }
}
pub fn reload(path: &Path) -> Result<()> {
    let mut events = Events::connect()?;
    let _previous = events.config_loaded()?;
    // Drain already buffered events. A second ConfigLoaded cannot acknowledge a
    // request we have not sent yet.
    events.pending.clear();
    let end = Instant::now() + Duration::from_secs(1);
    loop {
        ensure!(
            Instant::now() < end,
            "Niri event stream did not settle before reload"
        );
        let mut discard = [0u8; 4096];
        match events.out.read(&mut discard) {
            Ok(0) => bail!("Niri event stream closed before reload"),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.into()),
        }
    }
    let mut cmd = Command::new("niri");
    cmd.args(["msg", "action", "load-config-file", "--path"])
        .arg(path);
    crate::control::bounded(cmd).context("Niri rejected the reload request")?;
    ensure!(
        events.config_loaded()?,
        "Niri reported a failed configuration load"
    );
    Ok(())
}
