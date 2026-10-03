#!/usr/bin/env bash
# Full release: tests, signed APK, Termux/Linux arm64 package and checksums in dist/.
source "$(dirname "$0")/lib.sh"
ensure_android_env
cd "$ROOT"

VERSION="$(version)"
info "release $VERSION"

info "running tests"
cargo test --workspace --quiet

KEYSTORE_DIR="$ROOT/android/keystore"
PROPS="$ROOT/android/keystore.properties"
if [[ ! -f "$PROPS" ]]; then
  info "creating a release signing key in android/keystore (keep it: updates must use the same key)"
  mkdir -p "$KEYSTORE_DIR"
  PASS="$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24)"
  keytool -genkeypair -v -keystore "$KEYSTORE_DIR/release.jks" -alias localdebrid \
    -keyalg RSA -keysize 4096 -validity 10000 -storepass "$PASS" -keypass "$PASS" \
    -dname "CN=Local Debrid" >/dev/null 2>&1
  cat > "$PROPS" <<PROPS
storeFile=keystore/release.jks
storePassword=$PASS
keyAlias=localdebrid
keyPassword=$PASS
PROPS
fi

scripts/build-android.sh

info "building the Termux / Linux arm64 binary"
cargo ndk -t arm64-v8a --platform 26 build --release -p localdebrid
PKG="$ROOT/dist/localdebrid-termux-arm64-$VERSION"
rm -rf "$PKG" && mkdir -p "$PKG"
cp target/aarch64-linux-android/release/localdebrid "$PKG/"
cp scripts/termux-install.sh "$PKG/install.sh"
tar -C "$ROOT/dist" -czf "$PKG.tar.gz" "$(basename "$PKG")"
rm -rf "$PKG"

(cd dist && shasum -a 256 "LocalDebrid-$VERSION.apk" "$(basename "$PKG").tar.gz" > SHA256SUMS)
info "done:"
ls -lh dist
