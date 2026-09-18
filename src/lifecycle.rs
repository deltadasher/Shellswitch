//! Per-shell startup inventory, cooperative gates and recovery diagnostics.
use crate::{
    control::{self, Running, State, Store},
    model::{Backend, Candidate},
    ownership::{self, Change, OwnedFile, Snapshot},
    process,
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn registry(dir: &Path) -> Result<Vec<Candidate>> {
    let path = dir.join("state.json");
    if !path.exists() {
        return Ok(vec![]);
    }
    let state: State = serde_json::from_slice(&fs::read(path)?)?;
    Ok(state.installed.into_values().collect())
}

pub fn install(dir: &Path, manifest: &Path, payload: Option<&Path>) -> Result<Candidate> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(
        s.pending.is_none(),
        "Finish or recover the current transaction before installing"
    );
    let source = fs::canonicalize(manifest)?;
    let mut c = crate::discovery::manifest(&source)?;
    let version = store
        .dir
        .join("packages")
        .join(&c.id)
        .join(ownership::nonce()?);
    fs::create_dir_all(&version)?;
    if let Some(root) = payload {
        let root = fs::canonicalize(root)?;
        let dest = version.join("payload");
        ensure!(
            !dest.starts_with(&root),
            "Package destination cannot be inside payload source"
        );
        let mut count = 0;
        copy_tree(&root, &dest, &root, &mut count)?;
        let map = |value: &str| -> String {
            let p = Path::new(value);
            p.strip_prefix(&root)
                .ok()
                .map(|rel| dest.join(rel).to_string_lossy().into_owned())
                .unwrap_or_else(|| value.into())
        };
        if let Backend::Process { argv, cwd } = &mut c.backend {
            for a in argv {
                *a = map(a);
            }
            *cwd = PathBuf::from(map(&cwd.to_string_lossy()));
        }
        if let Some(l) = &mut c.lifecycle {
            if let Some(f) = &mut l.niri_fragment {
                *f = PathBuf::from(map(&f.to_string_lossy()));
            }
            for endpoint in &mut l.commands {
                for route in &mut endpoint.routes {
                    for a in &mut route.argv {
                        *a = map(a);
                    }
                }
            }
            for a in &mut l.verify_argv {
                *a = map(a);
            }
            let mut copied = l.processes.clone();
            for p in &mut copied {
                for a in &mut p.argv_prefix {
                    *a = map(a);
                }
            }
            l.processes.extend(copied);
        }
    }
    if let Some(l) = &mut c.lifecycle
        && let Some(fragment) = &l.niri_fragment
    {
        let (root, files) = ownership::freeze(fragment, &version.join("niri"))?;
        ownership::validate(&root)?;
        l.niri_fragment = Some(root);
        l.frozen = files;
    }
    ownership::write(
        &version.join("shellswitch.toml"),
        &ownership::read(&source)?,
    )?;
    c.source = version.join("shellswitch.toml");
    // Installing never executes upstream installers, rewrites shared config,
    // replaces public launchers, enables services, or changes selected state.
    let replaced_ids: Vec<String> = s
        .installed
        .iter()
        .filter(|(id, old)| {
            *id != &c.id
                && (old.name.eq_ignore_ascii_case(&c.name)
                    || old.source == c.source
                    || old.source.ends_with(&c.source))
        })
        .map(|(id, _)| id.clone())
        .collect();
    for old_id in &replaced_ids {
        s.installed.remove(old_id);
        if s.selected.as_deref() == Some(old_id) {
            s.selected = Some(c.id.clone());
        }
        if let Some(active) = &mut s.active
            && active.candidate.id == *old_id
        {
            active.candidate = c.clone();
        }
    }
    s.installed.insert(c.id.clone(), c.clone());
    // If this revision is already the active process, refresh its candidate
    // metadata in place. Otherwise the registry would gain an adapter while
    // the running state continued to carry the unmanaged pre-install copy.
    if let Some(active) = &mut s.active
        && active.candidate.id == c.id
    {
        active.candidate = c.clone();
    }
    s.last_event = format!(
        "Staged {}. Selection and live configuration were not changed",
        c.name
    );
    store.save(&s)?;
    Ok(c)
}
fn copy_tree(source: &Path, dest: &Path, root: &Path, count: &mut usize) -> Result<()> {
    *count += 1;
    ensure!(*count < 50000, "Package tree exceeds 50000 entries");
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        let real = fs::canonicalize(source)?;
        ensure!(
            real.starts_with(root),
            "Payload symlink escapes root: {}",
            source.display()
        );
        ensure!(
            real.is_file(),
            "Directory symlinks require an explicitly flattened payload: {}",
            source.display()
        );
        return copy_tree(&real, dest, root, count);
    }
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        for e in fs::read_dir(source)? {
            let e = e?;
            if [".git", "target", "node_modules", ".cache"]
                .iter()
                .any(|n| e.file_name() == *n)
            {
                continue;
            }
            copy_tree(&e.path(), &dest.join(e.file_name()), root, count)?;
        }
    } else {
        ensure!(
            meta.is_file(),
            "Special file cannot be staged: {}",
            source.display()
        );
        fs::copy(source, dest)?;
        fs::set_permissions(dest, meta.permissions())?;
        fs::File::open(dest)?.sync_all()?;
    }
    Ok(())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn desired(
    map: &mut BTreeMap<PathBuf, OwnedFile>,
    path: &Path,
    expected: Snapshot,
    owner: &str,
) -> Result<()> {
    let path = ownership::absolute(path)?;
    if let Some(f) = map.get_mut(&path) {
        ensure!(
            f.owner == owner,
            "{} has conflicting owners {} and {}",
            path.display(),
            f.owner,
            owner
        );
        f.expected = expected;
    } else {
        map.insert(
            path.clone(),
            OwnedFile {
                original: ownership::read(&path)?,
                path,
                expected,
                owner: owner.into(),
            },
        );
    }
    Ok(())
}
pub fn protection_plan(
    dir: &Path,
    s: &State,
    target: Option<&str>,
) -> Result<(Vec<Change>, Vec<OwnedFile>)> {
    let mut map: BTreeMap<_, _> = s
        .protections
        .iter()
        .map(|f| (f.path.clone(), f.clone()))
        .collect();
    let exe = std::env::current_exe()?;
    let cfg = crate::discovery::config_home();
    let mut integrated = false;
    for c in s.installed.values() {
        let Some(l) = &c.lifecycle else {
            continue;
        };
        integrated = true;
        for gate in &l.commands {
            let script = format!(
                "#!/bin/sh\n# Shellswitch managed gate: {}\nexec {} --state-dir {} gate {} --entry {} -- \"$@\"\n",
                c.id,
                quote(&exe.to_string_lossy()),
                quote(&dir.to_string_lossy()),
                quote(&c.id),
                quote(&gate.path.to_string_lossy())
            );
            desired(
                &mut map,
                &gate.path,
                ownership::text(&script, 0o755),
                &format!("cli:{}", c.id),
            )?;
        }
        for path in &l.autostart {
            desired(
                &mut map,
                path,
                ownership::text(
                    "[Desktop Entry]\nType=Application\nName=Managed by Shellswitch\nHidden=true\n",
                    0o644,
                ),
                &format!("autostart:{}", c.id),
            )?;
        }
        for unit in &l.services {
            let path = cfg.join("systemd/user").join(unit);
            let active = target == Some(&c.id)
                && !s.disabled.contains(&c.id)
                && matches!(&c.backend,Backend::Systemd{unit:u}if u==unit);
            let expected = if active {
                map.get(&path)
                    .map(|f| f.original.clone())
                    .unwrap_or(ownership::read(&path)?)
            } else {
                Snapshot::Symlink {
                    target: "/dev/null".into(),
                }
            };
            desired(&mut map, &path, expected, &format!("service:{}", c.id))?;
        }
    }
    if integrated {
        let marker = cfg.join("shellswitch/handoff.json");
        if !map.contains_key(&marker) && marker.exists() {
            let prior: serde_json::Value = serde_json::from_slice(&fs::read(&marker)?)?;
            ensure!(
                prior["state_dir"] == dir.to_string_lossy().as_ref(),
                "Another ownership domain already controls this desktop: {}",
                marker.display()
            );
        }
        let record = serde_json::json!({"version":1,"state_dir":dir,"shellswitch":exe,"contract":"shellswitch-handoff-v1"});
        desired(
            &mut map,
            &marker,
            Snapshot::File {
                data: serde_json::to_vec_pretty(&record)?,
                mode: 0o600,
            },
            "handoff-contract",
        )?;
    }
    let files: Vec<_> = map.into_values().collect();
    let mut changes = vec![];
    for f in &files {
        let before = ownership::read(&f.path)?;
        if before != f.expected {
            changes.push(Change {
                path: f.path.clone(),
                before,
                after: f.expected.clone(),
            });
        }
    }
    Ok((changes, files))
}
pub fn reload_services(s: &State) -> Result<()> {
    if s.installed
        .values()
        .any(|c| c.lifecycle.as_ref().is_some_and(|l| !l.services.is_empty()))
    {
        let mut cmd = Command::new("systemctl");
        cmd.args(["--user", "daemon-reload"]);
        control::bounded(cmd)?;
    }
    Ok(())
}
pub fn stop_commands(s: &mut State) -> Result<()> {
    for r in &s.commands {
        control::stop(r)?;
    }
    s.commands.clear();
    Ok(())
}
pub(crate) fn declared<'a>(
    c: &'a Candidate,
    p: &process::Process,
) -> Option<&'a crate::ownership::ProcessMatch> {
    if !process::same_session(p.identity.pid) {
        return None;
    }
    c.lifecycle.as_ref()?.processes.iter().find(|m| {
        m.argv_prefix.len() <= p.argv.len()
            && m.argv_prefix
                .iter()
                .zip(&p.argv)
                .enumerate()
                .all(|(i, (a, b))| {
                    if a == b {
                        return true;
                    }
                    if i == 0 {
                        let b = crate::model::executable(b);
                        return b
                            .is_some_and(|b| fs::canonicalize(a).ok() == fs::canonicalize(b).ok());
                    }
                    false
                })
    })
}
pub fn quiesce(s: &State, _destination: Option<&str>) -> Result<()> {
    // Masked services are stopped through their actual supervisor first.
    for c in s.installed.values() {
        if let Some(l) = &c.lifecycle {
            for unit in &l.services {
                control::systemctl("stop", unit)?;
            }
        }
    }
    let processes = process::all();
    for c in s.installed.values() {
        for p in &processes {
            if let Some(m) = declared(c, p) {
                if m.kill_without_cleanup {
                    process::kill_exact(&p.identity)?;
                } else {
                    process::stop(&p.identity, false)?;
                }
            }
        }
    }
    Ok(())
}
pub fn verify_inactive(s: &State, target: Option<&str>) -> Result<()> {
    for c in s
        .installed
        .values()
        .filter(|c| Some(c.id.as_str()) != target)
    {
        if let Some(l) = &c.lifecycle {
            for unit in &l.services {
                let mut cmd = Command::new("systemctl");
                cmd.args(["--user", "show", unit, "--property=ActiveState", "--value"]);
                let status = control::bounded(cmd)?;
                ensure!(
                    !matches!(status.trim(), "active" | "activating" | "reloading"),
                    "Inactive shell service {unit} is running; disable {} --yes",
                    c.id
                );
            }
        }
        for p in process::all() {
            if declared(c, &p).is_some() && process::alive(&p.identity) {
                bail!(
                    "Inactive shell {} resurrected as PID {}; disable it and inspect startup inventory",
                    c.name,
                    p.identity.pid
                );
            }
        }
    }
    Ok(())
}
pub fn protect(dir: &Path) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(s.pending.is_none(), "Pending transaction; recover first");
    ownership::verify(&s.protections)?;
    let (changes, files) = protection_plan(dir, &s, s.selected.as_deref())?;
    // A separate durable file-operation journal covers protect/repair, which do
    // not change selection or launch a shell.
    file_transaction(&store, &mut s, &changes, &files, None)?;
    s.last_event = "CLI/autostart/service protections installed; no shell was started".into();
    store.save(&s)
}
fn file_transaction(
    store: &Store,
    s: &mut State,
    changes: &[Change],
    files: &[OwnedFile],
    next_config: Option<ownership::Config>,
) -> Result<()> {
    let journal = store.dir.join("files-pending.json");
    ensure!(
        !journal.exists(),
        "Interrupted file operation; run recover first"
    );
    let previous_config = s.config.clone();
    ownership::save_json(
        &journal,
        &serde_json::json!({"changes":changes,"previous_protections":s.protections,"next_protections":files,"previous_config":s.config}),
    )?;
    if let Some(next) = next_config {
        s.config = Some(next);
    }
    let result = (|| -> Result<()> {
        ownership::apply_changes(changes)?;
        reload_services(s)?;
        if let Some(c) = &s.config
            && changes.iter().any(|change| {
                change.path == c.target
                    || c.user_files
                        .iter()
                        .chain(&c.dependencies)
                        .any(|f| f.path == change.path)
            })
        {
            ownership::validate(&c.target)?;
            if c.live_reload {
                crate::niri::reload(&c.target)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        ownership::restore_changes(&store.dir, changes)?;
        s.config = previous_config;
        reload_services(s)?;
        if let Some(c) = &s.config
            && c.live_reload
            && ownership::validate(&c.target).is_ok()
        {
            crate::niri::reload(&c.target)?;
        }
        // A failed repair restores the pre-repair bytes, which may themselves be
        // invalid. Do not load those into the compositor; leave diagnostics.
        fs::remove_file(journal)?;
        return Err(e);
    }
    s.protections = files.to_vec();
    store.save(s)?;
    fs::remove_file(journal)?;
    Ok(())
}
pub fn recover_files(dir: &Path) -> Result<bool> {
    let store = Store::open(dir)?;
    let mut s = store.load()?;
    let path = dir.join("files-pending.json");
    if !path.exists() {
        return Ok(false);
    }
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
    let changes: Vec<Change> = serde_json::from_value(value["changes"].clone())?;
    ownership::restore_changes(dir, &changes)?;
    s.protections = serde_json::from_value(value["previous_protections"].clone())?;
    if let Some(previous) = value.get("previous_config") {
        s.config = serde_json::from_value(previous.clone())?;
    }
    if let Some(c) = &s.config
        && c.live_reload
        && ownership::validate(&c.target).is_ok()
    {
        crate::niri::reload(&c.target)?;
    }
    reload_services(&s)?;
    store.save(&s)?;
    fs::remove_file(path)?;
    Ok(true)
}
pub fn disable(dir: &Path, id: &str) -> Result<()> {
    let store = Store::open(dir)?;
    let mut s = store.load()?;
    ensure!(s.installed.contains_key(id), "Unknown installed shell {id}");
    // Durable deny first; even interrupted cleanup cannot authorize resurrection.
    s.disabled.insert(id.into());
    if s.selected.as_deref() == Some(id) {
        s.selected = None;
    }
    s.last_event = format!("Emergency hold for {id}; cleanup in progress");
    store.save(&s)?;
    if dir.join("files-pending.json").exists() {
        drop(store);
        recover_files(dir)?;
        return disable(dir, id);
    }
    if s.pending.is_some() {
        drop(store);
        control::revert(dir)?;
        return disable(dir, id);
    }
    let (changes, files) = protection_plan(dir, &s, s.selected.as_deref())?;
    // Preserve an overwritten wrapper/config before repairing it.
    for c in &changes {
        ownership::archive(dir, &c.path)?;
    }
    file_transaction(&store, &mut s, &changes, &files, None)?;
    stop_commands(&mut s)?;
    let c = s.installed.get(id).unwrap().clone();
    if let Some(l) = &c.lifecycle {
        for unit in &l.services {
            control::systemctl("stop", unit)?;
        }
    }
    for p in process::all() {
        if let Some(m) = declared(&c, &p) {
            if m.kill_without_cleanup {
                process::kill_exact(&p.identity)?;
            } else {
                process::stop(&p.identity, false)?;
            }
        }
    }
    if s.active.as_ref().is_some_and(|r| r.candidate.id == id) {
        control::stop(s.active.as_ref().unwrap())?;
        s.active = None;
    }
    s.last_event =
        format!("{id} stopped and held disabled. release {id} clears the hold without starting it");
    store.save(&s)
}
pub fn release(dir: &Path, id: &str) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(s.pending.is_none(), "Recover the transaction first");
    ensure!(s.disabled.remove(id), "Shell has no emergency hold");
    s.last_event = format!("Hold released for {id}; still inactive until explicitly selected");
    store.save(&s)
}

pub fn authorize(dir: &Path, id: &str, purpose: &str) -> Result<()> {
    // Lock-free read of an atomically replaced journal. Native startup hooks run
    // while the switching parent owns the mutation lock.
    let s: State = serde_json::from_slice(&fs::read(dir.join("state.json"))?)?;
    ensure!(s.schema == 2, "Unsupported handoff state schema");
    ensure!(!s.disabled.contains(id), "{id} is emergency-disabled");
    let token = std::env::var("SHELLSWITCH_HANDOFF").unwrap_or_default();
    ensure!(
        !token.is_empty(),
        "No authorized Shellswitch lease; use the managed entrypoint"
    );
    ensure!(
        ["start", "runtime", "configure"].contains(&purpose),
        "Unknown contract purpose"
    );
    if let Some(p) = &s.pending {
        ensure!(
            p.target.id == id
                && p.token == token
                && matches!(p.phase, control::Phase::Starting | control::Phase::Trial),
            "Lease is stale or outside its handoff phase"
        );
    } else {
        ensure!(
            purpose != "configure",
            "Configuration writes require an active handoff, not a runtime lease"
        );
        ensure!(
            s.selected.as_deref() == Some(id)
                && s.active.as_ref().is_some_and(|r| r.candidate.id == id
                    && r.lease == token
                    && control::healthy(r)),
            "Shell is not the selected live instance"
        );
    }
    Ok(())
}
pub fn gate(dir: &Path, id: &str, entry: Option<&Path>, args: &[String]) -> Result<()> {
    let store = Store::open(dir)?;
    let mut s = store.load()?;
    // Introspection is always available, including for inactive shells.
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") || args == ["help"] {
        println!("{id}: Shellswitch-managed entrypoint");
        println!("Lifecycle: start / session-start / run (resume selected shell); status");
        println!(
            "Switch, stop, restart or update through Shellswitch to preserve rollback and ownership."
        );
        if let Some(c) = s.installed.get(id)
            && let Some(l) = &c.lifecycle
        {
            for endpoint in &l.commands {
                if entry.is_none_or(|p| endpoint.path == p) {
                    for route in &endpoint.routes {
                        println!("  {}", route.prefix.join(" "));
                    }
                }
            }
        }

        return Ok(());
    }
    if args == ["status"] {
        println!(
            "{id}: selected={}, running={}, disabled={}, trial={}",
            s.selected.as_deref() == Some(id),
            s.active
                .as_ref()
                .is_some_and(|r| r.candidate.id == id && control::healthy(r)),
            s.disabled.contains(id),
            s.pending.is_some()
        );
        return Ok(());
    }
    ensure!(
        gate_allows(&s, id) && !dir.join("files-pending.json").exists(),
        "{id} is inactive, disabled, or undergoing handoff; select it explicitly in Shellswitch"
    );
    control::check_config(&s)?;
    if args
        .first()
        .is_some_and(|a| ["start", "session-start", "run"].contains(&a.as_str()))
    {
        drop(store);
        return control::resume(dir, Some(id));
    }
    ensure!(
        s.active.as_ref().is_some_and(control::healthy),
        "Selected shell is not running; use resume"
    );
    let c = &s.active.as_ref().context("No active revision")?.candidate;
    let l = c.lifecycle.as_ref().context("No declared CLI routes")?;
    let endpoint = l
        .commands
        .iter()
        .find(|e| entry.is_none_or(|p| e.path == p))
        .context("Unknown CLI entrypoint")?;
    let mut routes = endpoint.routes.clone();
    if l.adapter == "tonantzintla-bridge-v1"
        && let crate::model::Backend::Process { argv, .. } = &c.backend
    {
        for (name, tail) in [
            ("lock", vec!["umbra", "lock"]),
            ("preview-lock", vec!["umbra", "preview"]),
            ("quick", vec!["quickactions", "toggle"]),
        ] {
            if !routes.iter().any(|r| r.prefix == [name]) {
                let mut command = argv.clone();
                command.extend(["ipc".into(), "call".into()]);
                command.extend(tail.into_iter().map(str::to_owned));
                routes.push(crate::ownership::Route {
                    prefix: vec![name.into()],
                    argv: command,
                });
            }
        }
    }

    let route=routes.iter().filter(|r|args.starts_with(&r.prefix)).max_by_key(|r|r.prefix.len()).context("Unsupported command through managed gate; use Shellswitch install/switch/disable for lifecycle changes")?;
    let mut argv = route.argv.clone();
    argv.extend_from_slice(&args[route.prefix.len()..]);
    if l.adapter == "tonantzintla-bridge-v1" && args == ["quick"] {
        argv.push("telemetry".into());
    }
    normalize_widget_args(l.adapter.as_str(), &mut argv)?;
    let lease = s.active.as_ref().unwrap().lease.clone();
    let ticket = dir.join("tickets").join(ownership::nonce()?);
    fs::create_dir_all(ticket.parent().unwrap())?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("supervise")
        .arg("--ticket")
        .arg(&ticket)
        .arg("--")
        .args(&argv)
        .envs(crate::adapters::launch_environment(c)?)
        .env("SHELLSWITCH_STATE_DIR", dir)
        .env("SHELLSWITCH_SHELL_ID", id)
        .env("SHELLSWITCH_HANDOFF", &lease)
        .env("SHELLSWITCH_GATED", "1")
        .stdin(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let identity = process::inspect(child.id())
        .context("Cannot inspect gated command")?
        .identity;
    s.commands.push(Running {
        candidate: c.clone(),
        process: Some(identity.clone()),
        owned_group: true,
        lease,
    });
    store.save(&s)?;
    ownership::write(&ticket, &ownership::text("go", 0o600))?;
    drop(store);
    let status = child.wait()?;
    let store = Store::open(dir)?;
    let mut s = store.load()?;
    s.commands.retain(|r| {
        r.process
            .as_ref()
            .is_none_or(|p| p.pid != identity.pid || p.start != identity.start)
    });
    store.save(&s)?;
    ensure!(
        status.success(),
        "Gated command failed or was cancelled by a handoff"
    );
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub detail: String,
    pub repair: String,
}
pub fn diagnostics(dir: &Path, s: &State) -> Vec<Diagnostic> {
    let mut d = vec![];
    if s.config.is_none() && s.selected.is_some() {
        d.push(Diagnostic {
            code: "unowned-config".into(),
            detail: "A shell is selected while the Niri entrypoint is outside Shellswitch ownership".into(),
            repair: "enroll --config ~/.config/niri/config.kdl --user-config REVIEWED_BASELINE --policy preserve-user --yes".into(),
        });
    }
    if let Some(active) = &s.active
        && active.candidate.lifecycle.is_none()
    {
        d.push(Diagnostic {
            code: "unmanaged-active-shell".into(),
            detail: format!(
                "{} is running without a lifecycle adapter",
                active.candidate.name
            ),
            repair: format!(
                "adapter {} --shell-root {} > shellswitch.toml, review it, then install it",
                active.candidate.name.to_ascii_lowercase(),
                active.candidate.source.display()
            ),
        });
    }
    let mut add = |code: &str, detail: String, repair: &str| {
        d.push(Diagnostic {
            code: code.into(),
            detail,
            repair: repair.into(),
        })
    };
    if let Some(p) = &s.pending {
        add(
            "pending",
            format!("{:?}: {}", p.phase, p.target.name),
            "recover (or keep/revert a healthy trial)",
        );
    }
    if dir.join("files-pending.json").exists() {
        add(
            "interrupted-protection",
            "A file operation was interrupted".into(),
            "recover",
        );
    }
    for f in &s.protections {
        match ownership::read(&f.path) {
            Ok(actual) if actual == f.expected => {}
            _ => add(
                "startup-drift",
                format!("{}: {}", f.owner, f.path.display()),
                "repair --yes",
            ),
        }
    }
    if let Some(c) = &s.config {
        if ownership::read(&c.target).ok().as_ref() != Some(&c.expected) {
            add(
                "config-overwrite",
                format!(
                    "Selected {:?}, Niri entrypoint no longer matches saved ownership",
                    s.selected
                ),
                "repair --yes",
            );
        }
        for f in c.user_files.iter().chain(&c.dependencies) {
            if ownership::read(&f.path).ok().as_ref() != Some(&f.expected) {
                add(
                    "include-drift",
                    f.path.display().to_string(),
                    "repair --yes",
                );
            }
        }
        if s.selected.is_some() && c.owner != s.selected && s.pending.is_none() {
            add(
                "owner-mismatch",
                format!("Selected {:?}, config owner {:?}", s.selected, c.owner),
                "switch <selected-id> --yes",
            );
        }
    }
    if s.pending.is_none() {
        if let Some(id) = &s.selected
            && !s
                .active
                .as_ref()
                .is_some_and(|r| r.candidate.id == *id && control::healthy(r))
        {
            add("selected-not-running", id.clone(), "resume");
        }
        if let Some(r) = &s.active
            && control::healthy(r)
            && s.selected.as_deref() != Some(&r.candidate.id)
        {
            add(
                "unexpected-managed-process",
                r.candidate.id.clone(),
                "disable <id> --yes",
            );
        }
    }
    for c in s.installed.values() {
        if let Some(l) = &c.lifecycle {
            for f in &l.frozen {
                if ownership::read(&f.path).ok().as_ref() != Some(&f.expected) {
                    add(
                        "staged-config-drift",
                        f.path.display().to_string(),
                        "repair --yes",
                    );
                }
            }
            for p in process::all() {
                if declared(c, &p).is_some()
                    && process::alive(&p.identity)
                    && s.selected.as_deref() != Some(&c.id)
                    && !s.pending.as_ref().is_some_and(|p| p.target.id == c.id)
                {
                    add(
                        "inactive-running",
                        format!("{} PID {}", c.name, p.identity.pid),
                        &format!("disable {} --yes", c.id),
                    );
                }
            }
        } else {
            add(
                "integration-gap",
                format!("{} has no startup-source adapter", c.name),
                "install a lifecycle manifest; inspect inventory",
            );
        }
    }
    d
}
pub fn doctor(dir: &Path, json: bool) -> Result<()> {
    let store = Store::open(dir)?;
    let s = store.load()?;
    let d = diagnostics(dir, &s);
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
    } else {
        println!(
            "Selected: {} | generation {} | holds: {:?}",
            s.selected.as_deref().unwrap_or("none"),
            s.generation,
            s.disabled
        );
        if d.is_empty() {
            println!("No drift in the registered ownership scope.");
        }
        for item in d {
            println!(
                "{}: {}\n  → shellswitch {}",
                item.code, item.detail, item.repair
            );
        }
    }
    Ok(())
}
pub fn repair(dir: &Path) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(
        s.pending.is_none(),
        "Recover the pending switch before repairing drift"
    );
    let mut files = s.protections.clone();
    if let Some(c) = &s.config {
        files.extend(c.user_files.clone());
        files.extend(c.dependencies.clone());
        files.push(OwnedFile {
            path: c.target.clone(),
            original: c.original.clone(),
            expected: c.expected.clone(),
            owner: "niri-entrypoint".into(),
        });
    }
    for c in s.installed.values() {
        if let Some(l) = &c.lifecycle {
            files.extend(l.frozen.clone());
        }
    }
    let mut map = BTreeMap::new();
    for f in files {
        map.insert(f.path.clone(), f);
    }
    let mut changes = vec![];
    for f in map.values() {
        let actual = ownership::read(&f.path)?;
        if actual != f.expected {
            ownership::archive(dir, &f.path)?;
            changes.push(Change {
                path: f.path.clone(),
                before: actual,
                after: f.expected.clone(),
            });
        }
    }
    let protections = s.protections.clone();
    file_transaction(&store, &mut s, &changes, &protections, None)?;
    if let Some(c) = &s.config {
        ownership::validate(&c.target)?;
    }
    s.last_event="Saved ownership restored; overwritten contents were archived under recovery/. No shell was started".into();
    store.save(&s)
}
#[derive(Debug, Serialize)]
pub struct StartupFinding {
    pub path: PathBuf,
    pub line: usize,
    pub shell: String,
    pub protection: String,
}
/// Bounded static audit: evidence only, never source a script or invoke its CLI.
pub fn startup_findings(s: &State) -> Vec<StartupFinding> {
    let cfg = crate::discovery::config_home();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut pending: Vec<(PathBuf, usize)> = [
        cfg.join("autostart"),
        cfg.join("systemd/user"),
        cfg.join("niri"),
        cfg.join("hypr"),
        cfg.join("sway"),
        home.join(".config/66"),
        home.join(".local/bin"),
        home.join(".profile"),
        home.join(".bash_profile"),
        home.join(".zprofile"),
        home.join(".xinitrc"),
    ]
    .into_iter()
    .map(|p| (p, 0))
    .collect();
    let mut result = vec![];
    let mut count = 0;
    while let Some((path, depth)) = pending.pop() {
        count += 1;
        if count > 10000 {
            break;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() && depth < 10 {
            if let Ok(entries) = fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|e| (e.path(), depth + 1)));
            }
            continue;
        }
        if !meta.is_file() || meta.len() > 262144 {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for c in s.installed.values() {
            let Some(l) = &c.lifecycle else { continue };
            let mut tokens: Vec<String> = l
                .commands
                .iter()
                .filter_map(|g| g.path.file_name())
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            tokens.extend(
                l.processes
                    .iter()
                    .flat_map(|m| m.argv_prefix.iter().skip(1))
                    .filter(|v| v.contains('/'))
                    .cloned(),
            );
            for (line, content) in text.lines().enumerate() {
                if !content.trim_start().starts_with('#')
                    && !content.trim_start().starts_with("//")
                    && tokens.iter().any(|t| !t.is_empty() && content.contains(t))
                {
                    result.push(StartupFinding {path:path.clone(),line:line+1,shell:c.id.clone(),protection:if s.protections.iter().any(|f|f.path == path) { "owned startup file; doctor checks drift" } else { "external reference; only declared command gates apply, inspect indirect/raw launch" }.into()});
                }
            }
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    result
}
pub fn inventory(dir: &Path) -> Result<()> {
    let store = Store::open(dir)?;
    let s = store.load()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "adapters": s.installed.values().map(|c|serde_json::json!({"id":c.id,"name":c.name,"lifecycle":c.lifecycle,"backend":c.backend})).collect::<Vec<_>>(),
            "startup_references": startup_findings(&s),
            "limits": "Static audit, 10000 entries, depth 10, files <=256KiB. Symlinks, generated code, other roots and arbitrary installers require review. Declarations are not proof of complete coverage."
        }))?
    );
    Ok(())
}

/// Import intentional user edits without mistaking them for ownership drift.
pub fn update_config(dir: &Path, user: &Path, policy: ownership::Policy) -> Result<()> {
    let store = Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(
        s.pending.is_none(),
        "Finish the switch before changing user settings"
    );
    control::check_config(&s)?;
    let mut cfg = s.config.clone().context("Enroll a configuration first")?;
    let (root, files) =
        ownership::freeze(user, &dir.join("user-config").join(ownership::nonce()?))?;
    ownership::validate(&root)?;
    cfg.user_root = root;
    cfg.user_files = files;
    cfg.policy = policy;
    let candidate = s
        .active
        .as_ref()
        .filter(|r| s.selected.as_deref() == Some(&r.candidate.id))
        .map(|r| &r.candidate)
        .or_else(|| s.selected.as_ref().and_then(|id| s.installed.get(id)));
    let (expected, dependencies) = ownership::compose(dir, &cfg, candidate)?;
    ownership::validate_snapshot(dir, &expected)?;
    let change = Change {
        path: cfg.target.clone(),
        before: ownership::read(&cfg.target)?,
        after: expected.clone(),
    };
    cfg.expected = expected;
    cfg.dependencies = dependencies;
    cfg.owner = s.selected.clone();
    let protections = s.protections.clone();
    file_transaction(&store, &mut s, &[change], &protections, Some(cfg))?;
    s.last_event =
        "User settings imported with explicit precedence; shell selection unchanged".into();
    store.save(&s)
}

// Serpantinum's IPC method requires cmd, targetWidget and arg, including an
// explicit empty arg when the CLI caller omits its optional subtarget.
fn normalize_widget_args(adapter: &str, argv: &mut Vec<String>) -> Result<()> {
    if adapter == "serpantinum-bridge-v1"
        && let Some(i) = argv
            .windows(4)
            .position(|w| w == ["ipc", "call", "main", "handleCommand"])
    {
        let count = argv.len() - i - 4;
        ensure!(
            (2..=3).contains(&count),
            "Widget command requires a target and at most one subtarget"
        );
        if count == 2 {
            argv.push(String::new());
        }
    }

    Ok(())
}

#[cfg(test)]
mod widget_tests {
    use super::*;
    #[test]
    fn optional_widget_subtarget_is_explicit() {
        let base: Vec<String> = [
            "qs",
            "ipc",
            "call",
            "main",
            "handleCommand",
            "toggle",
            "settings",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut args = base.clone();
        normalize_widget_args("serpantinum-bridge-v1", &mut args).unwrap();
        assert_eq!(args.last().unwrap(), "");
        let mut supplied = base.clone();
        supplied.push("appearance".into());
        normalize_widget_args("serpantinum-bridge-v1", &mut supplied).unwrap();
        assert_eq!(supplied.last().unwrap(), "appearance");
        let mut missing = base;
        missing.pop();
        assert!(normalize_widget_args("serpantinum-bridge-v1", &mut missing).is_err());
    }
}

fn gate_allows(s: &State, id: &str) -> bool {
    !s.disabled.contains(id)
        && match &s.pending {
            Some(p) => {
                p.phase == control::Phase::Trial
                    && p.target.id == id
                    && s.active.as_ref().is_some_and(|r| r.candidate.id == id)
            }
            None => s.selected.as_deref() == Some(id),
        }
}

#[cfg(test)]
mod gate_policy_tests {
    use super::*;
    #[test]
    fn arbitrary_shell_selection_and_emergency_hold() {
        let mut s = State {
            selected: Some("unknown-shell-42".into()),
            ..State::default()
        };
        assert!(gate_allows(&s, "unknown-shell-42"));
        assert!(!gate_allows(&s, "other"));
        s.disabled.insert("unknown-shell-42".into());
        assert!(!gate_allows(&s, "unknown-shell-42"));
    }
    #[test]
    fn help_and_status_work_without_a_running_shell() {
        let dir =
            std::env::temp_dir().join(format!("shellswitch-gate-{}", ownership::nonce().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        gate(&dir, "arbitrary", None, &[]).unwrap();
        gate(&dir, "arbitrary", None, &["status".into()]).unwrap();
        assert!(gate(&dir, "arbitrary", None, &["start".into()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
