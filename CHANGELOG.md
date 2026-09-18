# Changelog

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
