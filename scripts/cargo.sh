#!/bin/sh
set -eu
nbmcbot_toolchain=nightly-2026-03-01
RUSTC=$(rustup which --toolchain "$nbmcbot_toolchain" rustc)
RUSTDOC=$(rustup which --toolchain "$nbmcbot_toolchain" rustdoc)
PATH="$(dirname "$RUSTC"):$PATH"
export RUSTC RUSTDOC PATH
exec rustup run "$nbmcbot_toolchain" cargo "$@"
