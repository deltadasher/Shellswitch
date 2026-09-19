//! Reviewed bridges for the incident shells. Discovery remains framework-based;
//! these adapters describe lifecycle semantics that static detection cannot infer.
use crate::ownership::{CommandGate, Lifecycle, ProcessMatch, Route};
use anyhow::{Context, Result, ensure};
use std::path::Path;

pub fn generate(kind: &str, root: &Path, fragment: Option<&Path>) -> Result<String> {
    let root = root.canonicalize()?;
    let home = std::env::var_os("HOME").context("HOME is required")?;
    let bin = std::path::PathBuf::from(home).join(".local/bin");
    let qs = crate::model::executable("quickshell")
        .or_else(|| crate::model::executable("qs"))
        .context("Install Quickshell before generating an adapter")?;
    let qs = qs.canonicalize()?.to_string_lossy().into_owned();
    let bash = crate::model::executable("bash")
        .context("bash missing")?
        .canonicalize()?
        .to_string_lossy()
        .into_owned();
    let python = crate::model::executable("python3")
        .context("python3 missing")?
        .canonicalize()?
        .to_string_lossy()
        .into_owned();
    let (name, entry, aliases) = match kind {
        "tonantzintla" => (
            "Tonantzintla",
            root.join("src/quickshell"),
            vec!["blackhole", "astralithctl"],
        ),
        "serpantinum" => (
            "Serpantinum",
            root.join("src/quickshell/Shell.qml"),
            vec!["serpantinum", "serpantinumd"],
        ),
        _ => anyhow::bail!(
            "No reviewed bridge for {kind}; use a lifecycle manifest for other shells"
        ),
    };
    ensure!(
        entry.exists(),
        "Expected shell entrypoint is absent: {}",
        entry.display()
    );
    let entry = entry.to_string_lossy().into_owned();
    let argv = vec![qs.clone(), "-p".into(), entry.clone()];
    let ipc = |tail: &[&str]| {
        let mut a = argv.clone();
        a.extend(["ipc".into(), "call".into()]);
        a.extend(tail.iter().map(|s| s.to_string()));
        a
    };
    let routes = if kind == "tonantzintla" {
        [
            ("toggle", vec!["ephemeris", "toggle"]),
            ("open", vec!["ephemeris", "open"]),
            ("close", vec!["ephemeris", "close"]),
            ("bar-edit", vec!["aperture", "edit"]),
            ("island-settings", vec!["aperture", "openIsland"]),
            ("close-island", vec!["aperture", "closeIsland"]),
        ]
        .into_iter()
        .map(|(cmd, args)| Route {
            prefix: vec![cmd.into()],
            argv: ipc(&args),
        })
        .collect::<Vec<_>>()
    } else {
        // Only widget commands; workspace routing is compositor-specific and is
        // deliberately not guessed. Crucially, never call ensure_daemon.
        ["toggle", "open", "close"]
            .into_iter()
            .map(|cmd| Route {
                prefix: vec!["msg".into(), cmd.into()],
                argv: ipc(&["main", "handleCommand", cmd]),
            })
            .collect()
    };
    let mut processes = vec![ProcessMatch {
        argv_prefix: argv.clone(),
        kill_without_cleanup: false,
    }];
    if kind == "tonantzintla" {
        processes.push(ProcessMatch {
            argv_prefix: vec![
                python,
                root.join("src/libexec/session-daemon.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            kill_without_cleanup: false,
        });
        processes.push(ProcessMatch {
            argv_prefix: vec![qs.clone(), "-n".into(), "-p".into(), entry.clone()],
            kill_without_cleanup: false,
        });
    } else {
        processes.push(ProcessMatch {
            argv_prefix: vec![
                bash,
                root.join("bin/serpantinumd").to_string_lossy().into_owned(),
            ],
            kill_without_cleanup: true,
        });
        // No generic focus_daemon matcher: only this shell's exact helper path.
        processes.push(ProcessMatch {
            argv_prefix: vec![
                python,
                root.join("src/quickshell/guide/wellbeing/focus_daemon.py")
                    .to_string_lossy()
                    .into_owned(),
            ],
            kill_without_cleanup: false,
        });
    }
    let lifecycle = Lifecycle {
        adapter: format!("{kind}-bridge-v1"),
        niri_fragment: fragment.map(|p| p.canonicalize()).transpose()?,
        commands: aliases
            .into_iter()
            .map(|alias| CommandGate {
                path: bin.join(alias),
                inline_body: None,
                inline_original: None,
                native_argv: vec![],
                blocked_prefixes: vec![],
                routes: if alias == "serpantinumd" {
                    vec![]
                } else {
                    routes.clone()
                },
            })
            .collect(),
        processes,
        verify_argv: vec![qs, "-p".into(), entry, "list".into()],
        ..Default::default()
    };
    let value = serde_json::json!({"id":kind,"name":name,"protocols":["wayland"],"compositors":["niri"],"argv":argv});
    let mut value: toml::Value = serde_json::from_value(value)?;
    value
        .as_table_mut()
        .unwrap()
        .insert("lifecycle".into(), toml::Value::try_from(lifecycle)?);
    Ok(format!(
        "# Review inventory before protect/switch. Add discovered dedicated services and autostarts.\n# Native Tonantzintla contract patch is supplied separately; bridge IPC bypasses upstream CLI.\n# No shell config fragment is guessed. Use --fragment for reviewed shell-only KDL.\n{}",
        toml::to_string_pretty(&value)?
    ))
}

/// Restore the runtime paths normally supplied by the upstream launcher,
/// without invoking its daemon or startup side effects.
pub fn launch_environment(
    c: &crate::model::Candidate,
) -> Result<Vec<(&'static str, std::path::PathBuf)>> {
    if !c
        .lifecycle
        .as_ref()
        .is_some_and(|l| l.adapter == "serpantinum-bridge-v1")
    {
        return Ok(Vec::new());
    }
    let crate::model::Backend::Process { argv, .. } = &c.backend else {
        anyhow::bail!("Serpantinum bridge requires a process backend");
    };
    let entry = argv
        .windows(2)
        .find(|w| w[0] == "-p")
        .map(|w| std::path::PathBuf::from(&w[1]))
        .context("Missing Serpantinum entrypoint")?;
    let qs = entry.parent().context("Missing Quickshell directory")?;
    let src = qs
        .parent()
        .context("Missing Serpantinum runtime directory")?;
    ensure!(
        src.join("scripts/qs_manager.sh").is_file(),
        "Missing Serpantinum widget dispatcher"
    );
    Ok(vec![
        ("SERPANTINUM_DIR", src.to_owned()),
        ("QS_DIR", qs.to_owned()),
        ("MAIN_QML", entry),
    ])
}
