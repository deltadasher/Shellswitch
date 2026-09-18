#!/usr/bin/env bash
# Install the command only; never activate a shell or change desktop settings.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${HOME:?HOME must be set}/.local/bin"
binary="$root/shellswitch"
while (($#)); do
    case "$1" in
        --bin-dir|--binary)
            (($# >= 2)) || { echo "Missing value for $1" >&2; exit 2; }
            if [[ "$1" == --bin-dir ]]; then bin_dir="$2"; else binary="$2"; fi
            shift 2 ;;
        --help|-h)
            echo 'Usage: ./install.sh [--bin-dir DIRECTORY] [--binary PATH]'
            echo 'Installs shellswitch in ~/.local/bin; builds with Cargo if no bundled binary exists.'
            exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done
if [[ ! -f "$binary" ]]; then
    [[ "$binary" == "$root/shellswitch" ]] || { echo "Binary not found: $binary" >&2; exit 1; }
    command -v cargo >/dev/null || { echo 'Install Rust/Cargo or use the Linux release archive.' >&2; exit 1; }
    cargo build --release --locked --manifest-path "$root/Cargo.toml" --target-dir "$root/target"
    binary="$root/target/release/shellswitch"
fi
[[ -x "$binary" ]] || { echo "Binary is not executable: $binary" >&2; exit 1; }
version="$("$binary" --version)"
[[ "$version" == 'shellswitch '* ]] || { echo 'Expected a Shellswitch binary.' >&2; exit 1; }
mkdir -p -- "$bin_dir"
bin_dir="$(cd -- "$bin_dir" && pwd)"
[[ ! -d "$bin_dir/shellswitch" ]] || { echo 'Destination is a directory.' >&2; exit 1; }
staged="$(mktemp "$bin_dir/.shellswitch-install.XXXXXX")"
trap 'rm -f -- "$staged"' EXIT
install -m 755 -- "$binary" "$staged"
# Atomic replacement keeps any currently running manager on its existing inode.
mv -fT -- "$staged" "$bin_dir/shellswitch"
printf 'Installed %s at %s/shellswitch\n' "$version" "$bin_dir"
case ":$PATH:" in
    *":$bin_dir:"*) echo 'Run: shellswitch' ;;
    *) printf 'Add this directory to your Bash PATH, then open a new terminal:\n  export PATH=%q:"$PATH"\n' "$bin_dir" ;;
esac
