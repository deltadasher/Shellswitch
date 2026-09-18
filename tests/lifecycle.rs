use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};
fn call(root: &Path, state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_shellswitch"))
        .args(["--isolated", "--root"])
        .arg(root)
        .arg("--state-dir")
        .arg(state)
        .args(args)
        .env("WAYLAND_DISPLAY", "shellswitch-test-no-display")
        .env("XDG_CURRENT_DESKTOP", "testwm")
        .output()
        .unwrap()
}
fn ok(root: &Path, state: &Path, args: &[&str]) -> Output {
    let o = call(root, state, args);
    assert!(
        o.status.success(),
        "{:?}\n{}\n{}",
        args,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    o
}
fn state(root: &Path, dir: &Path) -> Value {
    serde_json::from_slice(&ok(root, dir, &["status"]).stdout).unwrap()
}
fn fixture(root: &Path, name: &str, argv: &str) {
    let dir = root.join(name);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("shellswitch.toml"),
        format!("name={name:?}\nargv={argv}\nprotocols=['wayland']\n"),
    )
    .unwrap();
}
#[test]
fn switching_commit_failure_rollback_watchdog_and_stop() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("shells");
    let journal = t.path().join("state");
    fs::create_dir(&root).unwrap();
    fixture(&root, "alpha", "['/usr/bin/sleep', '301']");
    fixture(&root, "beta", "['/usr/bin/sleep', '302']");
    fixture(&root, "broken", "['/usr/bin/false']");
    // No side effects without explicit execution approval.
    assert!(!call(&root, &journal, &["switch", "alpha"]).status.success());
    ok(&root, &journal, &["switch", "alpha", "--yes"]);
    ok(&root, &journal, &["keep"]);
    let first = state(&root, &journal);
    assert_eq!(first["active"]["candidate"]["name"], "alpha");
    assert!(first["pending"].is_null());
    ok(&root, &journal, &["switch", "beta", "--yes"]);
    ok(&root, &journal, &["revert"]);
    let restored = state(&root, &journal);
    assert_eq!(restored["active"]["candidate"]["name"], "alpha");
    assert_ne!(
        first["active"]["process"]["pid"],
        restored["active"]["process"]["pid"]
    );
    let failure = call(&root, &journal, &["switch", "broken", "--yes"]);
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("previous shell restored"));
    assert_eq!(
        state(&root, &journal)["active"]["candidate"]["name"],
        "alpha"
    );
    ok(&root, &journal, &["switch", "beta", "--yes"]);
    let end = Instant::now() + Duration::from_secs(25);
    loop {
        let s = state(&root, &journal);
        if s["pending"].is_null() {
            assert_eq!(s["active"]["candidate"]["name"], "alpha");
            break;
        }
        assert!(
            Instant::now() < end,
            "watchdog did not restore previous shell"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    ok(&root, &journal, &["stop", "--yes"]);
    assert!(state(&root, &journal)["active"].is_null());
}
#[test]
fn compatibility_blocks_foreign_compositor_and_sessions() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("shells");
    let state = t.path().join("state");
    fs::create_dir(&root).unwrap();
    fixture(&root, "wm-specific", "['/usr/bin/sleep', '303']");
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(root.join("wm-specific/shellswitch.toml"))
        .unwrap();
    writeln!(f, "compositors=['impossible-wm']").unwrap();
    assert!(
        !call(&root, &state, &["switch", "wm-specific", "--yes"])
            .status
            .success()
    );
    fs::create_dir(root.join("wayland-sessions")).unwrap();
    fs::write(
        root.join("wayland-sessions/session.desktop"),
        "[Desktop Entry]\nName=Full Session\nExec=niri\n",
    )
    .unwrap();
    let result = call(&root, &state, &["plan", "Full Session"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("login manager"));
}

#[test]
fn foreground_launcher_children_are_cleaned_on_early_exit() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("shells");
    let journal = t.path().join("state");
    fs::create_dir(&root).unwrap();
    let pidfile = t.path().join("child.pid");
    let script = format!("sleep 305 & echo $! > '{}'", pidfile.display());
    fixture(
        &root,
        "forking",
        &format!(
            "['/bin/sh', '-c', {}]",
            serde_json::to_string(&script).unwrap()
        ),
    );
    let output = call(&root, &journal, &["switch", "forking", "--yes"]);
    assert!(!output.status.success());
    let pid = fs::read_to_string(pidfile).unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid.trim()));
    assert!(
        stat.is_err() || stat.unwrap().split(") ").nth(1).unwrap().starts_with('Z'),
        "background child survived rejected launcher"
    );
}

#[test]
fn systemd_backend_uses_user_scope_and_restores_services() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("shells");
    let journal = t.path().join("state");
    let bin = t.path().join("bin");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&bin).unwrap();
    let mock = bin.join("systemctl");
    fs::write(&mock,r#"#!/bin/sh
[ "$1" = '--user' ] || exit 98
case "$2" in
show-environment) env ;;
show) printf 'ExecStart={ path=/usr/bin/example-shell ; argv[]=/usr/bin/example-shell ; }\nPropagatesStopTo=\nConflicts=\n' ;;
start) touch "$MOCK_DIR/$3" ;;
stop) rm -f "$MOCK_DIR/$3" ;;
is-active) test -f "$MOCK_DIR/$3" ;;
*) exit 99 ;;
esac
"#).unwrap();
    fs::set_permissions(&mock, fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["alpha-service", "beta-service"] {
        let p = root.join(name);
        fs::create_dir(&p).unwrap();
        fs::write(
            p.join("shellswitch.toml"),
            format!("name={name:?}\nsystemd_unit='{name}.service'\nprotocols=['wayland']\n"),
        )
        .unwrap();
    }
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_shellswitch"))
            .args(["--isolated", "--root"])
            .arg(&root)
            .arg("--state-dir")
            .arg(&journal)
            .args(args)
            .env("WAYLAND_DISPLAY", "test-display")
            .env("MOCK_DIR", t.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    invoke(&["switch", "alpha-service", "--yes"]);
    invoke(&["keep"]);
    assert!(t.path().join("alpha-service.service").exists());
    invoke(&["switch", "beta-service", "--yes"]);
    assert!(!t.path().join("alpha-service.service").exists());
    assert!(t.path().join("beta-service.service").exists());
    invoke(&["revert"]);
    assert!(t.path().join("alpha-service.service").exists());
    assert!(!t.path().join("beta-service.service").exists());
    invoke(&["stop", "--yes"]);
}
