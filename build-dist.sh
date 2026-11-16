#!/usr/bin/env bash
# Build single-file release binaries into dist/:
#   dist/linux-x86_64/gpu-r          (needs only glibc; Vulkan loader is opened at runtime)
#   dist/windows-x86_64/gpu-r.exe    (static CRT, needs only system DLLs)
# Windows is cross-compiled with cargo-xwin (cargo install cargo-xwin;
# rustup target add x86_64-pc-windows-msvc).
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
cargo xwin build --release --target x86_64-pc-windows-msvc
mkdir -p dist/linux-x86_64 dist/windows-x86_64
cp target/release/gpu-r dist/linux-x86_64/gpu-r
cp target/x86_64-pc-windows-msvc/release/gpu-r.exe dist/windows-x86_64/gpu-r.exe
( cd dist && sha256sum linux-x86_64/gpu-r windows-x86_64/gpu-r.exe > SHA256SUMS )
cat dist/SHA256SUMS
