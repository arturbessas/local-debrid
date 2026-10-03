#!/usr/bin/env bash
# Day-to-day development commands. Run without arguments for help.
source "$(dirname "$0")/lib.sh"
cd "$ROOT"

usage() {
  cat <<USAGE
usage: scripts/dev.sh <command>

  test            run all Rust tests (unit + integration with a local swarm)
  lint            rustfmt check + clippy
  run [args]      run the server on this machine (data in ./.dev-data, port 8765)
  swarm <file>    seed a file locally with a tracker + fixture addon (prints its URLs)
  e2e [args]      end-to-end test against a desktop server (scripts/e2e.py --target local)
  e2e-android     end-to-end test against the app on the adb device/emulator
  apk             build and install a debug APK on the adb device
  logcat          follow the app's logs on the adb device
USAGE
}

case "${1:-}" in
  test) cargo test --workspace ;;
  lint)
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings ;;
  run) shift; cargo run --release -p localdebrid -- --data-dir .dev-data "$@" ;;
  swarm) shift; [[ $# -ge 1 ]] || die "usage: dev.sh swarm <file-or-dir>"; cargo run --release -p localdebrid-testkit -- --content "$@" ;;
  e2e) shift; python3 scripts/e2e.py --target local "$@" ;;
  e2e-android) shift; python3 scripts/e2e.py --target android "$@" ;;
  apk)
    ensure_android_env
    (cd android && ./gradlew installDebug -PrustAbis="$ABIS") ;;
  logcat) adb logcat -s LocalDebrid:V AndroidRuntime:E ;;
  *) usage ;;
esac
