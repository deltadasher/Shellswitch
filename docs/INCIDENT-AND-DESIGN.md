# Configuration ownership and lifecycle investigation

The reported overwrite is treated as a configuration/lifecycle ownership failure, not malware. The cause of the unexpected Kitty windows is unconfirmed.

Local source inspected before implementation:

- Shellswitch 0.1: a display-scoped flock and process-only trial journal. No configuration snapshots, startup-source ownership, inactive-shell gate, or installer integration. A display-specific lock cannot serialize writers to the same user Niri config.
- Installed Serpantinum: `bin/serpantinum` calls `ensure_daemon` for launch/msg/ipc; it can start `bin/serpantinumd`. Its daemon and CLI contain broad `pkill -f` cleanup. Invoking the public CLI as a status/stop probe is therefore not an appropriate discovery or safe-stop backend.
- Tonantzintla checkout at `/home/delta/.gemini/antigravity/scratch/Tonantzintla`: clean main at inspection. Source and installed tree are distinct. `bin/blackhole sync` installs then restarts; Rust `install_niri` can copy the full template over the Niri entrypoint; maintenance update reuses the recorded Niri mode and restarts a detected running shell. `session-daemon.py` is an independent restarting supervisor.
- The restored live Niri entrypoint contains custom input/layout/bindings and a legacy `astralithctl session-start` startup. It was read, not edited or activated for development.

Implementation direction:

1. One user/config ownership domain and durable selected-shell state; serialize installation, protection, switching, repair, and emergency operations under the same flock.
2. Register/stage code and config without activating anything. Registered adapters declare exact process identities, public CLI routes, autostart entries, and dedicated service units.
3. Gate supported entrypoints before stopping processes. Do not invoke a third-party broad-kill routine. Unintegrated direct entrypoints remain detectable gaps, not a claimed security boundary.
4. Freeze included KDL dependencies and compose a staged destination with an explicit user/shell conflict policy. Validate before handoff and again before commit. Archive external drift before recovery.
5. Persist each phase before its side effects; make process launch wait for a durable journal acknowledgement. Recovery repeats idempotent actions and does not resurrect an emergency-disabled shell.
6. A versioned handoff record and narrowly scoped transaction lease allow cooperative native hooks to distinguish authorized activation from an unrelated installer/shortcut.

Same-user programs can bypass wrappers, modify files and state, unmask units, or execute an original binary directly. This is cooperative lifecycle coordination with drift detection and recoverable snapshots, not a sandbox, malware detector, or security boundary.
