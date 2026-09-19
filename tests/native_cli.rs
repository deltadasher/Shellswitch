use serde_json::json;
use std::{fs, process::Command};

#[test]
fn native_cli_preserves_unknown_commands_arguments_and_inactive_gate() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    let script = dir.join("native.sh");
    fs::write(
        &script,
        "#!/bin/sh
printf '%s\\n' \"$@\" > \"$CAPTURE\"
",
    )
    .unwrap();
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let start: u64 = stat[stat.rfind(')').unwrap() + 2..]
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let candidate = json!({
        "id":"unknown-provider", "name":"Arbitrary Desktop", "kind":"shell",
        "framework":"manifest", "source":dir.join("manifest.toml"), "evidence":[],
        "protocols":[], "compositors":[], "backend":{"type":"process","argv":["/bin/sleep","60"],"cwd":dir},
        "declared":true,"running_pids":[],
        "lifecycle":{"adapter":"cooperative-custom","commands":[{
            "path":"/fixture/native", "native_argv":["/bin/sh",script],
            "blocked_prefixes":[["install"]]
        }]}
    });
    let mut state = json!({
        "schema":2,"selected":"unknown-provider",
        "active":{"candidate":candidate,"process":{"pid":std::process::id(),"start":start,"boot":boot.trim()},"owned_group":false,"lease":"fixture"},
        "pending":null,"installed":{"unknown-provider":candidate},"last_event":"fixture"
    });
    let state_path = dir.join("state.json");
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let capture = dir.join("capture");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_shellswitch"))
            .arg("--state-dir")
            .arg(dir)
            .args([
                "gate",
                "unknown-provider",
                "--entry",
                "/fixture/native",
                "--",
            ])
            .args(args)
            .env("CAPTURE", &capture)
            .output()
            .unwrap()
    };
    let args = [
        "future-feature",
        "two words",
        "$(touch should-not-exist)",
        "",
    ];
    let result = run(&args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read_to_string(&capture).unwrap(),
        "future-feature
two words
$(touch should-not-exist)

"
    );
    fs::remove_file(&capture).unwrap();
    assert!(!run(&["install"]).status.success());
    assert!(!capture.exists());
    state["selected"] = json!("another-provider");
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(!run(&["future-feature"]).status.success());
    assert!(!capture.exists());
    // Reconcile a legacy discovery ID using the exact declared entrypoint.
    let runtime = dir.join("runtime");
    fs::create_dir(&runtime).unwrap();
    let qml = runtime.join("shell.qml");
    fs::write(&qml, "fixture").unwrap();
    state["selected"] = json!("legacy-hash");
    state["active"]["candidate"]["id"] = json!("legacy-hash");
    state["active"]["candidate"]["lifecycle"] = serde_json::Value::Null;
    state["active"]["candidate"]["backend"] =
        json!({"type":"process","argv":["/fixture/qs","-p",qml],"cwd":dir});
    state["installed"]["unknown-provider"]["lifecycle"]["processes"] =
        json!([{"argv_prefix":["/fixture/qs","-p",runtime]}]);
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let result = run(&["future-feature"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let migrated: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(migrated["selected"], "unknown-provider");
    assert_eq!(
        migrated["active"]["candidate"]["backend"]["argv"][2],
        json!(qml)
    );
}
