# Shellswitch 0.2.12

Rust desktop-shell discovery and transactional lifecycle control, with a three-pane Ratatui interface inspired by Linutil. This version adds configuration ownership, inactive-shell gates, staged installs, rollback, and incident-specific Tonantzintla/Serpantinum bridges.

## Install the command

Download and extract the Linux x86_64 archive from the [0.2.5 release](https://github.com/deltadasher/Shellswitch/releases/tag/v0.2.5), then run:

```sh
./install.sh
shellswitch --version
shellswitch
```

The installer places the executable at `~/.local/bin/shellswitch`. If that directory is not on PATH, it prints the Bash line to add. It does not activate a desktop shell or change Niri, startup files, or shell selection. From a source checkout, the same installer builds with Cargo first when there is no bundled binary. `--bin-dir DIRECTORY` selects a different command location.

The release binary targets Linux x86_64 with glibc 2.39 or newer and libgcc_s; it is not a static musl or ARM binary. Build from source if your system cannot run it. Keep the installed path stable because managed entrypoints refer to it.

## Open Shellswitch

```sh
shellswitch                 # browse; opening the TUI does not activate anything
shellswitch doctor
shellswitch inventory
```

**Development did not change the live desktop.** The included binary and Tonantzintla patch are tested deliverables, not an assertion that the installed desktop is already protected. Discovery runs a fresh bounded scan each time you open Shellswitch or run `scan`; it is not a one-time database. First review the startup inventory and integration gaps below.

## Transaction and ownership

One persistent `selected` shell and one user-level flock serialize mutations. State defaults to `$XDG_STATE_HOME/shellswitch` (`~/.local/state/shellswitch`). All commands and hooks must use this same ownership domain. Do not create separate active state directories for the same desktop.

A switch captures the previous selection, exact process identities, entrypoint bytes, permissions/symlinks and startup protections. It freezes referenced KDL files, validates the staged destination using `niri validate`, installs command/autostart/service gates, stops declared owned processes, atomically replaces the entrypoint, requests a Niri reload and waits for a fresh `ConfigLoaded` result. It then starts the destination and checks process survival and any adapter health command. `keep` commits after rechecking; failure, interruption or trial expiry restores the prior configuration and shell.

The process supervisor waits for a durable launch ticket before executing shell code. A detached watchdog handles interrupted switches and the 20-second confirmation period. File-only operations have a separate recoverable journal. Recovery archives intervening external edits instead of discarding them. If recovery itself fails, its journal remains for `recover --yes`; this is not a guarantee against disk failure or arbitrary same-user interference.

```sh
shellswitch switch SHELL_ID --yes
shellswitch keep              # within 20 seconds after readiness
# alternatively:
shellswitch revert
shellswitch recover --yes     # interrupted operation; safe to repeat
```

Process survival and successful validation do not prove visual usability. Inspect the desktop before keeping the trial. Niri live reload requires `load-config-file --path` and `ConfigLoaded` event support (tested against the 26.04 interface using a fixture). `enroll --offline` is for fixtures/offline validation, not a verified live handoff.

## Install separately; preserve personal settings

`install` copies a payload into a versioned package directory and records its manifest. It never starts the shell, rewrites Niri, replaces command gates, clears a hold, or selects the new revision. It is not a dependency package manager. Use `--payload` for pinned runtime code; without it only metadata/configuration is captured and referenced code remains mutable.

```sh
shellswitch install /path/to/shellswitch.toml --payload /path/to/project --yes
shellswitch enroll --config "$HOME/.config/niri/config.kdl" \
  --user-config /path/to/my-reviewed-user-baseline.kdl --policy preserve-user --yes
shellswitch inventory
shellswitch protect --yes
shellswitch plan SHELL_ID
```

Keep the executable at a stable absolute path before protection/activation: generated gates and startup use that path. Enrolment snapshots configuration but leaves the live entrypoint unchanged. The user baseline should contain your own settings and bindings; review old shell startup commands and includes before adopting it. Shell-specific fragments must omit startup nodes: Shellswitch supplies `resume`. Shellswitch does not delete arbitrary personal startup commands from your baseline.

Conflict policy is mandatory:

| Policy | Behavior |
|---|---|
| `abort-conflicts` | Refuse overlapping top-level sections; deliberately conservative, even for disjoint bindings. |
| `preserve-user` | Include shell settings first, user settings last, using Niri's documented merge/override rules. |
| `shell-wins` | Reverse that order, explicitly allowing shell overrides. |

Files are not silently text-merged. In particular, Niri does not merge every nested setting. The original files and captured includes remain available. Intentional user changes use the same explicit policy:

```sh
shellswitch config-update --user-config /path/to/edited-baseline.kdl \
  --policy preserve-user --yes
```

This validates and journals a configuration-only update without restarting or selecting a shell. Direct edits of managed entrypoints are reported as drift; use `repair` to restore, or archive/review those edits and import the intended settings through `config-update`.

## Stop resurrection and recover drift

Lifecycle manifests declare public CLI paths, supported command routes, dedicated user services, user autostart overrides, and exact process argv prefixes. Unsupported CLI forms fail closed. A command gate verifies selection, hold state, configuration ownership and transaction phase before running a route. Invocations are supervised and cancelled on handoff. It never calls an upstream CLI just to discover or stop a shell.

Declared inactive systemd services are persistently masked in the user configuration, then stopped. Declared autostarts become `Hidden=true` overrides. The selected shell resumes through Shellswitch. Legacy processes are matched by exact declared executable/arguments and signalled using PID/start-time/boot identity with pidfds. Serpantinum's legacy daemon uses exact-PID forced termination to avoid its broad cleanup trap; no generic `pkill -f` is used.

```sh
shellswitch doctor                  # concise findings and next actions
shellswitch doctor --json
shellswitch inventory               # declarations + bounded static startup references
shellswitch repair --yes            # archive overwrite, restore ownership; no shell launch
shellswitch disable serpantinum --yes
shellswitch release serpantinum --yes
```

`disable` first persists an emergency hold, stops that shell, and keeps supported startup paths gated. `release` clears the hold **without starting it**; selecting it requires a subsequent switch. Disabling the selected shell leaves its last configuration in place so personal settings remain available; its command gates deny execution. The hold survives installation/update and interrupted cleanup.

TUI: arrows select, Tab changes group, `/` searches, Enter reviews a switch, `k` keeps, `u` reverts, `x` disables the highlighted registered shell, `l` releases its hold, `d` shows diagnostics, `f` repairs, `e` recovers, `r` rescans, `q` quits. Mutating TUI actions have review prompts. Diagnostics run on request and before relevant operations; there is no always-running drift watcher.

## Incident integrations

Generate a bridge against the actual installed/source root; these commands only print TOML:

```sh
shellswitch adapter tonantzintla --shell-root /path/to/Tonantzintla > tonantzintla.toml
shellswitch adapter serpantinum --shell-root /path/to/serpantinum > serpantinum.toml
```

Add `--fragment /path/to/reviewed-shell-only.kdl` when shell-specific Niri settings are wanted. No full vendor configuration is guessed or imported. Review/add the service names, autostart override paths and exact legacy launch forms found by inventory **before** installing/protecting. The generated bridges cover observed source layouts, not every historical launch spelling.

The bridges run Quickshell in the foreground and send supported widget IPC directly. Serpantinum's widget commands bypass `ensure_daemon`; its workspace helpers, setup UI and auxiliary wellbeing daemon are not recreated. Tonantzintla routes common widget and bar-edit IPC; multi-argument `open` extensions, lockscreens and other CLI features need additional reviewed routes. Generated health checks verify command success plus supervised process survival, not rendered surfaces.

The [Tonantzintla patch](integrations/tonantzintla/handoff-v1.patch) implements the shared native contract. Its [integration guide](integrations/tonantzintla/README.md) explains installation and tests. It is supplied separately and has **not** been applied to your checkout or installed runtime.

## Limits and external installers

An installer that ignores this contract can overwrite Niri, replace a gate, unmask a service or invoke a raw binary. Shellswitch cannot prevent arbitrary programs running as your user from doing those things. Saved content and process identities allow detection and repair when Shellswitch next checks; an overwrite may affect the desktop before detection. `repair` restores saved files and archives foreign contents; `disable` stops an identified competing shell. The unexpected Kitty trigger remains unconfirmed and is not attributed to malware.

66/runit/s6/OpenRC-specific service control is not implemented; use the process backend only after independently retiring their respawners. systemd masks require dedicated shell units; indirect units, timers, D-Bus activation, service-generated overrides and arbitrary scripts require review. Generic service backends do not inject the native transaction lease into systemd's manager environment; use the foreground bridge for the supplied native contract. Escaping/double-forking descendants need a real lifecycle adapter, not guessed process-name killing.

Startup audit is bounded to standard config/startup roots, 10,000 entries, depth 10 and text files up to 256 KiB. It reports references, not proof that all launch paths were found. Symlinks and indirect/generated code can evade that audit. Install copying rejects escaping/directory symlinks; KDL snapshotting rejects cyclic includes and unsupported multiline string forms. Missing optional includes must be resolved explicitly.

Version 0.1 used display-scoped state directories. Finish/revert old trials and stop the old manager before enrolling in the new shared domain; no automatic cross-directory migration is performed. Do not run both managers. Full desktop sessions/compositors remain review-only: this tool does not replace a running compositor or configure the display manager.

## What “any shell” means here

There is no standard desktop-shell identity or lifecycle contract. A layer-shell client can be a panel, a screenshot selector, a wallpaper, or a lockscreen. A shell can also be a collection of scripts, a native application, a QML project, or part of the compositor itself. Arbitrary source code cannot reliably reveal every runtime requirement or a safe stop command.

Shellswitch uses **open-ended discovery and explicit evidence**, not a catalog of shell brands. It distinguishes inferred shells, components, full sessions, and review candidates. Static evidence is never labelled runtime verification. A manifest permits any foreground executable or dedicated user service to participate without adding Rust code.

| Provider | Evidence and behavior |
| --- | --- |
| XDG config and additional roots | Bounded recursive scan; canonical paths; symlink loop prevention and deduplication |
| Quickshell | Finds arbitrary `shell.qml` projects, follows the scanned component tree for panel/service evidence, derives `qs --path` argv |
| AGS/Astal | Identifies application startup plus Astal usage; version-dependent lifecycle stays review-only |
| Eww | Identifies window definitions; window selection and daemon ownership require a manifest |
| Source APIs | Searches Python, Rust, C/C++, Vala, JS/TS, QML, and scripts for layer-shell/X11 desktop APIs; groups common project layouts |
| Executables | Samples ELF bytes for layer-shell libraries/symbols and X11 dock/desktop atoms; these remain review candidates |
| Launcher scripts | Shows relevant launch lines without sourcing, executing, or evaluating scripts |
| Desktop entries | Extracts the main section's name, description and Exec as evidence; does not evaluate Exec or invent field-code expansion |
| User-service definitions | Reads descriptions and launch/stop declarations; explicit service manifests select the systemd backend |
| Running processes | Correlates exact launch/config identities; adds mapped shell-library evidence where `/proc` permits it; filters by graphical session |
| Local manifests | Defines an arbitrary shell's exact argv or dedicated user-service unit, protocol/compositor constraints, and required Wayland globals |

Default roots include XDG config, the complete user XDG data tree, application/session directories, local launchers, system user-unit definitions, `/usr/bin`, and `/usr/local/bin`. This catches installed runtimes such as `~/.local/share/<provider>/src/quickshell/Shell.qml`, including uppercase entrypoint names. It does **not** search the Internet, install packages, read every file on every disk, or automatically run unknown executables to identify them. Add project trees explicitly when they live elsewhere:

```bash
shellswitch --root /path/to/custom/shells
shellswitch --isolated --root /path/to/a/project scan --json
```

Limits: depth 12, 30,000 files; text metadata up to 512 KiB; shell component aggregation up to about 2 MiB; generic evidence budget 512 MiB and a 128 KiB sample for executables. Large, deeply nested, obfuscated, statically linked or unusual projects can be missed. Scan a narrower root and/or supply a manifest. Source references can be optional or appear in comments; a manifest can correct inferred constraints.

## Build and verification

```sh
cargo build --release --locked
cargo test --locked -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

See [verification](docs/VERIFICATION.md), [incident analysis](docs/INCIDENT-AND-DESIGN.md), and [handoff contract](docs/HANDOFF.md). Linux process tests use disposable processes and temporary HOME/XDG directories. Real Niri validates fixtures; live reload and user-service behavior use fake endpoints. Actual graphical switching and host service operation are not verified in the sandbox.

Core code: `control.rs` (journal/transactions), `ownership.rs` (snapshots/KDL), `lifecycle.rs` (gates/holds/repair), `niri.rs` (reload acknowledgement), `adapters.rs` (reviewed bridges), `process.rs` (supervision/identity), `discovery.rs` and `generic.rs` (discovery), `ui.rs` (TUI).

## References

- [Niri include semantics](https://github.com/niri-wm/niri/wiki/Configuration:-Include)
- [Niri live configuration loading](https://github.com/niri-wm/niri/blob/main/docs/wiki/Integrating-niri.md) and [ConfigLoaded events](https://docs.rs/niri-ipc/latest/niri_ipc/enum.Event.html)
- [XDG autostart overrides](https://specifications.freedesktop.org/autostart/latest/)
- [Linutil](https://github.com/ChrisTitusTech/linutil), visual inspiration; no code copied.

Shellswitch core is MIT. The separate Tonantzintla integration patch modifies GPL-3.0-or-later upstream code and is supplied under that license; it is not a relicensing of Tonantzintla.
