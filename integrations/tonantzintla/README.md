# Tonantzintla handoff integration

`handoff-v1.patch` targets Tonantzintla main commit `917f2f57b622e4f826d847e136065a5ebc92c552`. It was built and tested in an isolated copy, not applied to the live checkout/runtime. Upstream is GPL-3.0-or-later; this patch is supplied under the same license.

Apply to a reviewed checkout (preserve local changes first):

```sh
git apply --check /path/to/handoff-v1.patch
git apply /path/to/handoff-v1.patch
cargo test --manifest-path install/Cargo.toml --locked
python3 -m unittest discover -s tests -p 'test_shellswitch.py'
```

The installer patch uses Rust `File::lock`, requiring Rust 1.89 or newer. Tests were built with the available Rust 1.97 toolchain. Native installation requires rebuilding the installer; editing source alone does not update the installed binary. Stage the patched project through Shellswitch's `install --payload`, and explicitly switch to that revision. To protect an existing raw native entrypoint too, deploy the patch through the project's normal installation process with `--niri keep`, after reviewing the managed marker and gates. The generated Shellswitch bridge bypasses raw `blackhole` for supported IPC routes, so basic managed switching does not depend on executing it.

Changes:

- `blackhole` checks the ownership marker before command side effects. Managed lifecycle actions route to Shellswitch. Widget commands require a gate or a validated runtime lease.
- The native session daemon refuses managed startup and checks before every respawn.
- Native `apply --niri keep` holds the shared ownership lock, preserves Niri and command gates, and performs no activation. `--niri replace` is rejected.
- Native sync/update is refused while managed because it can execute newly pulled code that does not honor this contract. Update source separately, stage it through Shellswitch, then switch explicitly. Managed uninstall is refused to avoid removing owned startup paths.
- Without a marker, existing native behavior is preserved.

The patched native installer tests cover preserved Niri and gates, rejected replace, interrupted Shellswitch operations and malformed markers. Python tests cover routed startup, stale command gates, emergency stop, native supervisor refusal, malformed markers and invalid leases. This is a contract patch, not a claim that every Tonantzintla UI feature has been tested under the bridge.
