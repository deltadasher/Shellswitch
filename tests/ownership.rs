//! Everything is rooted in temporary HOME/XDG dirs. No live desktop operations.
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    cfg: PathBuf,
    source: PathBuf,
}
impl Fixture {
    fn new(policy: &str) -> Self {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        let state = root.join("state");
        let cfg = root.join("config/niri/config.kdl");
        let source = root.join("source");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(&cfg,"// custom original\ninput { keyboard { repeat-delay 411; }; }\nbinds { Mod+Return { spawn \"terminology\"; }; }\n").unwrap();
        let f = Self {
            _temp: t,
            root,
            state,
            cfg,
            source,
        };
        f.ok(&[
            "enroll",
            "--config",
            f.cfg.to_str().unwrap(),
            "--user-config",
            f.cfg.to_str().unwrap(),
            "--policy",
            policy,
            "--offline",
            "--yes",
        ]);
        f
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_shellswitch"));
        cmd.args(["--state-dir"])
            .arg(&self.state)
            .arg("--isolated")
            .arg("--root")
            .arg(&self.source)
            .args(args)
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("xdg-state"))
            .env("XDG_RUNTIME_DIR", self.root.join("runtime"))
            .env("WAYLAND_DISPLAY", "fixture-wayland")
            .env("XDG_CURRENT_DESKTOP", "niri")
            .env_remove("NIRI_SOCKET")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("SWAYSOCK");
        if self.root.join("mockbin").exists() {
            cmd.env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("mockbin").display()),
            );
        }
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> Output {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        o
    }
    fn fail(&self, args: &[&str], contains: &str) {
        let o = self.run(args);
        assert!(!o.status.success(), "unexpected success {args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(contains),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&self.ok(&["status"]).stdout).unwrap()
    }
    fn add(&self, id: &str, fragment: &str, verify: Option<&str>) {
        let p = self.source.join(id);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("shell.kdl"), fragment).unwrap();
        let ctl = p.join("control.sh");
        fs::write(&ctl, "#!/bin/sh\nprintf 'command reached\\n'\n").unwrap();
        fs::set_permissions(&ctl, fs::Permissions::from_mode(0o755)).unwrap();
        let gate = self.root.join("bin").join(id);
        fs::create_dir_all(gate.parent().unwrap()).unwrap();
        fs::write(&gate, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&gate, fs::Permissions::from_mode(0o755)).unwrap();
        let autostart = self
            .root
            .join("config/autostart")
            .join(format!("{id}.desktop"));
        fs::create_dir_all(autostart.parent().unwrap()).unwrap();
        fs::write(
            &autostart,
            format!(
                "[Desktop Entry]\nType=Application\nExec={} start\n",
                gate.display()
            ),
        )
        .unwrap();
        let seconds = match id {
            "alpha" => "881",
            "beta" => "882",
            _ => "883",
        };
        let verify = verify
            .map(|s| format!("verify_argv=[{s:?}]\n"))
            .unwrap_or_default();
        fs::write(p.join("shellswitch.toml"),format!("id={id:?}\nname={id:?}\nargv=['/usr/bin/sleep',{seconds:?}]\nprotocols=['wayland']\n[lifecycle]\nadapter='fixture'\nniri_fragment='shell.kdl'\nautostart=[{autostart:?}]\n{verify}\n[[lifecycle.commands]]\npath={gate:?}\n[[lifecycle.commands.routes]]\nprefix=['msg']\nargv=[{ctl:?}]\n[[lifecycle.processes]]\nargv_prefix=['/usr/bin/sleep',{seconds:?}]\n",autostart=autostart.to_str().unwrap(),gate=gate.to_str().unwrap(),ctl=ctl.to_str().unwrap())).unwrap();
        self.ok(&[
            "install",
            p.join("shellswitch.toml").to_str().unwrap(),
            "--payload",
            p.to_str().unwrap(),
            "--yes",
        ]);
    }
    fn activate(&self, id: &str) {
        self.ok(&["switch", id, "--yes"]);
        self.ok(&["keep"]);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.run(&["recover", "--yes"]);
        for id in ["alpha", "beta", "broken"] {
            let _ = self.run(&["disable", id, "--yes"]);
        }
    }
}
#[test]
fn competing_shells_stale_shortcuts_drift_emergency_and_release() {
    let f = Fixture::new("preserve-user");
    f.add(
        "alpha",
        "binds { Mod+Return { spawn \"kitty\"; }; }\n",
        None,
    );
    f.add("beta", "layout { gaps 7; }\n", None);
    let original = fs::read(&f.cfg).unwrap();
    let install_state = f.state();
    assert!(install_state["selected"].is_null());
    assert_eq!(fs::read(&f.cfg).unwrap(), original);
    f.ok(&["protect", "--yes"]);
    assert_eq!(fs::read(&f.cfg).unwrap(), original);
    f.fail(&["gate", "beta", "--", "msg", "toggle"], "inactive");
    let stale = Command::new(f.root.join("bin/beta"))
        .arg("msg")
        .arg("toggle")
        .env("HOME", &f.root)
        .env("XDG_CONFIG_HOME", f.root.join("config"))
        .output()
        .unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("inactive"));
    assert!(
        fs::read_to_string(f.root.join("config/autostart/beta.desktop"))
            .unwrap()
            .contains("Hidden=true")
    );
    f.activate("alpha");
    let alpha_config = fs::read(&f.cfg).unwrap();
    assert_eq!(f.state()["selected"], "alpha");
    // User bindings occur in the final include under explicit preserve-user policy.
    let cfg = String::from_utf8(alpha_config.clone()).unwrap();
    let user_root = f.state()["config"]["user_root"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(cfg.rfind(&user_root).unwrap() > cfg.find("packages/").unwrap());
    assert!(
        fs::read_to_string(&user_root)
            .unwrap()
            .contains("terminology")
    );
    f.ok(&["gate", "alpha", "--", "msg", "toggle"]);
    f.activate("beta");
    f.fail(&["gate", "alpha", "--", "msg", "toggle"], "inactive");
    // A deliberately bypassing same-user process is detected, not called malware.
    let mut competing = Command::new("/usr/bin/sleep");
    competing
        .arg("881")
        .env("WAYLAND_DISPLAY", "fixture-wayland")
        .env("XDG_RUNTIME_DIR", f.root.join("runtime"));
    let mut other = competing.spawn().unwrap();
    let d = String::from_utf8(f.ok(&["doctor", "--json"]).stdout).unwrap();
    assert!(d.contains("inactive-running"));
    f.ok(&["disable", "alpha", "--yes"]);
    assert!(!other.wait().unwrap().success());
    f.fail(&["switch", "alpha", "--yes"], "emergency-disabled");
    f.ok(&["release", "alpha", "--yes"]);
    assert_eq!(f.state()["selected"], "beta");
    // External installer overwrites both root config and a supported CLI gate.
    fs::write(
        &f.cfg,
        "// foreign config\nbinds { Mod+X { spawn \"kitty\"; }; }\n",
    )
    .unwrap();
    fs::write(f.root.join("bin/alpha"), "#!/bin/sh\nexit 99\n").unwrap();
    f.fail(&["switch", "alpha", "--yes"], "drift");
    f.ok(&["repair", "--yes"]);
    assert!(fs::read_dir(f.state.join("recovery")).unwrap().count() >= 2);
    assert!(
        fs::read_to_string(f.root.join("bin/alpha"))
            .unwrap()
            .contains("gate")
    );
    f.activate("alpha");
    f.ok(&["disable", "alpha", "--yes"]);
    assert!(f.state()["selected"].is_null());
    f.fail(&["gate", "alpha", "--", "start"], "disabled");
}
#[test]
fn invalid_configs_conflicts_and_failed_health_leave_previous_working() {
    let f = Fixture::new("abort-conflicts");
    f.add("alpha", "layout { gaps 3; }\n", None);
    f.activate("alpha");
    let before = fs::read(&f.cfg).unwrap();
    f.add("beta", "binds { Mod+Return { spawn \"kitty\"; }; }\n", None);
    f.fail(&["switch", "beta", "--yes"], "conflict");
    assert_eq!(fs::read(&f.cfg).unwrap(), before);
    f.add("broken", "layout { gaps 12; }\n", Some("/usr/bin/false"));
    f.fail(&["switch", "broken", "--yes"], "previous shell restored");
    assert_eq!(f.state()["selected"], "alpha");
    assert_eq!(fs::read(&f.cfg).unwrap(), before);
    fs::write(
        f.source.join("beta/shell.kdl"),
        "layout { absolutely-invalid true; }\n",
    )
    .unwrap();
    f.fail(
        &[
            "install",
            f.source.join("beta/shellswitch.toml").to_str().unwrap(),
            "--yes",
        ],
        "validation",
    );
    assert_eq!(fs::read(&f.cfg).unwrap(), before);
}
#[test]
fn concurrent_switches_and_interrupted_handoff_recover() {
    let f = Fixture::new("preserve-user");
    f.add("alpha", "layout { gaps 2; }\n", None);
    f.add("beta", "layout { gaps 3; }\n", None);
    f.activate("alpha");
    let before = fs::read(&f.cfg).unwrap();
    let mut one = f
        .cmd(&["switch", "beta", "--yes"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let two = f.run(&["switch", "beta", "--yes"]);
    let first = one.wait().unwrap();
    assert!(
        first.success() ^ two.status.success(),
        "exactly one trial may own the switch lock"
    );
    f.ok(&["revert"]);
    assert_eq!(fs::read(&f.cfg).unwrap(), before);
    // Make readiness slow through a declared health command, then kill the parent
    // after it durably reaches Starting. The independent watchdog must recover.
    let mut manifest = fs::read_to_string(f.source.join("beta/shellswitch.toml")).unwrap();
    manifest = manifest.replace(
        "adapter='fixture'",
        "adapter='fixture'\nverify_argv=['/usr/bin/sleep','4']",
    );
    fs::write(f.source.join("beta/shellswitch.toml"), manifest).unwrap();
    f.ok(&[
        "install",
        f.source.join("beta/shellswitch.toml").to_str().unwrap(),
        "--yes",
    ]);
    let mut interrupted = f
        .cmd(&["switch", "beta", "--yes"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(data) = fs::read(f.state.join("state.json")) {
            let s: Value = serde_json::from_slice(&data).unwrap();
            if s["pending"]["phase"] == "starting" {
                break;
            }
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(20));
    }
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let s = f.state();
        if s["pending"].is_null() {
            assert_eq!(s["selected"], "alpha");
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(fs::read(&f.cfg).unwrap(), before);
    f.ok(&["recover", "--yes"]);
}
#[test]
fn inactive_update_does_not_replace_config_or_enable_hold() {
    let f = Fixture::new("preserve-user");
    f.add("alpha", "layout { gaps 2; }\n", None);
    f.add("beta", "layout { gaps 4; }\n", None);
    f.activate("alpha");
    f.ok(&["disable", "beta", "--yes"]);
    let config = fs::read(&f.cfg).unwrap();
    let before = f.state();
    fs::write(f.source.join("beta/shell.kdl"), "layout { gaps 19; }\n").unwrap();
    f.ok(&[
        "install",
        f.source.join("beta/shellswitch.toml").to_str().unwrap(),
        "--payload",
        f.source.join("beta").to_str().unwrap(),
        "--yes",
    ]);
    let after = f.state();
    assert_eq!(after["selected"], before["selected"]);
    assert_eq!(after["active"]["process"], before["active"]["process"]);
    assert_eq!(fs::read(&f.cfg).unwrap(), config);
    assert!(
        after["disabled"]
            .as_array()
            .unwrap()
            .contains(&Value::String("beta".into()))
    );
}

#[test]
fn user_settings_import_and_interrupted_file_recovery() {
    let f = Fixture::new("preserve-user");
    f.add("alpha", "layout { gaps 2; }\n", None);
    f.activate("alpha");
    let baseline = f.root.join("my-settings.kdl");
    fs::write(&baseline, "binds { Mod+Return { spawn \"terminology\"; }; }\ninput { keyboard { repeat-delay 499; }; }\n").unwrap();
    let before = f.state();
    f.ok(&[
        "config-update",
        "--user-config",
        baseline.to_str().unwrap(),
        "--policy",
        "preserve-user",
        "--yes",
    ]);
    let after = f.state();
    assert_eq!(before["active"]["process"], after["active"]["process"]);
    assert_eq!(after["selected"], "alpha");
    assert!(
        fs::read_to_string(after["config"]["user_root"].as_str().unwrap())
            .unwrap()
            .contains("499")
    );
    // Simulate interruption after writing a protected file but before commit.
    let gate = f.root.join("bin/alpha");
    let prior = fs::read(&gate).unwrap();
    let changed = b"#!/bin/sh\nexit 42\n";
    fs::write(&gate, changed).unwrap();
    let journal = serde_json::json!({"changes":[{"path":gate,"before":{"kind":"file","data":prior,"mode":493},"after":{"kind":"file","data":changed,"mode":493}}],"previous_protections":after["protections"],"previous_config":after["config"]});
    fs::write(
        f.state.join("files-pending.json"),
        serde_json::to_vec(&journal).unwrap(),
    )
    .unwrap();
    f.fail(&["release", "alpha", "--yes"], "Interrupted file operation");
    f.ok(&["recover", "--yes"]);
    assert_eq!(fs::read(gate).unwrap(), prior);
    f.ok(&["recover", "--yes"]);
}

#[test]
fn service_masks_and_fresh_niri_reload_failure_roll_back() {
    let f = Fixture::new("preserve-user");
    let mockbin = f.root.join("mockbin");
    fs::create_dir(&mockbin).unwrap();
    let ctl = mockbin.join("systemctl");
    fs::write(
        &ctl,
        "#!/bin/sh\ncase \"$2\" in show) echo inactive;; esac\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&ctl, fs::Permissions::from_mode(0o755)).unwrap();
    f.add("alpha", "layout { gaps 2; }\n", None);
    f.add("beta", "layout { gaps 4; }\n", None);
    let manifest = f.source.join("beta/shellswitch.toml");
    let text = fs::read_to_string(&manifest).unwrap().replace(
        "adapter='fixture'",
        "adapter='fixture'\nservices=['beta.service']",
    );
    fs::write(&manifest, text).unwrap();
    f.ok(&["install", manifest.to_str().unwrap(), "--yes"]);
    f.activate("alpha");
    assert_eq!(
        fs::read_link(f.root.join("config/systemd/user/beta.service")).unwrap(),
        PathBuf::from("/dev/null")
    );
    let before = fs::read(&f.cfg).unwrap();
    let niri = mockbin.join("niri");
    fs::write(
        &niri,
        r#"#!/usr/bin/python3
import os, sys, time, json
from pathlib import Path
home = Path(os.environ['HOME'])
trigger = home/'reload-event'
if sys.argv[1] == 'validate': os.execv('/usr/bin/niri',['niri',*sys.argv[1:]])
if sys.argv[-1] == 'event-stream':
    previous = trigger.read_text() if trigger.exists() else ''
    print(json.dumps({'ConfigLoaded':{'failed':False}}),flush=True)
    while True:
        current = trigger.read_text() if trigger.exists() else ''
        if current != previous:
            print(json.dumps({'ConfigLoaded':{'failed':current.startswith('fail')}}),flush=True)
            previous = current
        time.sleep(.01)
else:
    fail = home/'fail-next-reload'
    prefix = 'fail' if fail.exists() else 'ok'
    fail.unlink(missing_ok=True)
    trigger.write_text(prefix+str(time.time_ns()))
"#,
    )
    .unwrap();
    fs::set_permissions(&niri, fs::Permissions::from_mode(0o755)).unwrap();
    let mut state = f.state();
    state["config"]["live_reload"] = Value::Bool(true);
    fs::write(
        f.state.join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    fs::write(f.root.join("fail-next-reload"), "").unwrap();
    f.fail(&["switch", "beta", "--yes"], "previous shell restored");
    assert_eq!(f.state()["selected"], "alpha");
    assert_eq!(fs::read(&f.cfg).unwrap(), before);
    f.activate("beta");
    f.fail(&["gate", "alpha", "--", "msg", "toggle"], "inactive");
}
