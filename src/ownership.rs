//! Durable file ownership. No path/name is treated as a security boundary.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Snapshot {
    Absent,
    File { data: Vec<u8>, mode: u32 },
    Symlink { target: PathBuf },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedFile {
    pub path: PathBuf,
    pub original: Snapshot,
    pub expected: Snapshot,
    pub owner: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub path: PathBuf,
    pub before: Snapshot,
    pub after: Snapshot,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, clap::ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    AbortConflicts,
    PreserveUser,
    ShellWins,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub target: PathBuf,
    #[serde(default)]
    pub live_reload: bool,
    pub original: Snapshot,
    pub expected: Snapshot,
    pub user_root: PathBuf,
    pub user_files: Vec<OwnedFile>,
    pub policy: Policy,
    pub owner: Option<String>,
    pub dependencies: Vec<OwnedFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    #[serde(default)]
    pub adapter: String,
    pub niri_fragment: Option<PathBuf>,
    #[serde(default)]
    pub commands: Vec<CommandGate>,
    #[serde(default)]
    pub autostart: Vec<PathBuf>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub processes: Vec<ProcessMatch>,
    #[serde(default)]
    pub verify_argv: Vec<String>,
    #[serde(default)]
    pub frozen: Vec<OwnedFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandGate {
    pub path: PathBuf,
    #[serde(default)]
    pub routes: Vec<Route>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub prefix: Vec<String>,
    pub argv: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessMatch {
    pub argv_prefix: Vec<String>,
    #[serde(default)]
    pub kill_without_cleanup: bool,
}

pub fn nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn absolute(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "Expected absolute path: {}",
        path.display()
    );
    ensure!(
        !path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir)),
        "Parent traversal is not supported: {}",
        path.display()
    );
    let parent = path.parent().context("Path has no parent")?;
    fs::create_dir_all(parent)?;
    Ok(fs::canonicalize(parent)?.join(path.file_name().context("Path has no filename")?))
}
pub fn read(path: &Path) -> Result<Snapshot> {
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Snapshot::Absent),
        Err(e) => return Err(e.into()),
    };
    if m.file_type().is_symlink() {
        return Ok(Snapshot::Symlink {
            target: fs::read_link(path)?,
        });
    }
    ensure!(m.is_file(), "Not a regular file: {}", path.display());
    ensure!(
        m.len() <= 4 * 1024 * 1024,
        "Ownership snapshot exceeds 4 MiB: {}",
        path.display()
    );
    Ok(Snapshot::File {
        data: fs::read(path)?,
        mode: m.permissions().mode() & 0o777,
    })
}
pub fn text(s: &str, mode: u32) -> Snapshot {
    Snapshot::File {
        data: s.as_bytes().to_vec(),
        mode,
    }
}
pub fn bytes(s: &Snapshot) -> Result<&[u8]> {
    match s {
        Snapshot::File { data, .. } => Ok(data),
        _ => bail!("Expected a regular configuration file, not a symlink or missing path"),
    }
}
pub fn write(path: &Path, s: &Snapshot) -> Result<()> {
    let parent = path.parent().context("No parent")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".shellswitch-{}", nonce()?));
    match s {
        Snapshot::Absent => {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            };
        }
        Snapshot::File { data, mode } => {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(*mode)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&tmp)?;
            f.write_all(data)?;
            f.set_permissions(fs::Permissions::from_mode(*mode))?;
            f.sync_all()?;
            if let Err(e) = fs::rename(&tmp, path) {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
        Snapshot::Symlink { target } => {
            std::os::unix::fs::symlink(target, &tmp)?;
            if let Err(e) = fs::rename(&tmp, path) {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
        }
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn save_json(path: &Path, value: &impl Serialize) -> Result<()> {
    write(
        path,
        &Snapshot::File {
            data: serde_json::to_vec_pretty(value)?,
            mode: 0o600,
        },
    )
}
pub fn archive(dir: &Path, path: &Path) -> Result<PathBuf> {
    let saved = dir.join("recovery").join(format!("{}.json", nonce()?));
    save_json(
        &saved,
        &serde_json::json!({"path":path,"snapshot":read(path)?}),
    )?;
    Ok(saved)
}
pub fn restore_changes(dir: &Path, changes: &[Change]) -> Result<()> {
    for c in changes.iter().rev() {
        let actual = read(&c.path)?;
        if actual != c.before && actual != c.after {
            archive(dir, &c.path)?;
        }
        if actual != c.before {
            write(&c.path, &c.before)?;
        }
    }
    Ok(())
}
pub fn apply_changes(changes: &[Change]) -> Result<()> {
    for c in changes {
        ensure!(
            read(&c.path)? == c.before,
            "External change at {}; run doctor / repair",
            c.path.display()
        );
        write(&c.path, &c.after)?;
    }
    Ok(())
}
pub fn verify(files: &[OwnedFile]) -> Result<()> {
    for f in files {
        ensure!(
            read(&f.path)? == f.expected,
            "Ownership drift at {}; run doctor / repair",
            f.path.display()
        );
    }
    Ok(())
}

// A bounded lexical scanner, not a KDL semantic rewriter. It locates top-level
// includes and section names; niri itself remains the syntax/semantic validator.
#[derive(Clone, Debug)]
struct Token {
    value: String,
    start: usize,
    end: usize,
    string: bool,
}
fn tokens(s: &str) -> Result<Vec<Token>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = vec![];
    while i < b.len() {
        let start = i;
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            let mut d = 1;
            while i < b.len() && d > 0 {
                if b.get(i..i + 2) == Some(b"/*") {
                    d += 1;
                    i += 2;
                } else if b.get(i..i + 2) == Some(b"*/") {
                    d -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ensure!(d == 0, "Unclosed KDL comment");
            continue;
        }
        if b[i].is_ascii_whitespace() && b[i] != b'\n' {
            i += 1;
            continue;
        }
        let mut hashes = 0;
        while b.get(i + hashes) == Some(&b'#') {
            hashes += 1;
        }
        if b.get(i + hashes) == Some(&b'"') {
            ensure!(
                b.get(i + hashes..i + hashes + 3) != Some(b"\"\"\""),
                "Multiline KDL strings need manual flattening before enrollment"
            );
            i += hashes + 1;
            let content = i;
            let mut value = String::new();
            let mut chunk = i;
            let mut closed = false;
            while i < b.len() {
                if b[i] == b'"' && (0..hashes).all(|h| b.get(i + 1 + h) == Some(&b'#')) {
                    value.push_str(&s[chunk..i]);
                    i += 1 + hashes;
                    closed = true;
                    break;
                }
                if hashes == 0 && b[i] == b'\\' {
                    value.push_str(&s[chunk..i]);
                    i += 1;
                    let ch = *b.get(i).context("Unfinished escape")?;
                    match ch {
                        b'"' => value.push('"'),
                        b'\\' => value.push('\\'),
                        b'n' => value.push('\n'),
                        b'r' => value.push('\r'),
                        b't' => value.push('\t'),
                        _ => {
                            value.push('\\');
                            value.push(ch as char);
                        }
                    }
                    i += 1;
                    chunk = i;
                } else {
                    i += 1;
                }
            }
            ensure!(closed, "Unclosed KDL string at {content}");
            out.push(Token {
                value,
                start,
                end: i,
                string: true,
            });
            continue;
        }
        if b"{};\n=".contains(&b[i]) {
            i += 1;
            out.push(Token {
                value: s[start..i].into(),
                start,
                end: i,
                string: false,
            });
            continue;
        }
        while i < b.len() && !b[i].is_ascii_whitespace() && !b"{};=\"".contains(&b[i]) {
            if b.get(i..i + 2) == Some(b"//") || b.get(i..i + 2) == Some(b"/*") {
                break;
            }
            i += 1;
        }
        ensure!(i > start, "Unsupported KDL token");
        out.push(Token {
            value: s[start..i].into(),
            start,
            end: i,
            string: false,
        });
    }
    Ok(out)
}
fn nodes(s: &str) -> Result<Vec<Vec<Token>>> {
    let mut result = vec![];
    let mut node = vec![];
    let mut depth = 0i32;
    for token in tokens(s)? {
        if !token.string {
            if token.value == "{" {
                depth += 1;
            } else if token.value == "}" {
                depth -= 1;
                ensure!(depth >= 0, "Unbalanced KDL braces");
            }
            if depth == 0 && (token.value == "\n" || token.value == ";") {
                if !node.is_empty() {
                    result.push(std::mem::take(&mut node));
                }
                continue;
            }
        }
        node.push(token);
    }
    ensure!(depth == 0, "Unbalanced KDL braces");
    if !node.is_empty() {
        result.push(node);
    }
    Ok(result)
}
pub fn sections(path: &Path, seen: &mut BTreeSet<PathBuf>) -> Result<BTreeSet<String>> {
    ensure!(seen.len() < 128, "Too many configuration includes");
    let real = fs::canonicalize(path)?;
    if !seen.insert(real.clone()) {
        return Ok(BTreeSet::new());
    }
    let s = fs::read_to_string(&real)?;
    let mut result = BTreeSet::new();
    for n in nodes(&s)? {
        let name = &n[0].value;
        if name == "/-" {
            continue;
        }
        if name == "include" {
            let t = n
                .iter()
                .skip(1)
                .find(|t| t.string)
                .context("Unsupported include syntax")?;
            result.extend(sections(&PathBuf::from(&t.value), seen)?);
        } else {
            result.insert(name.clone());
        }
    }
    Ok(result)
}
pub fn freeze(source: &Path, dest: &Path) -> Result<(PathBuf, Vec<OwnedFile>)> {
    fs::create_dir_all(dest)?;
    let mut files = vec![];
    let mut stack = BTreeSet::new();
    let root = freeze_one(source, dest, &mut files, &mut stack)?;
    Ok((root, files))
}
fn freeze_one(
    source: &Path,
    dest: &Path,
    files: &mut Vec<OwnedFile>,
    stack: &mut BTreeSet<PathBuf>,
) -> Result<PathBuf> {
    ensure!(
        files.len() + stack.len() < 128,
        "Config include limit reached"
    );
    let real = fs::canonicalize(source)
        .with_context(|| format!("Missing include {}", source.display()))?;
    ensure!(
        stack.insert(real.clone()),
        "Config include cycle at {}",
        real.display()
    );
    let snapshot = read(&real)?;
    let s = std::str::from_utf8(bytes(&snapshot)?)?;
    let mut replacements = vec![];
    for n in nodes(s)? {
        if n[0].value != "include" {
            continue;
        }
        let t = n.iter().skip(1).find(|t| t.string).context(
            "Include needs a quoted file path; unsupported syntax must be flattened explicitly",
        )?;
        ensure!(
            !t.value.contains('\\') && !t.value.contains('\n'),
            "Unsupported escaped include path"
        );
        let path = if let Some(rel) = t.value.strip_prefix("~/") {
            PathBuf::from(std::env::var_os("HOME").context("HOME missing")?).join(rel)
        } else {
            let p = PathBuf::from(&t.value);
            if p.is_absolute() {
                p
            } else {
                real.parent().unwrap().join(p)
            }
        };
        // Missing optional includes are also rejected: future file creation must not
        // silently change an already-validated generation.
        let captured = freeze_one(&path, dest, files, stack)?;
        replacements.push((t.start, t.end, serde_json::to_string(&captured)?));
    }
    let mut rewritten = s.to_owned();
    for (start, end, text) in replacements.into_iter().rev() {
        rewritten.replace_range(start..end, &text);
    }
    let out = dest.join(format!("{}.kdl", nonce()?));
    let expected = text(&rewritten, 0o600);
    write(&out, &expected)?;
    files.push(OwnedFile {
        path: out.clone(),
        original: Snapshot::Absent,
        expected,
        owner: "configuration snapshot".into(),
    });
    stack.remove(&real);
    Ok(out)
}
pub fn compose(
    dir: &Path,
    cfg: &Config,
    shell: Option<&crate::model::Candidate>,
) -> Result<(Snapshot, Vec<OwnedFile>)> {
    verify(&cfg.user_files)?;
    let mut deps = cfg.user_files.clone();
    let mut source = String::from(
        "// Shellswitch owns this entrypoint. Edit your user baseline, then use config-update explicitly.\n",
    );
    let fragment = shell
        .and_then(|c| c.lifecycle.as_ref())
        .and_then(|l| l.niri_fragment.as_ref());
    if let Some(path) = fragment {
        let l = shell.unwrap().lifecycle.as_ref().unwrap();
        verify(&l.frozen)?;
        deps.extend(l.frozen.clone());
        let shell_sections = sections(path, &mut BTreeSet::new())?;
        ensure!(
            !shell_sections.contains("spawn-at-startup")
                && !shell_sections.contains("spawn-sh-at-startup"),
            "Shell fragments must not own autostart; use the Shellswitch resume path"
        );
        if cfg.policy == Policy::AbortConflicts {
            let user_sections = sections(&cfg.user_root, &mut BTreeSet::new())?;
            let conflicts: Vec<_> = shell_sections.intersection(&user_sections).collect();
            ensure!(
                conflicts.is_empty(),
                "Config sections conflict: {conflicts:?}. Choose preserve-user or shell-wins explicitly; no bindings were replaced"
            );
        }
        if cfg.policy == Policy::ShellWins {
            source.push_str(&format!(
                "include {}\ninclude {}\n",
                serde_json::to_string(&cfg.user_root)?,
                serde_json::to_string(path)?
            ));
        } else {
            source.push_str(&format!(
                "include {}\ninclude {}\n",
                serde_json::to_string(path)?,
                serde_json::to_string(&cfg.user_root)?
            ));
        }
    } else {
        source.push_str(&format!(
            "include {}\n",
            serde_json::to_string(&cfg.user_root)?
        ));
    }
    source.push_str(&format!(
        "spawn-at-startup {} \"--state-dir\" {} \"resume\"\n",
        serde_json::to_string(&std::env::current_exe()?)?,
        serde_json::to_string(dir)?
    ));
    Ok((text(&source, 0o600), deps))
}
pub fn validate(path: &Path) -> Result<()> {
    let mut cmd = std::process::Command::new("niri");
    cmd.args(["validate", "-c"]).arg(path);
    crate::control::bounded(cmd).context("Niri validation failed")?;
    Ok(())
}
pub fn validate_snapshot(dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let path = dir.join("validation").join(format!("{}.kdl", nonce()?));
    write(&path, snapshot)?;
    let result = validate(&path);
    let _ = fs::remove_file(path);
    result
}
pub fn enroll(
    dir: &Path,
    target: &Path,
    user: &Path,
    policy: Policy,
    live_reload: bool,
) -> Result<()> {
    let store = crate::control::Store::open(dir)?;
    store.mutation_ready()?;
    let mut s = store.load()?;
    ensure!(
        s.pending.is_none(),
        "Recover or finish the pending transaction first"
    );
    ensure!(
        s.config.is_none(),
        "Configuration already enrolled; use repair or config-update"
    );
    let target = absolute(target)?;
    let current = read(&target)?;
    bytes(&current)?;
    let (root, files) = freeze(user, &dir.join("user-config").join(nonce()?))?;
    validate(&root)?;
    s.config = Some(Config {
        target,
        live_reload,
        original: current.clone(),
        expected: current,
        user_root: root,
        user_files: files,
        policy,
        owner: s.selected.clone(),
        dependencies: vec![],
    });
    s.last_event =
        "Niri config enrolled and snapshotted; no entrypoint or shell was changed".into();
    store.save(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn freezes_nested_include_and_ignores_comment() {
        let t = tempfile::tempdir().unwrap();
        fs::write(
            t.path().join("a.kdl"),
            "// include \"missing\"\ninclude \"b.kdl\"\nbinds { Mod+T { spawn \"foot\"; }; }\n",
        )
        .unwrap();
        fs::write(t.path().join("b.kdl"), "layout { gaps 3; }\n").unwrap();
        let (root, files) = freeze(&t.path().join("a.kdl"), &t.path().join("freeze")).unwrap();
        assert_eq!(files.len(), 2);
        fs::write(t.path().join("b.kdl"), "invalid").unwrap();
        verify(&files).unwrap();
        assert!(
            sections(&root, &mut BTreeSet::new())
                .unwrap()
                .contains("layout")
        );
    }
    #[test]
    fn rejects_include_cycle() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a"), "include \"a\"\n").unwrap();
        assert!(freeze(&t.path().join("a"), &t.path().join("f")).is_err());
    }
    #[test]
    fn snapshots_restore_symlink_without_writing_referent() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        let p = t.path().join("link");
        fs::write(&real, "original").unwrap();
        std::os::unix::fs::symlink(&real, &p).unwrap();
        let original = read(&p).unwrap();
        write(&p, &text("gate", 0o700)).unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), "original");
        write(&p, &original).unwrap();
        assert!(p.is_symlink());
    }
}
