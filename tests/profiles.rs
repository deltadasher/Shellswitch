use serde_json::{Value, json};
use std::{fs, process::Command};
#[test]
fn compositor_choices_persist_independently_and_stale_pid_does_not_abort_scan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (id, comp) in [("first", "niri"), ("second", "hyprland")] {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("shellswitch.toml"),
            format!("id = \"{id}\"\nname = \"{id}\"\nargv = [\"/bin/sleep\",\"98374.127\"]\ncompositors = [\"{comp}\"]\n")).unwrap();
    }
    let state = root.join("state");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_shellswitch"))
            .arg("--isolated")
            .arg("--root")
            .arg(root)
            .arg("--state-dir")
            .arg(&state)
            .args(args)
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CURRENT_DESKTOP", "sway")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .env_remove("NIRI_SOCKET")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("SWAYSOCK")
            .output()
            .unwrap()
    };
    for (comp, id) in [("niri", "first"), ("hyprland", "second")] {
        let result = run(&["autostart", "--compositor", comp, "--shell", id]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let path = state.join("state.json");
    let mut s: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(s["autostart"]["niri"], "first");
    assert_eq!(s["autostart"]["hyprland"], "second");
    assert!(s["active"].is_null());
    assert!(s["selected"].is_null());
    assert!(
        root.join("config/autostart/shellswitch-session.desktop")
            .is_file()
    );
    s["installed"]["first"]["running_pids"] = json!([4294967294u32]);
    fs::write(&path, serde_json::to_vec(&s).unwrap()).unwrap();
    let result = run(&["scan", "--json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(run(&["autostart"]).status.success()); // no sway profile: no launch
    assert!(
        run(&["autostart", "--compositor", "niri", "--clear"])
            .status
            .success()
    );
    let s: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(s["autostart"].get("niri").is_none());
    assert_eq!(s["autostart"]["hyprland"], "second");
    assert!(s["active"].is_null());
}
