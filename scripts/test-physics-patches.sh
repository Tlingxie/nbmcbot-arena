#!/bin/sh
set -eu
nbmcbot_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mkdir -p "$nbmcbot_root/.runtime"
nbmcbot_test_dir=$(mktemp -d "$nbmcbot_root/.runtime/physics-patches.XXXXXX")
trap 'rm -rf "$nbmcbot_test_dir"' EXIT HUP INT TERM
cp "$nbmcbot_root/vendor/azalea-physics/Cargo.toml" "$nbmcbot_test_dir/Cargo.toml"
cp "$nbmcbot_root/Cargo.lock" "$nbmcbot_test_dir/Cargo.lock"
printf '\n[workspace]\n[patch.crates-io]\nazalea-entity = { path = "%s/vendor/azalea-entity" }\n' "$nbmcbot_root" >> "$nbmcbot_test_dir/Cargo.toml"
ln -s "$nbmcbot_root/vendor/azalea-physics/src" "$nbmcbot_test_dir/src"
ln -s "$nbmcbot_root/vendor/azalea-physics/tests" "$nbmcbot_test_dir/tests"
CARGO_TARGET_DIR="$nbmcbot_root/target"
CARGO_PROFILE_TEST_DEBUG=0
export CARGO_TARGET_DIR CARGO_PROFILE_TEST_DEBUG
sh "$nbmcbot_root/scripts/cargo.sh" test --offline --manifest-path "$nbmcbot_test_dir/Cargo.toml" --test physics "${1:-gliding_}" -- --nocapture
