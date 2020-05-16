#!/usr/bin/env bash

set -e

apt-get -y update && apt-get -y install \
  fuse libfuse-dev pkg-config gcc llvm libclang-dev clang curl
su vagrant -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
