//! Source refresh is file-only: no shell or compositor is launched.
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_shellswitch"))
        .arg("--state-dir")
        .arg(root.join("state"))
        .args(args)
        .output()
        .unwrap()
}
fn state(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("state/state.json")).unwrap()).unwrap()
}
#[test]
fn refresh_rebinds_generic_source_preserves_old_revision_and_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let src = root.join("original");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("shell.qml"), "old").unwrap();
    let manifest = root.join("shellswitch.toml");
    fs::write(
        &manifest,
        format!(
            "id = \"example\"\nname = \"Example\"\nargv = [\"/usr/bin/true\", {:?}]\n",
            src.join("shell.qml")
        ),
    )
    .unwrap();
    let out = run(
        root,
        &[
            "install",
            manifest.to_str().unwrap(),
            "--payload",
            src.to_str().unwrap(),
            "--yes",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let before = state(root);
    let old = before["installed"]["example"]["backend"]["argv"][1]
        .as_str()
        .unwrap();
    assert_eq!(fs::read_to_string(old).unwrap(), "old");
    let checkout = root.join("checkout");
    fs::create_dir(&checkout).unwrap();
    fs::write(checkout.join("shell.qml"), "new").unwrap();
    assert!(
        run(
            root,
            &[
                "refresh",
                "example",
                "--payload",
                checkout.to_str().unwrap(),
                "--yes"
            ]
        )
        .status
        .success()
    );
    let after = state(root);
    let new = after["installed"]["example"]["backend"]["argv"][1]
        .as_str()
        .unwrap();
    assert_eq!(fs::read_to_string(new).unwrap(), "new");
    assert_eq!(fs::read_to_string(old).unwrap(), "old");
    assert_eq!(before["active"], after["active"]);
    assert_eq!(before["selected"], after["selected"]);
    fs::write(checkout.join("shell.qml"), "newer").unwrap();
    assert!(run(root, &["refresh", "example", "--yes"]).status.success());
    let latest = state(root);
    assert_eq!(
        fs::read_to_string(
            latest["installed"]["example"]["backend"]["argv"][1]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        "newer"
    );
    fs::remove_dir_all(&checkout).unwrap();
    assert!(!run(root, &["refresh", "example", "--yes"]).status.success());
    assert_eq!(latest, state(root));
    assert!(
        !run(root, &["refresh", "unregistered", "--yes"])
            .status
            .success()
    );
}
