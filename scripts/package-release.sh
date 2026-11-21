#!/usr/bin/env bash
# Build and package the committed source in a temporary directory.
set -euo pipefail
if (( $# > 1 )); then
    echo 'Usage: scripts/package-release.sh [output-directory]' >&2
    exit 2
fi
repo=$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)
output=$(realpath -m -- "${1:-$repo/dist}")
host=$(rustc -vV | sed -n 's/^host: //p')
case $host in
    *-unknown-linux-gnu) platform="linux-${host%%-*}" ;;
    *) echo "This archive format targets native GNU/Linux builds, found $host." >&2; exit 1 ;;
esac
commit=$(git -C "$repo" rev-parse HEAD)
version=$(git -C "$repo" show "$commit:Cargo.toml" | sed -nE 's/^version = "([0-9A-Za-z.+-]+)"$/\1/p')
[[ -n $version ]] || { echo 'Missing package version.' >&2; exit 1; }
epoch=$(git -C "$repo" show -s --format=%ct "$commit")
name=shellswitch-$version
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
source_dir=$stage/$name
mkdir -p -- "$source_dir"
git -C "$repo" archive "$commit" | tar -x -C "$source_dir"

# Cargo passes encoded flags as individual arguments, including paths with spaces.
flags=()
if [[ -n ${CARGO_ENCODED_RUSTFLAGS:-} ]]; then
    IFS=$'\x1f' read -r -a flags <<< "$CARGO_ENCODED_RUSTFLAGS"
elif [[ -n ${RUSTFLAGS:-} ]]; then
    read -r -a flags <<< "$RUSTFLAGS"
fi
cargo_dir=$(realpath -m -- "${CARGO_HOME:-$HOME/.cargo}")
for mapping in "$HOME=/build" "$repo=/source" "$stage=/build" "$cargo_dir=/cargo"; do
    flags+=("--remap-path-prefix=$mapping")
done
export CARGO_ENCODED_RUSTFLAGS
CARGO_ENCODED_RUSTFLAGS=$(IFS=$'\x1f'; printf '%s' "${flags[*]}")
cargo build --locked --release --target "$host" --target-dir "$stage/target" --manifest-path "$source_dir/Cargo.toml"
install -m 755 -- "$stage/target/$host/release/shellswitch" "$source_dir/shellswitch"
for location in "$HOME" "$repo" "$stage" "$cargo_dir"; do
    if [[ $location != / ]] && LC_ALL=C grep -aFq -- "$location/" "$source_dir/shellswitch"; then
        echo 'The binary contains a local build path. Review the toolchain before packaging.' >&2
        exit 1
    fi
done

archive=$name-$platform.tar.gz
tar --sort=name --format=gnu --owner=0 --group=0 --numeric-owner --mtime="@$epoch" \
    -C "$stage" -cf - "$name" | gzip -n -9 > "$stage/$archive"
(cd "$stage" && sha256sum -- "$archive" > SHA256SUMS)
mkdir -p -- "$output"
install -m 644 -- "$stage/$archive" "$stage/SHA256SUMS" "$output/"
printf 'Packaged %s at %s\n' "$commit" "$output/$archive"
