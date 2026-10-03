# Shared helpers for the scripts in this directory.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ABIS="${ABIS:-arm64-v8a,x86_64}"

die() { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }

find_android_sdk() {
  for candidate in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do
    if [[ -n "$candidate" && -d "$candidate/platforms" ]]; then
      export ANDROID_HOME="$candidate"
      return
    fi
  done
  die "Android SDK not found; set ANDROID_HOME"
}

find_android_ndk() {
  if [[ -n "${ANDROID_NDK_HOME:-}" && -d "$ANDROID_NDK_HOME" ]]; then return; fi
  local ndk
  ndk="$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
  [[ -n "$ndk" ]] || die "Android NDK not found; install it with: sdkmanager 'ndk;27.1.12297006'"
  export ANDROID_NDK_HOME="$ndk"
}

ensure_rust_android() {
  command -v cargo >/dev/null || die "Rust is not installed (https://rustup.rs)"
  command -v cargo-ndk >/dev/null || { info "installing cargo-ndk"; cargo install cargo-ndk; }
  local installed
  installed="$(rustup target list --installed)"
  for target in aarch64-linux-android x86_64-linux-android; do
    grep -qx "$target" <<<"$installed" || rustup target add "$target"
  done
}

ensure_android_env() {
  find_android_sdk
  find_android_ndk
  ensure_rust_android
  [[ -f "$ROOT/android/local.properties" ]] || echo "sdk.dir=$ANDROID_HOME" > "$ROOT/android/local.properties"
}

version() {
  grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"(.*)".*/\1/'
}
