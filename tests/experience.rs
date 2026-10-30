use serde_json::json;
use std::{fs, process::Command};
fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_shellswitch"))
        .arg("--state-dir")
        .arg(dir)
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn first_use_guidance_is_actionable_and_does_not_create_a_selection() {
    let t = tempfile::tempdir().unwrap();
    let out = run(t.path(), &["guide"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("scan") && text.contains("plan") && text.contains("keep"));
    assert!(!t.path().join("state.json").exists());
}
#[test]
fn human_status_separates_staged_and_active_and_preserves_json_default() {
    let t = tempfile::tempdir().unwrap();
    let c = json!({"id":"sample","name":"Sample","kind":"shell","framework":"manifest","source":"/packages/sample/new/shellswitch.toml","evidence":[],"protocols":[],"compositors":[],"backend":{"type":"review","reason":"fixture"},"declared":true,"running_pids":[]});
    let mut old = c.clone();
    old["source"] = json!("/packages/sample/old/shellswitch.toml");
    let state = json!({"schema":2,"selected":"sample","installed":{"sample":c},"active":{"candidate":old,"process":null,"owned_group":false},"pending":null,"last_event":"fixture"});
    fs::write(
        t.path().join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let out = run(t.path(), &["status", "--human"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("/old/") && text.contains("/new/") && text.contains("not active"));
    assert!(!text.contains("Running: Sample"));
    let raw = run(t.path(), &["status"]);
    let parsed: serde_json::Value = serde_json::from_slice(&raw.stdout).unwrap();
    assert_eq!(
        parsed["active"]["candidate"]["source"],
        state["active"]["candidate"]["source"]
    );
}
#[test]
fn update_without_selection_never_guesses_a_shell() {
    let t = tempfile::tempdir().unwrap();
    let out = run(
        t.path(),
        &[
            "--isolated",
            "--root",
            t.path().to_str().unwrap(),
            "update",
            "--yes",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("No selected shell"));
}
