# Cooperative handoff contract v1

`$XDG_CONFIG_HOME/shellswitch/handoff.json` is the shared ownership marker:

```json
{"version":1,"contract":"shellswitch-handoff-v1","state_dir":"/absolute/state","shellswitch":"/absolute/shellswitch"}
```

Treat a malformed/unsupported marker as an error, not as permission to start or overwrite. The native installer and Shellswitch use exclusive `flock` on `state_dir/lock`. The installer reads schema 2 state while holding that lock, refuses a pending switch/file journal, preserves Niri and managed CLI paths, and never activates. Update orchestration must not hold the lock while invoking a child installer that also takes it.

Activation runs with `SHELLSWITCH_STATE_DIR`, `SHELLSWITCH_SHELL_ID`, and `SHELLSWITCH_HANDOFF`. Before a cooperative shell starts or respawns, validate:

```sh
/absolute/shellswitch --state-dir /absolute/state authorize SHELL_ID --purpose runtime
```

`authorize` atomically reads state without taking the mutation lock: it may run while the switching parent holds that lock. It accepts only the matching transaction target/token in Starting/Trial or the committed, selected, live instance's lease. Holds override leases. `configure` is rejected outside a transaction. An authorization check is not permission for arbitrary file writes; Shellswitch remains the writer of the composed entrypoint. Leases distinguish cooperative actions and expire on handoff; they are not secrets or a same-user security boundary.

Ordinary public start/session-start commands call `resume --shell ID`. This starts only the already-selected, non-disabled shell. It cannot select a previously inactive one. Public widget commands go through `gate`; `SHELLSWITCH_GATED=1` alone is insufficient authorization. Gate routes are explicit argv arrays, not evaluated command strings. Install/update must never imply `resume`, `switch` or `release`.

The supplied Tonantzintla patch uses Shellswitch's foreground supervisor in managed mode, so its native supervisor refuses start/serve and checks again before each respawn. This removes two competing supervisors rather than attempting to coordinate both. Native `sync`/`update` and uninstall are refused while managed; native `apply --niri keep` preserves shared files under the common lock. A separately staged revision becomes active only through `switch`.

Direct external installers can bypass all of this. File snapshots, explicit process inventories, and recovery journals provide operational detection/recovery, not access control.
