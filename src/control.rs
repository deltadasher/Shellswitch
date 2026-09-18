use crate::{
    lifecycle,
    ownership::{self, Change, Config, OwnedFile},
};
use crate::{
    model::*,
    process::{self, Identity},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Running {
    pub candidate: Candidate,
    pub process: Option<Identity>,
    pub owned_group: bool,
    #[serde(default)]
    pub lease: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Prepared,
    Protected,
    Stopped,
    ConfigApplied,
    Starting,
    Trial,
    Restoring,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub previous: Option<Running>,
    pub target: Candidate,
    pub deadline: u64,
    pub token: String,
    pub phase: Phase,
    pub previous_selected: Option<String>,
    pub previous_config: Option<Config>,
    pub next_config: Option<Config>,
    pub previous_protections: Vec<OwnedFile>,
    pub next_protections: Vec<OwnedFile>,
    pub changes: Vec<Change>,
    pub ticket: Option<PathBuf>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub schema: u32,
    pub active: Option<Running>,
    pub pending: Option<Pending>,
    pub last_event: String,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub installed: BTreeMap<String, Candidate>,
    #[serde(default)]
    pub disabled: BTreeSet<String>,
    #[serde(default)]
    pub config: Option<Config>,
    #[serde(default)]
    pub protections: Vec<OwnedFile>,
    #[serde(default)]
    pub commands: Vec<Running>,
    #[serde(default)]
    pub generation: u64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            schema: 2,
            active: None,
            pending: None,
            last_event: String::new(),
            selected: None,
            installed: BTreeMap::new(),
            disabled: BTreeSet::new(),
            config: None,
            protections: vec![],
            commands: vec![],
            generation: 0,
        }
    }
}
pub struct Store {
    pub dir: PathBuf,
    _lock: File,
}
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        });
    base.join("shellswitch")
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let meta = fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
            bail!("State directory must be owned by you and not a symlink");
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self {
            dir: dir.into(),
            _lock: lock,
        })
    }
    pub fn mutation_ready(&self) -> Result<()> {
        ensure!(
            !self.dir.join("files-pending.json").exists(),
            "Interrupted file operation; run recover --yes first"
        );
        Ok(())
    }
    pub fn load(&self) -> Result<State> {
        match fs::read(self.dir.join("state.json")) {
            Ok(data) => {
                let mut state:State=serde_json::from_slice(&data).context("Invalid or legacy pending journal; recover it with the previous version before migration")?;
                if state.schema < 2 {
                    state.schema = 2;
                    if let Some(r) = &state.active {
                        state.selected = Some(r.candidate.id.clone());
                        state
                            .installed
                            .insert(r.candidate.id.clone(), r.candidate.clone());
                    }
                }
                ensure!(state.schema == 2, "Unsupported state schema");
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, s: &State) -> Result<()> {
        let temp = self.dir.join("state.tmp");
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        f.write_all(&serde_json::to_vec_pretty(s)?)?;
        f.sync_all()?;
        fs::rename(temp, self.dir.join("state.json"))?;
        File::open(&self.dir)?.sync_all()?;
        Ok(())
    }
}
pub(crate) fn bounded(mut cmd: Command) -> Result<String> {
    // Small outputs only; use a temp file to avoid pipe-buffer deadlocks.
    let path = std::env::temp_dir().join(format!(
        "shellswitch-command-{}-{}",
        std::process::id(),
        now()
    ));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .read(true)
        .mode(0o600)
        .open(&path)?;
    fs::remove_file(&path)?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file.try_clone()?)
        .spawn()?;
    let end = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait()? {
            use std::io::{Read, Seek};
            let mut f = file;
            f.rewind()?;
            let mut s = String::new();
            f.take(32768).read_to_string(&mut s)?;
            if !status.success() {
                bail!("Command failed: {}", s.trim());
            }
            return Ok(s);
        }
        if std::time::Instant::now() > end {
            child.kill()?;
            child.wait()?;
            bail!("Backend command timed out after 8 seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub(crate) fn systemctl(action: &str, unit: &str) -> Result<String> {
    let mut cmd = Command::new("systemctl");
    cmd.args(["--user", action, unit]);
    bounded(cmd)
}
pub(crate) fn guarded(c: &Candidate) -> Result<()> {
    if c.kind == Kind::Session {
        bail!("Cannot replace a desktop session in place");
    }
    if let Backend::Process { argv, cwd } = &c.backend {
        if argv.is_empty() {
            bail!("Empty launch argv");
        }
        if !cwd.is_dir() {
            bail!("Working directory does not exist");
        }
        if executable(&argv[0]).is_none() {
            bail!("Executable unavailable: {}", argv[0]);
        }
        let base = Path::new(&argv[0])
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if [
            "niri",
            "hyprland",
            "sway",
            "i3",
            "gnome-shell",
            "kwin_wayland",
            "kwin_x11",
            "weston",
            "startplasma-wayland",
            "xfce4-session",
            "enlightenment",
            "labwc",
            "river",
        ]
        .contains(&base.as_str())
        {
            bail!(
                "{} is a compositor/session process, not a replaceable shell layer",
                base
            );
        }
    }
    if let Backend::Systemd { unit } = &c.backend {
        let mut environment = Command::new("systemctl");
        environment.args(["--user", "show-environment"]);
        let environment = bounded(environment)?;
        for key in ["WAYLAND_DISPLAY", "DISPLAY", "XDG_RUNTIME_DIR"] {
            if let Ok(expected) = std::env::var(key) {
                let prefix = format!("{key}=");
                let actual = environment
                    .lines()
                    .find_map(|line| line.strip_prefix(&prefix));
                if actual != Some(expected.as_str()) {
                    bail!(
                        "User service manager {key} differs from this graphical session. Review its environment before switching; Shellswitch will not import it globally."
                    );
                }
            }
        }
        let mut cmd = Command::new("systemctl");
        cmd.args([
            "--user",
            "show",
            unit,
            "--property=ExecStart,PartOf,PropagatesStopTo,Conflicts",
        ]);
        let details = bounded(cmd)?;
        if details
            .lines()
            .any(|l| l.starts_with("PropagatesStopTo=") && l != "PropagatesStopTo=")
        {
            bail!("Unit propagates stop to other units; refusing shell-only control");
        }
        if details
            .lines()
            .any(|l| l.starts_with("Conflicts=") && l != "Conflicts=")
        {
            bail!("Unit has Conflicts= dependencies; review before managing it");
        }
        for p in [
            "/niri ",
            "/Hyprland ",
            "/sway ",
            "/gnome-shell ",
            "/kwin_wayland ",
            "/i3 ",
            "/river ",
            "/labwc ",
        ] {
            if details.contains(p) {
                bail!("Unit runs a compositor/session executable");
            }
        }
    }
    Ok(())
}
pub fn healthy(r: &Running) -> bool {
    match &r.candidate.backend {
        Backend::Process { .. } => r.process.as_ref().is_some_and(process::alive),
        Backend::Systemd { unit } => systemctl("is-active", unit).is_ok(),
        _ => false,
    }
}
// A launch barrier closes the spawn/journal gap: a supervisor cannot execute
// shell code before its exact identity is durable in state.json.
fn prepare_launch(c: &Candidate, store: &Store, s: &mut State, lease: &str) -> Result<Running> {
    guarded(c)?;
    match &c.backend {
        Backend::Process { argv, cwd } => {
            let ticket = store.dir.join("tickets").join(ownership::nonce()?);
            std::fs::create_dir_all(ticket.parent().unwrap())?;
            if let Some(p) = &mut s.pending {
                p.ticket = Some(ticket.clone());
            }
            store.save(s)?;
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(store.dir.join(format!("{}.log", c.id)))?;
            let mut cmd = Command::new(std::env::current_exe()?);
            cmd.arg("supervise")
                .arg("--ticket")
                .arg(&ticket)
                .arg("--")
                .args(argv)
                .current_dir(cwd)
                .env("SHELLSWITCH_STATE_DIR", &store.dir)
                .env("SHELLSWITCH_SHELL_ID", &c.id)
                .env("SHELLSWITCH_HANDOFF", lease)
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = cmd.spawn()?;
            let identity = (0..50)
                .find_map(|_| {
                    let p = process::inspect(child.id());
                    if p.is_none() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    p.map(|p| p.identity)
                })
                .context("Cannot establish supervisor identity")?;
            let r = Running {
                candidate: c.clone(),
                process: Some(identity),
                owned_group: true,
                lease: lease.into(),
            };
            s.active = Some(r.clone());
            store.save(s)?;
            ownership::write(&ticket, &ownership::text("go", 0o600))?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(r)
        }
        Backend::Systemd { unit } => {
            // Persist ownership before StartUnit; rollback will stop the unit even
            // if the caller dies while StartUnit is in flight.
            let r = Running {
                candidate: c.clone(),
                process: None,
                owned_group: false,
                lease: lease.into(),
            };
            s.active = Some(r.clone());
            store.save(s)?;
            systemctl("start", unit)?;
            Ok(r)
        }
        Backend::Review { reason } => bail!("{reason}"),
    }
}
pub(crate) fn stop(r: &Running) -> Result<()> {
    match &r.candidate.backend {
        Backend::Process { .. } => {
            if let Some(id) = &r.process {
                process::stop(id, r.owned_group)?;
            }
        }
        Backend::Systemd { unit } => {
            systemctl("stop", unit)?;
        }
        _ => bail!("No stop backend"),
    }
    Ok(())
}
fn wait_ready(r: &Running) -> Result<()> {
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(125));
        ensure!(healthy(r), "Shell exited during startup; inspect its log");
    }
    if let Some(l) = &r.candidate.lifecycle
        && !l.verify_argv.is_empty()
    {
        let mut cmd = Command::new(&l.verify_argv[0]);
        cmd.args(&l.verify_argv[1..]);
        bounded(cmd).context("Adapter verification failed")?;
    }
    Ok(())
}
pub(crate) fn check_config(s: &State) -> Result<()> {
    ownership::verify(&s.protections)?;
    if let Some(c) = &s.config {
        ensure!(
            ownership::read(&c.target)? == c.expected,
            "Niri configuration ownership drift at {}; run doctor, then repair --yes",
            c.target.display()
        );
        ownership::verify(&c.user_files)?;
        ownership::verify(&c.dependencies)?;
    }
    Ok(())
}
pub fn plan(c: &Candidate, session: &Session, s: &State) -> Result<String> {
    session.compatibility(c).map_err(anyhow::Error::msg)?;
    guarded(c)?;
    ensure!(
        s.pending.is_none(),
        "A transaction is pending: keep, revert, or recover it first"
    );
    ensure!(
        !s.disabled.contains(&c.id),
        "{} is emergency-disabled. Run release {} --yes before selecting it",
        c.name,
        c.id
    );
    ensure!(
        s.config.is_some(),
        "Configuration ownership is not enrolled. Enroll the current Niri config first; otherwise its existing spawn-at-startup entries can resurrect the previous shell"
    );
    if let Some(active) = &s.active
        && active.candidate.id != c.id
    {
        ensure!(
            active.candidate.lifecycle.is_some(),
            "The currently running shell is unmanaged. Register its lifecycle adapter before switching so its supervisor and respawn paths can be stopped"
        );
    }
    ensure!(
        !s.active.as_ref().is_some_and(|r| r.candidate.id == c.id
            && r.candidate.source == c.source
            && healthy(r)),
        "Already active; stage an update before switching to a new revision"
    );
    check_config(s)?;
    Ok(format!(
        "1. Snapshot configuration, ownership and selected shell: {}\n2. Stage destination; validate Niri and explicit conflict policy\n3. Gate inactive CLI/autostart/service sources\n4. Stop only owned processes; apply validated configuration\n5. Start {} and verify process/adapter health\n6. Keep within 20 seconds or restore prior configuration and shell\n\n{}\nVisual usability still needs your confirmation.",
        s.selected.as_deref().unwrap_or("none"),
        c.name,
        c.command()
    ))
}
fn phase(store: &Store, s: &mut State, next: Phase) -> Result<()> {
    s.pending.as_mut().context("No transaction")?.phase = next;
    store.save(s)
}
fn watchdog_spawn(dir: &Path) -> Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--state-dir")
        .arg(dir)
        .arg("watchdog")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}
pub fn switch(dir: &Path, c: &Candidate, session: &Session, all: &[Candidate]) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    plan(c, session, &s)?;
    let mut observed = all.to_vec();
    process::annotate(&mut observed);
    // Unknown findings are not kill targets. Explicit lifecycle registrations may
    // quiesce only their exact declared process matchers and service units.
    for x in observed.iter().filter(|x| !x.running_pids.is_empty()) {
        if !s.active.as_ref().is_some_and(|r| r.candidate.id == x.id)
            && !s
                .installed
                .get(&x.id)
                .and_then(|c| c.lifecycle.as_ref())
                .is_some_and(|l| !l.processes.is_empty() || !l.services.is_empty())
        {
            bail!(
                "Unmanaged shell {} {:?}; adopt its exact PID or register a lifecycle adapter first",
                x.name,
                x.running_pids
            );
        }
    }
    if !s.installed.contains_key(&c.id) {
        s.installed.insert(c.id.clone(), c.clone());
    }
    let (mut changes, next_protections) = lifecycle::protection_plan(&store.dir, &s, Some(&c.id))?;
    let mut next_config = s.config.clone();
    if let Some(cfg) = &mut next_config {
        let (expected, dependencies) = ownership::compose(dir, cfg, Some(c))?;
        ownership::validate_snapshot(dir, &expected)?;
        changes.push(Change {
            path: cfg.target.clone(),
            before: ownership::read(&cfg.target)?,
            after: expected.clone(),
        });
        cfg.expected = expected;
        cfg.dependencies = dependencies;
        cfg.owner = Some(c.id.clone());
    }
    let token = ownership::nonce()?;
    s.pending = Some(Pending {
        previous: s.active.clone(),
        target: c.clone(),
        deadline: now() + 60,
        token: token.clone(),
        phase: Phase::Prepared,
        previous_selected: s.selected.clone(),
        previous_config: s.config.clone(),
        next_config,
        previous_protections: s.protections.clone(),
        next_protections,
        changes,
        ticket: None,
    });
    s.last_event =
        "Prepared switch; all ordinary shell entrypoints are gated during handoff".into();
    store.save(&s)?;
    if let Err(e) = watchdog_spawn(dir) {
        s.pending = None;
        store.save(&s)?;
        return Err(e);
    }
    let outcome = (|| -> Result<()> {
        phase(&store, &mut s, Phase::Protected)?;
        // Publish gates before any process shutdown. Config is applied only later.
        let config_path = s.config.as_ref().map(|c| &c.target);
        let protection_changes: Vec<_> = s
            .pending
            .as_ref()
            .unwrap()
            .changes
            .iter()
            .filter(|c| Some(&c.path) != config_path)
            .cloned()
            .collect();
        ownership::apply_changes(&protection_changes)?;
        lifecycle::reload_services(&s)?;
        lifecycle::stop_commands(&mut s)?;
        store.save(&s)?;
        lifecycle::quiesce(&s, Some(&c.id))?;
        if let Some(old) = &s.active {
            stop(old)?;
        }
        s.active = None;
        phase(&store, &mut s, Phase::Stopped)?;
        let mut remaining = all.to_vec();
        process::annotate(&mut remaining);
        ensure!(
            remaining.iter().all(|c| c.running_pids.is_empty()),
            "A discovered shell is still running after the declared handoff; adapter process coverage is incomplete"
        );
        let config_changes: Vec<_> = s
            .pending
            .as_ref()
            .unwrap()
            .changes
            .iter()
            .filter(|c| Some(&c.path) == s.config.as_ref().map(|c| &c.target))
            .cloned()
            .collect();
        ownership::apply_changes(&config_changes)?;
        s.config = s.pending.as_ref().unwrap().next_config.clone();
        s.protections = s.pending.as_ref().unwrap().next_protections.clone();
        phase(&store, &mut s, Phase::ConfigApplied)?;
        if let Some(cfg) = &s.config {
            ownership::validate(&cfg.target)?;
            if cfg.live_reload {
                crate::niri::reload(&cfg.target)?;
            }
        }
        phase(&store, &mut s, Phase::Starting)?;
        prepare_launch(c, &store, &mut s, &token)?;
        wait_ready(s.active.as_ref().unwrap())?;
        check_config(&s)?;
        lifecycle::verify_inactive(&s, Some(&c.id))?;
        s.pending.as_mut().unwrap().deadline = now() + 20;
        phase(&store, &mut s, Phase::Trial)?;
        Ok(())
    })();
    if let Err(e) = outcome {
        let restored = revert_locked(&store, &mut s);
        return match restored {
            Ok(()) => Err(e.context("Switch failed; previous shell restored")),
            Err(r) => Err(e.context(format!(
                "Switch failed; recovery also failed: {r}. Run recover"
            ))),
        };
    }
    s.last_event =
        "Trial verified. Keep within 20 seconds; otherwise configuration and shell are restored"
            .into();
    store.save(&s)
}
fn stop_ticket(p: &Pending) -> Result<()> {
    if let Some(ticket) = &p.ticket {
        if let Ok(data) = fs::read(ticket.with_extension("pid"))
            && let Ok(id) = serde_json::from_slice::<Identity>(&data)
        {
            process::stop(&id, true)?;
        }
        let _ = fs::remove_file(ticket);
    }
    Ok(())
}
fn revert_locked(store: &Store, s: &mut State) -> Result<()> {
    let p = s.pending.clone().context("No pending transaction")?;
    phase(store, s, Phase::Restoring)?;
    stop_ticket(&p)?;
    if let Some(r) = &s.active {
        stop(r)?;
    }
    s.active = None;
    store.save(s)?;
    lifecycle::stop_commands(s)?;
    ownership::restore_changes(&store.dir, &p.changes)?;
    s.config = p.previous_config;
    s.protections = p.previous_protections;
    s.selected = p.previous_selected.filter(|id| !s.disabled.contains(id));
    store.save(s)?;
    lifecycle::reload_services(s)?;
    if let Some(cfg) = &s.config
        && cfg.live_reload
    {
        crate::niri::reload(&cfg.target)?;
    }
    if let Some(previous) = p.previous
        && s.selected.as_deref() == Some(&previous.candidate.id)
    {
        let lease = ownership::nonce()?;
        // During rollback the handoff contract authorizes only this saved shell.
        s.pending.as_mut().unwrap().target = previous.candidate.clone();
        s.pending.as_mut().unwrap().token = lease.clone();
        phase(store, s, Phase::Starting)?;
        prepare_launch(&previous.candidate, store, s, &lease)?;
        wait_ready(s.active.as_ref().unwrap())?;
    }
    check_config(s)?;
    s.pending = None;
    s.last_event="Previous configuration and shell restored; external edits, if any, were archived under recovery/".into();
    store.save(s)
}
pub fn revert(dir: &Path) -> Result<()> {
    let store = Store::open(dir)?;
    let mut s = store.load()?;
    revert_locked(&store, &mut s)
}
pub fn keep(dir: &Path) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    let p = s.pending.clone().context("No pending trial")?;
    let checks = (|| -> Result<()> {
        ensure!(
            p.phase == Phase::Trial,
            "Transaction is not ready to commit; recover it first"
        );
        ensure!(now() < p.deadline, "Trial expired");
        ensure!(s.active.as_ref().is_some_and(healthy), "Trial shell died");
        check_config(&s)?;
        if let Some(c) = &s.config {
            ownership::validate(&c.target)?;
        }
        wait_ready(s.active.as_ref().unwrap())?;
        lifecycle::verify_inactive(&s, Some(&p.target.id))?;
        Ok(())
    })();
    if let Err(e) = checks {
        revert_locked(&store, &mut s)?;
        return Err(e.context("Commit rejected; previous configuration and shell restored"));
    }
    s.selected = Some(p.target.id);
    s.pending = None;
    s.generation += 1;
    s.last_event = "Switch committed".into();
    store.save(&s)
}
pub fn stop_active(dir: &Path) -> Result<()> {
    let id = {
        let store = Store::open(dir)?;
        let s = store.load()?;
        s.selected
            .or_else(|| s.active.map(|r| r.candidate.id))
            .context("No selected shell")?
    };
    lifecycle::disable(dir, &id)
}
pub fn adopt(dir: &Path, c: &Candidate, pid: Option<u32>) -> Result<()> {
    guarded(c)?;
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(
        s.pending.is_none() && !s.active.as_ref().is_some_and(healthy),
        "Already managing a shell or pending transaction"
    );
    ensure!(
        !s.disabled.contains(&c.id),
        "Release the emergency hold before adoption"
    );
    let process = match &c.backend {
        Backend::Process { .. } => Some(process::adoptable(c, pid.context("A PID is required")?)?),
        Backend::Systemd { unit } => {
            systemctl("is-active", unit)?;
            None
        }
        _ => bail!("Cannot adopt review-only candidate"),
    };
    s.selected = Some(c.id.clone());
    s.installed.entry(c.id.clone()).or_insert(c.clone());
    s.active = Some(Running {
        candidate: c.clone(),
        process,
        owned_group: false,
        lease: ownership::nonce()?,
    });
    s.last_event="Adopted exact process; install/protect its lifecycle adapter to gate external startup sources".into();
    store.save(&s)
}
pub fn resume(dir: &Path, requested: Option<&str>) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let s = store.load()?;
    ensure!(
        s.pending.is_none(),
        "Handoff/recovery in progress; resume is gated"
    );
    let id = s
        .selected
        .clone()
        .context("No selected shell; choose one explicitly")?;
    ensure!(
        requested.is_none_or(|r| r == id),
        "This shell is inactive; only the selected shell may resume"
    );
    ensure!(!s.disabled.contains(&id), "Shell is emergency-disabled");
    check_config(&s)?;
    if s.active.as_ref().is_some_and(healthy) {
        return Ok(());
    }
    let c = s
        .installed
        .get(&id)
        .cloned()
        .context("Selected shell missing from registry")?;
    crate::model::Session::detect()
        .compatibility(&c)
        .map_err(anyhow::Error::msg)?;
    // Resume reuses the full transaction path, including rollback and launch barrier.
    drop(store);
    switch(dir, &c, &crate::model::Session::detect(), &[])?;
    keep(dir)
}
pub fn watchdog(dir: &Path) -> Result<()> {
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let store = Store::open(dir)?;
        let mut s = store.load()?;
        let Some(p) = &s.pending else {
            return Ok(());
        };
        // Acquiring the lock means the mutating parent no longer holds it. A
        // non-trial phase therefore indicates an interrupted switch.
        if p.phase != Phase::Trial || now() >= p.deadline || !s.active.as_ref().is_some_and(healthy)
        {
            if let Err(e) = revert_locked(&store, &mut s) {
                s.last_event =
                    format!("Recovery failed: {e:#}. Inactive gates remain closed; run recover.");
                store.save(&s)?;
                return Err(e);
            }
            return Ok(());
        }
    }
}
