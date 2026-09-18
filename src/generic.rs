//! Name-independent evidence providers. These deliberately yield review candidates:
//! a layer-shell client could be a panel, wallpaper, notification daemon or lockscreen.
use crate::{discovery, model::*, process};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
const API_MARKERS: &[&str] = &[
    "gtk_layer_init_for_window",
    "gtk_layer_shell",
    "GtkLayerShell",
    "Gtk4LayerShell",
    "gtk4-layer-shell",
    "LayerShellQt",
    "zwlr_layer_shell_v1",
    "_NET_WM_WINDOW_TYPE_DOCK",
    "_NET_WM_WINDOW_TYPE_DESKTOP",
];
fn markers(data: &[u8]) -> Vec<String> {
    API_MARKERS
        .iter()
        .filter(|s| memchr::memmem::find(data, s.as_bytes()).is_some())
        .map(|s| s.to_string())
        .collect()
}
fn project(path: &Path) -> PathBuf {
    for p in path.ancestors().skip(1).take(5) {
        for name in [
            "Cargo.toml",
            "package.json",
            "meson.build",
            "pyproject.toml",
        ] {
            if p.join(name).is_file() {
                return p.join(name);
            }
        }
    }
    path.to_path_buf()
}
pub fn augment(files: &[PathBuf], candidates: &mut Vec<Candidate>, warnings: &mut Vec<String>) {
    let established: Vec<_> = candidates
        .iter()
        .filter(|c| c.kind == Kind::Shell || c.declared)
        .filter_map(|c| c.source.parent().map(Path::to_path_buf))
        .collect();
    let mut findings: HashMap<PathBuf, Candidate> = HashMap::new();
    let mut budget = 512 * 1024 * 1024usize;
    for path in files {
        if established.iter().any(|p| path.starts_with(p))
            || candidates.iter().any(|c| c.source == *path)
        {
            continue;
        }
        let Ok(meta) = fs::metadata(path) else {
            continue;
        };
        let exec = meta.permissions().mode() & 0o111 != 0;
        let source = path.extension().is_some_and(|e| {
            [
                "py", "rs", "c", "cpp", "h", "js", "ts", "tsx", "qml", "sh", "vala", "yuck",
            ]
            .iter()
            .any(|s| e == *s)
        });
        if !source && !exec {
            continue;
        }
        let size = (meta.len() as usize).min(if exec { 128 * 1024 } else { 512 * 1024 });
        if size > budget {
            continue;
        }
        budget -= size;
        let mut data = Vec::with_capacity(size);
        if fs::File::open(path)
            .and_then(|f| f.take(size as u64).read_to_end(&mut data))
            .is_err()
        {
            continue;
        }
        let evidence = markers(&data);
        let script = data.starts_with(b"#!")
            && ["quickshell", "ags run", "eww open", "exec qs "]
                .iter()
                .any(|s| memchr::memmem::find(&data, s.as_bytes()).is_some());
        if evidence.is_empty() && !script {
            continue;
        }
        let binary = data.starts_with(b"\x7fELF");
        let key = if binary || script {
            path.clone()
        } else {
            project(path)
        };
        let c = findings.entry(key.clone()).or_insert_with(|| {
            discovery::base(
                &key,
                key.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                if binary {
                    "native binary"
                } else if script {
                    "launcher script"
                } else {
                    "shell API source"
                },
                Kind::Candidate,
            )
        });
        for item in evidence {
            if item.contains("_NET_WM_") {
                c.protocols.insert("x11".into());
            } else {
                c.protocols.insert("wayland".into());
            }
            if c.evidence.len() < 16 {
                c.evidence
                    .push(format!("{} references {}", path.display(), item));
            }
        }
        if script {
            c.evidence
                .push("Shell runtime referenced by executable script (not executed)".into());
            c.evidence.extend(
                String::from_utf8_lossy(&data)
                    .lines()
                    .filter(|l| {
                        ["exec ", "quickshell", "ags run", "eww open"]
                            .iter()
                            .any(|s| l.contains(s))
                    })
                    .take(5)
                    .map(|l| l.chars().take(200).collect()),
            );
        }
        if binary {
            c.evidence.push("ELF byte evidence only; a panel/lockscreen/wallpaper is not necessarily a complete shell".into());
        }
    }
    if budget < 4 * 1024 * 1024 {
        warnings.push("Generic evidence scan approached its 512 MiB budget; use narrower --root paths for full coverage".into());
    }
    candidates.extend(findings.into_values());
    for p in process::all() {
        if p.identity.pid == std::process::id() || !process::same_session(p.identity.pid) {
            continue;
        }
        let maps = fs::read_to_string(format!("/proc/{}/maps", p.identity.pid)).unwrap_or_default();
        if ![
            "libgtk-layer-shell",
            "libgtk4-layer-shell",
            "libLayerShellQt",
            "libastal",
        ]
        .iter()
        .any(|s| maps.contains(s))
        {
            continue;
        }
        if candidates.iter().any(|c| process::matches(c, &p)) {
            continue;
        }
        let source = fs::read_link(format!("/proc/{}/exe", p.identity.pid))
            .unwrap_or_else(|_| p.cwd.clone());
        if let Some(c) = candidates.iter_mut().find(|c| c.source == source) {
            c.running_pids.push(p.identity.pid);
            c.evidence
                .push("Running process maps a shell/layer library".into());
            continue;
        }
        let mut c = discovery::base(
            &source,
            source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            "runtime library evidence",
            Kind::Candidate,
        );
        c.evidence.push(format!(
            "PID {} maps a shell/layer library; argv {:?}",
            p.identity.pid, p.argv
        ));
        c.running_pids.push(p.identity.pid);
        candidates.push(c);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_project_is_detected_by_api() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("unknown.py");
        fs::write(
            &p,
            "from gi.repository import GtkLayerShell\nGtkLayerShell.init_for_window(win)",
        )
        .unwrap();
        let mut c = vec![];
        augment(&[p], &mut c, &mut vec![]);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, Kind::Candidate);
        assert!(c[0].protocols.contains("wayland"));
    }
    #[test]
    fn ordinary_gtk_app_is_not_detected() {
        assert!(markers(b"Gtk.ApplicationWindow").is_empty());
    }
}
