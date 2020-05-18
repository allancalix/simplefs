#!/usr/bin/env bash

set -e

MOUNT_POINT="$HOME/fuse/vdir"

cargo build --bin sfs
mkdir -p "$MOUNT_POINT"
sudo RUST_LOG=debug ./target/debug/sfs "$MOUNT_POINT"
