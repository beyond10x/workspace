#!/usr/bin/env bash
set -euo pipefail

declared_version="$(sed -n 's/^rust-version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
toolchain_version="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml | head -1)"
image_version="$(sed -n 's/^FROM rust:\([^-@]*\)-bookworm@.* AS builder$/\1/p' Dockerfile | head -1)"

test -n "$declared_version"
case "$declared_version" in
  *.*.*) ;;
  *.*) declared_version="${declared_version}.0" ;;
esac
test "$toolchain_version" = "$declared_version"
test "$image_version" = "$declared_version"
