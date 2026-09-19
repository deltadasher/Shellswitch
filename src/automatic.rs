//! Automatic adapters for a reviewed launcher structure, not brand names.
use crate::{
    control::Store,
    model::{Backend, Candidate},
    ownership::{CommandGate, Lifecycle, ProcessMatch},
};
use anyhow::Result;
use std::{fs, path::Path};
const RESPAWN: &str = "if ! pgrep -f \"quickshell.*Shell.qml\" >/dev/null; then\n    quickshell -p \"$SHELL_QML_PATH\" >/dev/null 2>&1 &\n    disown\nfi";
fn body(source: &str, entry: &Path) -> Option<String> {
    if source.matches(RESPAWN).count() != 1 {
        return None;
    }
    let expanded = source.replace("$HOME", &std::env::var("HOME").ok()?);
    if !expanded.contains(&format!("SCRIPTS_DIR=\"{}\"", entry.parent()?.display()))
        || !source.contains("SHELL_QML_PATH=\"$SCRIPTS_DIR/Shell.qml\"")
    {
        return None;
    }
    let body = source.replacen(RESPAWN, "# Startup is coordinated by Shellswitch.", 1);
    if body
        .lines()
        .any(|l| l.contains("quickshell -p") && !l.contains(" ipc "))
    {
        return None;
    }
    Some(body)
}
fn detect(c: &Candidate) -> Option<Candidate> {
    if c.lifecycle.is_some() || c.framework != "Quickshell" {
        return None;
    }
    let Backend::Process { argv, .. } = &c.backend else {
        return None;
    };
    let entry = c.source.canonicalize().ok()?;
    let mut gates = vec![];
    for item in fs::read_dir(entry.parent()?.parent()?)
        .ok()?
        .flatten()
        .take(256)
    {
        let path = item.path();
        if path.extension().is_none_or(|e| e != "sh") || fs::metadata(&path).ok()?.len() > 262144 {
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(body) = body(&source, &entry) {
            gates.push(CommandGate {
                path: path.clone(),
                inline_body: Some(body),
                inline_original: Some(source),
                native_argv: vec!["/bin/bash".into(), path.to_string_lossy().into_owned()],
                blocked_prefixes: vec![],
                routes: vec![],
            });
        }
    }
    if gates.is_empty() {
        return None;
    }
    let mut result = c.clone();
    result.lifecycle = Some(Lifecycle {
        adapter: "quickshell-ipc-launcher-v1".into(),
        commands: gates,
        processes: vec![ProcessMatch {
            argv_prefix: argv.clone(),
            kill_without_cleanup: false,
        }],
        ..Default::default()
    });
    Some(result)
}
pub fn register(dir: &Path, candidates: &[Candidate]) -> Result<()> {
    let store = Store::open(dir)?;
    let mut state = store.load()?;
    if state.pending.is_some() {
        return Ok(());
    }
    let mut changed = false;
    for c in candidates {
        if state
            .installed
            .get(&c.id)
            .is_some_and(|c| c.lifecycle.is_some())
        {
            continue;
        }
        if let Some(managed) = detect(c) {
            if let Some(active) = &mut state.active
                && active.candidate.id == managed.id
            {
                active.candidate.lifecycle = managed.lifecycle.clone();
            }
            state.installed.insert(managed.id.clone(), managed);
            changed = true;
        }
    }
    if changed {
        store.save(&state)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_structure_and_rejects_unknown_respawn() {
        let source = format!(
            "SCRIPTS_DIR=\"/arbitrary/quickshell\"\nSHELL_QML_PATH=\"$SCRIPTS_DIR/Shell.qml\"\n{RESPAWN}"
        );
        let path = Path::new("/arbitrary/quickshell/Shell.qml");
        assert!(!body(&source, path).unwrap().contains("disown"));
        assert!(body(&source.replace("disown", "setsid unknown"), path).is_none());
        assert!(body(&source, Path::new("/other/Shell.qml")).is_none());
    }
}
