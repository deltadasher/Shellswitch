# Verification record — 0.2

All development operations used isolated HOME/XDG fixtures. The restored live Niri configuration, installed shells and real Tonantzintla checkout were inspected only; none was changed, started or stopped.

Passed:

- **25 Shellswitch tests:** 15 unit tests, 4 existing lifecycle integration tests, 6 ownership integration tests.
- **32 Tonantzintla Rust tests:** 10 unit, 7 dry-run, 15 installer executor tests, including the new managed-install test.
- **9 Tonantzintla Python tests:** 4 existing sync tests and 5 new handoff tests.
- Shellswitch `cargo clippy --all-targets -- -D warnings` and formatting check.
- Tonantzintla patch `git apply --check` against its exact source base.
- Both reviewed adapter generators ran against the inspected source layouts without invoking either shell CLI.

Ownership integration tests use actual disposable Linux processes and native `niri validate`. They cover two competing shells, stale public command wrappers, direct bypass detection, supported autostart overrides, emergency holds and explicit release, external config/gate overwrites with archived repair, binding precedence, invalid KDL, conflicting sections, failed health, concurrent switches, a killed switch parent, watchdog restoration, inactive updates, user-settings import, interrupted file-journal recovery, and repeated recovery.

The Niri IPC fixture emits an initial historical success followed by a fresh reload failure; the switch must reject that failure and restore the previous shell/configuration. A later success is accepted. Service fixtures exercise user scope, masks and lifecycle behavior. These are **not** host systemd or live compositor tests.

Native handoff tests verify that managed startup routes through Shellswitch, denied startup propagates failure, commands are gated, emergency stop is routed correctly, the native supervisor's start/respawn guard refuses managed mode, malformed contracts and managed sync fail before mutation, and an asserted gated flag still requires authorization. Installer fixtures preserve the active Niri file and CLI gate, reject replacement, and refuse pending/malformed manager state.

Not verified: rendered shell usability, the live desktop's service manager, every shell-specific widget/command, third-party installers honoring the contract, or complete discovery of all indirect launch paths. No claim is made about the unconfirmed Kitty trigger. See README for supported paths and operational limits.

## 0.2.5 release checks

The release changes the package version, adds the user-local command installer, and updates documentation; Rust application behavior is unchanged from the suite above. The optimized 0.2.5 build passed, Bash syntax and an isolated installer run passed, and an actual Bash login shell resolved the installed `shellswitch` command and reported `shellswitch 0.2.5`. Cargo formatting and Git whitespace checks passed. The packaged binary requires glibc 2.39 or newer, based on its ELF symbol versions.
