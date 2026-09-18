use crate::model::*;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 512 * 1024;
const MAX_FILES: usize = 30000;
#[derive(Debug, Serialize)]
pub struct Report {
    pub session: Session,
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
    pub files_visited: usize,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub id: Option<String>,
    pub lifecycle: Option<crate::ownership::Lifecycle>,
    #[serde(default = "shell_kind")]
    pub kind: Kind,
    #[serde(default)]
    pub protocols: BTreeSet<String>,
    #[serde(default)]
    pub compositors: BTreeSet<String>,
    #[serde(default)]
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub systemd_unit: Option<String>,
    #[serde(default)]
    pub required_globals: BTreeSet<String>,
}
fn shell_kind() -> Kind {
    Kind::Shell
}
pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}
pub fn default_roots() -> Vec<PathBuf> {
    let mut paths = vec![
        config_home(),
        home().join(".local/bin"),
        home().join(".local/share/applications"),
        home().join(".local/share/wayland-sessions"),
        home().join(".local/share/xsessions"),
        PathBuf::from("/etc/xdg"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/share/wayland-sessions"),
        PathBuf::from("/usr/share/xsessions"),
        PathBuf::from("/usr/lib/systemd/user"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for p in std::env::split_paths(&dirs) {
            paths.push(p.join("applications"));
            paths.push(p.join("wayland-sessions"));
            paths.push(p.join("xsessions"));
        }
    }
    if let Some(dirs) = std::env::var_os("XDG_CONFIG_DIRS") {
        paths.extend(std::env::split_paths(&dirs));
    }
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(&dir).join("applications"));
        // Installed shells commonly live in an application-owned tree under
        // XDG_DATA_HOME (for example ~/.local/share/<shell>/src/quickshell).
        // Scanning only applications misses those runtimes entirely.
        paths.push(PathBuf::from(dir));
    } else {
        paths.push(home().join(".local/share"));
    }
    paths
}
fn read(path: &Path) -> Option<String> {
    let m = fs::metadata(path).ok()?;
    if !m.is_file() || m.len() > MAX_FILE {
        return None;
    }
    fs::read_to_string(path).ok()
}
fn walk(
    path: &Path,
    depth: usize,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    if out.len() >= MAX_FILES || depth > 12 {
        return;
    }
    let Ok(real) = fs::canonicalize(path) else {
        return;
    };
    if !seen.insert(real.clone()) {
        return;
    }
    if real.is_file() {
        out.push(real);
        return;
    }
    if !real.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(&real) else {
        warnings.push(format!("Cannot read {}", real.display()));
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if [
            ".git",
            "node_modules",
            "target",
            "Cache",
            "cache",
            "CachedData",
            "GPUCache",
            "__pycache__",
            "BraveSoftware",
            "chromium",
            "google-chrome",
            "discord",
            "Code",
            "Codex",
        ]
        .contains(&name.as_ref())
        {
            continue;
        }
        walk(&p, depth + 1, seen, out, warnings);
    }
}
pub(crate) fn base(path: &Path, name: String, framework: &str, kind: Kind) -> Candidate {
    Candidate {
        id: stable_id(path),
        name,
        kind,
        framework: framework.into(),
        source: path.into(),
        evidence: vec![],
        protocols: BTreeSet::new(),
        compositors: BTreeSet::new(),
        backend: Backend::Review {
            reason: "Lifecycle has not been established; add a shellswitch.toml manifest".into(),
        },
        required_globals: BTreeSet::new(),
        lifecycle: None,
        declared: false,
        running_pids: vec![],
    }
}
fn folder_name(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);
    let name = parent.file_name().unwrap_or_default().to_string_lossy();
    if name == "quickshell" {
        if parent == config_home().join("quickshell") {
            return "Quickshell · default".into();
        }
        for ancestor in parent.ancestors().skip(1) {
            let n = ancestor.file_name().unwrap_or_default().to_string_lossy();
            if !["src", "config", ".config", "shell", "share"].contains(&n.as_ref())
                && !n.is_empty()
            {
                return n.into_owned();
            }
        }
    }
    name.into_owned()
}

fn source_files(path: &Path, all: &[PathBuf]) -> String {
    // Do not let nested shell configurations contaminate their parent's evidence.
    let root = path.parent().unwrap_or(path);
    let nested: Vec<_> = all
        .iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n == "shell.qml" || n == "shellswitch.toml")
                && p.parent() != Some(root)
                && p.starts_with(root)
        })
        .filter_map(|p| p.parent())
        .collect();
    let mut text = String::new();
    for p in all
        .iter()
        .filter(|p| p.starts_with(root) && !nested.iter().any(|n| p.starts_with(n)))
    {
        if text.len() > 2_000_000 {
            break;
        }
        if p.extension().is_some_and(|e| {
            ["qml", "js", "ts", "tsx", "yuck", "py", "rs", "sh"]
                .iter()
                .any(|x| e == *x)
        }) && let Some(s) = read(p)
        {
            text.push_str(&s);
            text.push('\n');
        }
    }
    text
}
fn capabilities(c: &mut Candidate, source: &str) {
    let s = source.to_lowercase();
    for (label, needles) in [
        (
            "panel/layer surface",
            vec![
                "panelwindow",
                "layershell",
                "layer_shell",
                "layer-shell",
                "astal.window",
                "exclusivity",
                ":exclusive",
            ],
        ),
        (
            "system tray",
            vec!["systemtray", "system_tray", "statusnotifier", "astaltray"],
        ),
        (
            "workspace integration",
            vec!["workspaces", "hyprland", "niri msg", "swaymsg"],
        ),
        (
            "notifications",
            vec!["notificationserver", "astalnotifd", "notifications"],
        ),
        (
            "launcher",
            vec!["desktopentries", "astalapps", "applications"],
        ),
    ] {
        if needles.iter().any(|n| s.contains(n)) {
            c.evidence.push(format!("Source references {label}"));
        }
    }
    // Imports and command calls are evidence, not a proof that a branch is mandatory.
    for (wm, needles) in [
        (
            "hyprland",
            vec!["quickshell.hyprland", "hyprctl", "gi://astalhyprland"],
        ),
        ("niri", vec!["niri msg", "niri\", \"msg", "niri', 'msg"]),
        ("sway", vec!["swaymsg", "swaysock"]),
    ] {
        if needles.iter().any(|n| s.contains(n)) {
            c.compositors.insert(wm.into());
            c.evidence.push(format!(
                "Compositor API reference: {wm} (inferred constraint; manifest can override)"
            ));
        }
    }
}
pub fn manifest(path: &Path) -> Result<Candidate> {
    let m: Manifest = toml::from_str(&fs::read_to_string(path)?)?;
    if m.name.trim().is_empty() {
        bail!("Manifest name cannot be empty");
    }
    if m.argv.is_empty() == m.systemd_unit.is_none() {
        bail!("Specify exactly one of argv or systemd_unit");
    }
    let mut c = base(path, m.name, "manifest", m.kind);
    if let Some(id) = m.id {
        anyhow::ensure!(
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Invalid shell id"
        );
        c.id = id;
    }
    c.lifecycle = m.lifecycle;
    if let Some(l) = &mut c.lifecycle {
        if let Some(p) = &mut l.niri_fragment
            && !p.is_absolute()
        {
            *p = path.parent().unwrap().join(&*p);
        }
        for unit in &l.services {
            anyhow::ensure!(
                unit.ends_with(".service")
                    && !unit.starts_with('-')
                    && !unit.contains('/')
                    && !unit.chars().any(char::is_whitespace),
                "Expected dedicated user .service names"
            );
        }
        for p in &l.processes {
            anyhow::ensure!(
                p.argv_prefix.len() >= 2 && std::path::Path::new(&p.argv_prefix[0]).is_absolute(),
                "Process matcher requires an absolute executable plus exact identifying arguments"
            );
        }
        for command in &l.commands {
            anyhow::ensure!(
                command.path.is_absolute(),
                "CLI gate paths must be absolute"
            );
            for r in &command.routes {
                anyhow::ensure!(
                    !r.prefix.is_empty() && !r.argv.is_empty(),
                    "CLI routes require a non-empty prefix and argv"
                );
            }
        }
    }
    c.required_globals = m.required_globals;
    c.declared = true;
    c.protocols = m.protocols;
    c.compositors = m.compositors;
    c.evidence
        .push("Explicit local shell manifest; declaration is not independent verification".into());
    c.backend = if let Some(unit) = m.systemd_unit {
        if !unit.ends_with(".service")
            || unit.starts_with('-')
            || unit.contains('/')
            || unit.chars().any(char::is_whitespace)
        {
            bail!("Expected a user .service unit name");
        }
        Backend::Systemd { unit }
    } else {
        let dir = path.parent().context("Manifest has no parent")?;
        let cwd = m
            .cwd
            .map(|p| if p.is_absolute() { p } else { dir.join(p) })
            .unwrap_or_else(|| dir.into());
        let mut argv = m.argv;
        if argv[0].starts_with('.') {
            argv[0] = cwd.join(&argv[0]).to_string_lossy().into_owned();
        }
        Backend::Process { argv, cwd }
    };
    Ok(c)
}
fn desktop(path: &Path, text: &str) -> Option<Candidate> {
    let mut section = "";
    let mut fields = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if section == "[Desktop Entry]"
            && let Some((k, v)) = line.split_once('=')
        {
            fields.insert(k, v);
        }
    }
    if fields.get("Hidden") == Some(&"true") {
        return None;
    }
    let session = path
        .components()
        .any(|p| p.as_os_str() == "wayland-sessions" || p.as_os_str() == "xsessions");
    let desc = format!(
        "{} {} {}",
        fields.get("Name").unwrap_or(&""),
        fields.get("Comment").unwrap_or(&""),
        fields.get("Categories").unwrap_or(&"")
    )
    .to_lowercase();
    if !session
        && !["desktop shell", "panel", "dock", "shell;"]
            .iter()
            .any(|x| desc.contains(x))
    {
        return None;
    }
    let mut c = base(
        path,
        fields
            .get("Name")
            .unwrap_or(&"Unnamed desktop entry")
            .to_string(),
        "desktop-entry",
        if session {
            Kind::Session
        } else {
            Kind::Candidate
        },
    );
    c.evidence.push(format!("Desktop entry: {desc}"));
    if let Some(exec) = fields.get("Exec") {
        c.evidence
            .push(format!("Declared Exec (not evaluated): {exec}"));
    }
    c.backend = Backend::Review { reason: if session { "Desktop session; use your display manager. Never replace a live compositor from a panel switch." } else { "Desktop metadata alone cannot establish a shell or its lifecycle. Review Exec and add a manifest." }.into() };
    Some(c)
}
pub fn scan(roots: &[PathBuf]) -> Report {
    let mut files = vec![];
    let mut warnings = vec![];
    let mut seen = HashSet::new();
    for root in roots {
        walk(root, 0, &mut seen, &mut files, &mut warnings);
    }
    if files.len() >= MAX_FILES {
        warnings.push("Scan reached 30000-file budget; scan narrower --root paths".into());
    }
    files.sort();
    let mut candidates = vec![];
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name == "shellswitch.toml"
            || path.extension().is_some_and(|e| e == "toml")
                && path
                    .parent()
                    .is_some_and(|p| p.ends_with("shellswitch/shells"))
        {
            match manifest(path) {
                Ok(c) => candidates.push(c),
                Err(e) => warnings.push(format!("{}: {e}", path.display())),
            };
            continue;
        }
        let lower_name = name.to_ascii_lowercase();
        let relevant = lower_name == "shell.qml"
            || name == "eww.yuck"
            || ["app.ts", "app.tsx", "config.js", "config.ts", "main.py"].contains(&name.as_ref())
            || path
                .extension()
                .is_some_and(|e| e == "desktop" || e == "service" || e == "sh");
        if !relevant {
            continue;
        }
        let Some(text) = read(path) else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "desktop") {
            if let Some(c) = desktop(path, &text) {
                candidates.push(c);
            }
            continue;
        }
        if lower_name == "shell.qml" && text.contains("import Quickshell") {
            let mut c = base(path, folder_name(path), "Quickshell", Kind::Shell);
            c.evidence
                .push("Quickshell entrypoint imports its shell runtime".into());
            let source = source_files(path, &files);
            capabilities(&mut c, &source);
            if source.contains("WlrLayershell") || source.contains("Quickshell.Hyprland") {
                c.protocols.insert("wayland".into());
            }
            c.required_globals.insert("zwlr_layer_shell_v1".into());
            // On X11 Quickshell uses native panels, not the Wayland layer-shell protocol.
            if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                c.required_globals.clear();
            }
            let shell_root = text.contains("ShellRoot") || text.contains("Shell {");
            let surfaces = source.contains("PanelWindow") || source.contains("WlrLayershell");
            if surfaces || shell_root {
                c.evidence.push(
                    if surfaces { "Entrypoint tree contains a panel surface; static evidence, not runtime proof" }
                    else { "Quickshell entrypoint declares a shell root; static evidence, not runtime proof" }
                        .into(),
                );
                if let Some(qs) = executable("qs").or_else(|| executable("quickshell")) {
                    c.backend = Backend::Process {
                        argv: vec![
                            qs.to_string_lossy().into_owned(),
                            "-p".into(),
                            path.to_string_lossy().into_owned(),
                        ],
                        cwd: path.parent().unwrap().into(),
                    };
                } else {
                    c.evidence.push("Missing Quickshell executable".into());
                }
            } else {
                c.kind = Kind::Candidate;
            }
            // Quickshell can target X11 or Wayland. Host support must be checked at launch.
            candidates.push(c);
            continue;
        }
        if name == "eww.yuck" && text.contains("(defwindow") {
            let mut c = base(path, folder_name(path), "Eww", Kind::Component);
            capabilities(&mut c, &text);
            c.evidence.push(
                "Eww window definitions; window selection and daemon ownership need a manifest"
                    .into(),
            );
            candidates.push(c);
            continue;
        }
        if text.contains("astal") && (text.contains("app.start(") || text.contains("App.config(")) {
            let mut c = base(path, folder_name(path), "AGS / Astal", Kind::Shell);
            c.protocols.insert("wayland".into());
            capabilities(&mut c, &source_files(path, &files));
            c.evidence.push(
                "Astal application entrypoint; version-dependent launch syntax requires manifest"
                    .into(),
            );
            candidates.push(c);
            continue;
        }
        let lower = text.to_lowercase();
        if path.extension().is_some_and(|e| e == "service")
            && [
                "desktop shell",
                "panel",
                "quickshell",
                "astal",
                "layer-shell",
            ]
            .iter()
            .any(|s| lower.contains(s))
        {
            let mut c = base(path, name.to_string(), "systemd metadata", Kind::Candidate);
            c.evidence.extend(
                text.lines()
                    .filter(|l| {
                        l.starts_with("Description=")
                            || l.starts_with("ExecStart=")
                            || l.starts_with("ExecStop=")
                    })
                    .map(String::from),
            );
            candidates.push(c);
        } else if text.starts_with("#!")
            && [
                "quickshell",
                "ags run",
                "eww open",
                "layer-shell",
                "desktop shell",
            ]
            .iter()
            .any(|s| lower.contains(s))
        {
            let mut c = base(path, name.to_string(), "launcher script", Kind::Candidate);
            c.evidence
                .push("Script references a shell runtime; not executed during discovery".into());
            c.evidence.extend(
                text.lines()
                    .filter(|l| {
                        ["quickshell", "ags run", "eww open", "exec "]
                            .iter()
                            .any(|s| l.contains(s))
                    })
                    .take(8)
                    .map(|s| s.chars().take(300).collect()),
            );
            candidates.push(c);
        }
    }
    // Explicit manifest owns its directory and supersedes an inferred entry in that directory.
    let declared_dirs: HashSet<_> = candidates
        .iter()
        .filter(|c| c.declared)
        .filter_map(|c| c.source.parent().map(Path::to_path_buf))
        .collect();
    candidates
        .retain(|c| c.declared || !c.source.parent().is_some_and(|p| declared_dirs.contains(p)));
    crate::generic::augment(&files, &mut candidates, &mut warnings);
    fn rank(c: &Candidate) -> u8 {
        match c.kind {
            Kind::Shell => 0,
            Kind::Component => 1,
            Kind::Candidate => 2,
            Kind::Session => 3,
        }
    }
    candidates.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then(
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id)),
        )
    });
    crate::process::annotate(&mut candidates);
    Report {
        session: Session::detect(),
        candidates,
        warnings,
        files_visited: files.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_quickshell_name_and_symlink_dedup() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("never-seen-before");
        fs::create_dir(&p).unwrap();
        fs::write(
            p.join("shell.qml"),
            "import Quickshell\nShellRoot { PanelWindow {} }",
        )
        .unwrap();
        std::os::unix::fs::symlink(&p, t.path().join("alias")).unwrap();
        let r = scan(&[t.path().into()]);
        assert_eq!(r.candidates.len(), 1);
        assert_eq!(r.candidates[0].name, "never-seen-before");
        assert_eq!(r.candidates[0].kind, Kind::Shell);
    }
    #[test]
    fn ordinary_qml_is_not_a_shell() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("shell.qml"), "import QtQuick\nWindow {}").unwrap();
        assert!(scan(&[t.path().into()]).candidates.is_empty());
    }
    #[test]
    fn metadata_is_not_execution_authority() {
        let p = Path::new("/tmp/app.desktop");
        let c = desktop(p,"[Desktop Entry]\nName=Desktop shell\nExec=sh -c evil\n[Desktop Action Foo]\nExec=other").unwrap();
        assert!(matches!(c.backend, Backend::Review { .. }));
        assert!(c.evidence.iter().any(|s| s.contains("sh -c evil")));
    }
    #[test]
    fn manifest_unknown_fields_rejected() {
        assert!(toml::from_str::<Manifest>("name='x'\nargz=['x']").is_err());
    }
    #[test]
    fn nested_config_does_not_contaminate_parent() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("child")).unwrap();
        fs::write(
            t.path().join("shell.qml"),
            "import Quickshell\nShellRoot {}",
        )
        .unwrap();
        fs::write(
            t.path().join("child/shell.qml"),
            "import Quickshell\nPanelWindow {}",
        )
        .unwrap();
        let r = scan(&[t.path().into()]);
        let c = r
            .candidates
            .iter()
            .find(|c| c.source == t.path().join("shell.qml"))
            .unwrap();
        assert_eq!(c.kind, Kind::Shell);
    }

    #[test]
    fn finds_installed_quickshell_under_xdg_data_with_case_insensitive_entrypoint() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("serpantinum/src/quickshell");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Shell.qml"),
            "import Quickshell\nShellRoot {}\nPanelWindow {}",
        )
        .unwrap();
        let report = scan(&[t.path().into()]);
        let candidate = report
            .candidates
            .iter()
            .find(|c| c.source == root.join("Shell.qml"))
            .expect("installed Shell.qml should be discovered");
        assert_eq!(candidate.name, "serpantinum");
        assert_eq!(candidate.kind, Kind::Shell);
    }
}
