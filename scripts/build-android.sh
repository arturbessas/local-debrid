#!/usr/bin/env bash
# Builds the installable release APK into dist/.
# ABIS=arm64-v8a,x86_64 by default (x86_64 is for emulators).
source "$(dirname "$0")/lib.sh"
ensure_android_env

cd "$ROOT/android"
info "building release APK for $ABIS"
./gradlew --console=plain assembleRelease -PrustAbis="$ABIS"

mkdir -p "$ROOT/dist"
out="$ROOT/dist/LocalDebrid-$(version).apk"
cp app/build/outputs/apk/release/app-release.apk "$out"
info "APK: $out"
if [[ ! -f keystore.properties ]]; then
  echo "note: signed with the debug key; run scripts/release.sh for a dedicated release key"
fi
