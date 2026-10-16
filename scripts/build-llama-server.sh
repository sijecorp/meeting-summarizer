#!/usr/bin/env bash
# Builds llama-server for macOS arm64 (Metal), llama.cpp and ggml linked in, as the Tauri sidecar
# src-tauri/binaries/llama-server-aarch64-apple-darwin (M6.5 plan 5.1, architecture 11.3). CI runs this
# same script. The commit is scripts/llama.cpp.rev and the CMake options scripts/llama-server.flags.
# The binary is cached outside the repository per commit, target and options, so another worktree only
# copies it. MS_LLAMA_SERVER_CACHE overrides the cache folder (CI).
# Windows: scripts/build-llama-server.ps1.
set -euo pipefail

triple=aarch64-apple-darwin
root=$(cd "$(dirname "$0")/.." && pwd)

if [ "$(uname -s)" != Darwin ]; then
  echo "This builds the macOS sidecar. On Windows run scripts/build-llama-server.ps1." >&2
  exit 1
fi

missing=()
command -v git >/dev/null || missing+=("git: xcode-select --install")
command -v cmake >/dev/null || missing+=("cmake: brew install cmake")
xcrun --find clang >/dev/null 2>&1 || missing+=("Xcode Command Line Tools: xcode-select --install")
if [ ${#missing[@]} -gt 0 ]; then
  echo "Missing for the llama-server build:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi

rev=$(grep -v '^#' "$root/scripts/llama.cpp.rev" | tr -d '[:space:]')
if ! [[ $rev =~ ^[0-9a-f]{40}$ ]]; then
  echo "scripts/llama.cpp.rev must hold one full commit SHA, not '$rev'" >&2
  exit 1
fi
flags_file=$root/scripts/llama-server.flags
flags=()
while read -r scope opt; do
  case $scope in '*' | "$triple") flags+=("-D$opt") ;; esac
done < <(grep -v '^#' "$flags_file")
hash=$(shasum -a 256 "$flags_file" | cut -c1-12)

cache_root=${MS_LLAMA_SERVER_CACHE:-$HOME/Library/Caches/meeting-summarizer/llama-server}
cached=$cache_root/$rev-$triple-$hash/llama-server
if [ -s "$cached" ]; then
  echo "Using the cached build $cached"
else
  # ponytail: one work folder per cache root, so two worktrees building at the same moment collide.
  # Rare (a build happens once per commit and options); give each run its own folder if it bites.
  work=$cache_root/work
  rm -rf "$work"
  git init -q "$work/src"
  git -C "$work/src" fetch -q --depth 1 https://github.com/ggml-org/llama.cpp "$rev"
  git -C "$work/src" checkout -q FETCH_HEAD
  echo "+ cmake -S $work/src -B $work/build ${flags[*]}"
  cmake -S "$work/src" -B "$work/build" "${flags[@]}"
  grep -E '^(GGML_(NATIVE|METAL|METAL_EMBED_LIBRARY|OPENMP|BLAS|LLAMAFILE)|BUILD_SHARED_LIBS|LLAMA_OPENSSL|CMAKE_OSX_(ARCHITECTURES|DEPLOYMENT_TARGET)):' \
    "$work/build/CMakeCache.txt"
  # The CPU baseline must reach ggml, or the binary needs the build machine's CPU.
  grep -qx 'GGML_NATIVE:BOOL=OFF' "$work/build/CMakeCache.txt" || {
    echo "llama.cpp's CMake cache does not have GGML_NATIVE=OFF" >&2
    exit 1
  }
  cmake --build "$work/build" --target llama-server -j "$(sysctl -n hw.ncpu)"
  mkdir -p "$(dirname "$cached")"
  cp "$work/build/bin/llama-server" "$cached.tmp"
  mv "$cached.tmp" "$cached"
  rm -rf "$work"
fi

# Stamp last: an interrupted copy leaves no stamp, and src-tauri/build.rs refuses a release build then.
# Copy, then move over: build.rs never sees a half-written file, and cp onto its placeholder would keep
# the placeholder's mode (not executable).
out=$root/src-tauri/binaries/llama-server-$triple
mkdir -p "$(dirname "$out")"
rm -f "$out.stamp"
cp "$cached" "$out.tmp"
chmod +x "$out.tmp"
mv -f "$out.tmp" "$out"
printf 'rev=%s\nflags=%s\n' "$rev" "$hash" >"$out.stamp"
echo "Sidecar: $out (llama.cpp $rev, flags $hash)"
