//! Opt-in native CLI delegation. Native programs remain responsible for their
//! command vocabulary; lifecycle operations stay with the transaction manager.
use crate::{
    model::{Backend, Candidate},
    ownership::{self, CommandGate},
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn runtime_root(c: &Candidate) -> Option<PathBuf> {
    let Backend::Process { argv, .. } = &c.backend else {
        return None;
    };
    let entry = argv.windows(2).find(|w| w[0] == "-p")?;
    let p = PathBuf::from(&entry[1]);
    let qs = if p.is_file() {
        p.parent()?.to_owned()
    } else {
        p
    };
    qs.parent()?.parent().map(Path::to_owned)
}

pub fn command(
    c: &Candidate,
    endpoint: &CommandGate,
    args: &[String],
) -> Result<Option<Vec<String>>> {
    for prefix in &endpoint.blocked_prefixes {
        ensure!(
            !args.starts_with(prefix),
            "This command changes installation or ownership; use a staged update instead"
        );
    }
    if !endpoint.native_argv.is_empty() {
        let mut argv = endpoint.native_argv.clone();
        argv.extend_from_slice(args);
        return Ok(Some(argv));
    }
    let adapter = c
        .lifecycle
        .as_ref()
        .map(|l| l.adapter.as_str())
        .unwrap_or("");
    if !["tonantzintla-bridge-v1", "serpantinum-bridge-v1"].contains(&adapter) {
        return Ok(None);
    }
    let alias = endpoint
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .context("Invalid CLI endpoint")?;
    if alias == "serpantinumd" {
        return Ok(None);
    }
    let root = runtime_root(c).context("Cannot locate staged native CLI")?;
    let native = root
        .join("bin")
        .join(if adapter == "tonantzintla-bridge-v1" {
            "blackhole"
        } else {
            "serpantinum"
        });
    ensure!(
        native.is_file(),
        "Native CLI is missing: {}",
        native.display()
    );
    let first = args
        .iter()
        .find(|a| !["-v", "--verbose"].contains(&a.as_str()))
        .map(String::as_str)
        .unwrap_or("");
    ensure!(
        !["install", "update", "uninstall", "sync"].contains(&first),
        "Native install/update may overwrite active configuration; stage an update through Shellswitch"
    );
    if first == "niri" {
        ensure!(
            args.get(1)
                .is_none_or(|a| ["status", "inspect", "check"].contains(&a.as_str())),
            "Niri ownership changes must go through Shellswitch"
        );
    }
    // Preserve upstream parsing and all ordinary commands. Patch only its
    // daemon guard in a sibling copy, retaining the expected bin/../src layout.
    let executable = if adapter == "serpantinum-bridge-v1" {
        let source = fs::read_to_string(&native)?;
        let patched = cooperative_serpantinum(&source)?;
        let target = native.with_file_name(".shellswitch-serpantinum");
        ownership::write(&target, &ownership::text(&patched, 0o700))?;
        target
    } else {
        native
    };
    let mut argv = vec![executable.to_string_lossy().into_owned()];
    argv.extend_from_slice(args);
    Ok(Some(argv))
}

fn cooperative_serpantinum(source: &str) -> Result<String> {
    let marker = "ensure_daemon() {";
    ensure!(
        source.matches(marker).count() == 1,
        "Serpantinum daemon hook changed; refusing unreviewed daemon startup"
    );
    Ok(source.replacen(marker, r#"ensure_daemon() {
    if [ "${SHELLSWITCH_GATED:-}" = "1" ]; then
        "${SHELLSWITCH_EXECUTABLE:?}" --state-dir "${SHELLSWITCH_STATE_DIR:?}" authorize "${SHELLSWITCH_SHELL_ID:?}" --purpose runtime || exit $?
        return 0
    fi
"#, 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cooperative_guard_skips_daemon_only_after_authorization() {
        let source = "ensure_daemon() {\n echo native-start\n}\nensure_daemon\necho control\n";
        let script = cooperative_serpantinum(source).unwrap();
        for (authorizer, success) in [("/bin/true", true), ("/bin/false", false)] {
            let result = std::process::Command::new("/bin/bash")
                .args(["-c", &script])
                .env("SHELLSWITCH_GATED", "1")
                .env("SHELLSWITCH_EXECUTABLE", authorizer)
                .env("SHELLSWITCH_STATE_DIR", "/fixture")
                .env("SHELLSWITCH_SHELL_ID", "arbitrary")
                .output()
                .unwrap();
            assert_eq!(result.status.success(), success);
            assert!(!String::from_utf8_lossy(&result.stdout).contains("native-start"));
            assert_eq!(
                String::from_utf8_lossy(&result.stdout).contains("control"),
                success
            );
        }
    }
    #[test]
    fn hook_preserves_native_body_and_requires_known_guard() {
        let original = "ensure_daemon() {\n echo native-start\n}\nensure_daemon\n";
        let result = cooperative_serpantinum(original).unwrap();
        assert!(result.contains("authorize"));
        assert!(result.contains("echo native-start"));
        assert!(cooperative_serpantinum("new_launcher() {}").is_err());
        assert!(cooperative_serpantinum("ensure_daemon() {}\nensure_daemon() {}").is_err());
    }
}
