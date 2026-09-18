# Changelog

## 0.2.12

- Prefer a single installed lifecycle-adapted candidate when discovery and the registry expose the same shell name.
- Remove false ambiguity for direct `plan` and `switch` commands while retaining full-ID selection for genuinely distinct shells.

## 0.2.11

- Automatically adopt one unambiguous recognized running shell when Shellswitch starts.
- Automatically generate a reviewable Niri user baseline when `enroll` omits `--user-config`.
- Remove unmanaged shell startup commands from that generated baseline while preserving common D-Bus and Polkit startup.

## 0.2.10

- Migrate hash-based discovered registry entries to stable manifest IDs when installing a lifecycle adapter.
- Preserve the selected/active process while replacing its metadata with the installed adapter.

## 0.2.9

- Refuse switching when Niri configuration ownership has not been enrolled.
- Refuse switching away from an unmanaged active shell whose supervisor/respawn paths cannot be controlled safely.
- Show diagnostics and concrete repair commands for both conditions.

## 0.2.8

- Prevent timestamped runtime backups from being selected as current shell roots.
- Reduce the default scan from thousands of irrelevant backup candidates to the current runtime set.
- Keep failed TUI actions open as an explicit diagnostic instead of silently returning to the browser.

## 0.2.7

- Prevent bulk user data, rollback copies, and caches from consuming the discovery budget before installed shell runtimes are reached.
- Group helper scripts inside a detected shell project under its entrypoint instead of listing them as independent shells.
- Add regression coverage for project grouping.

## 0.2.6

- Refresh default discovery from the complete user XDG data tree on every scan.
- Detect Quickshell entrypoints case-insensitively, including installed `Shell.qml` trees.
- Add regression coverage for installed runtimes outside application metadata directories.

## 0.2.5

First official GitHub release.

- Add a user-local installer so Shellswitch is available as the `shellswitch` command.
- Ship a Linux x86_64 executable, source and documentation with release checksums.
- Include transactional configuration ownership, serialized switches, validation and rollback.
- Include startup gates, emergency disable/release, drift diagnostics and repair.
- Include reviewed Tonantzintla/Serpantinum bridges and the separate Tonantzintla handoff patch.

Installing the command does not activate a shell or install desktop protections. Live graphical switching remains unverified in the development sandbox; supported paths and integration gaps are documented in README.
