#!/usr/bin/env bash
# Build RcompQuickLook.appex, the macOS Quick Look preview extension, into
# quicklook/build/. Run by Tauri as `beforeBundleCommand` on macOS (see
# src-tauri/tauri.macos.conf.json); the bundler then copies the extension into
# Rcomp.app/Contents/PlugIns/.
#
# - Builds the rcomp-quicklook static library for every installed Apple Rust
#   target (both, as the universal release build has) and links one Swift
#   binary per architecture, merged with lipo.
# - Signs with the identity Tauri signs the app with, using the hardened
#   runtime: $APPLE_SIGNING_IDENTITY when set, else the Developer ID in
#   $APPLE_CERTIFICATE (imported into a throwaway keychain, as CI provides it),
#   else ad-hoc. Tauri does not sign custom bundle files and signs the app
#   without --deep, so this nested code must arrive signed.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "quicklook: not macOS, skipping"
  exit 0
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
out="$here/build"
appex="$out/RcompQuickLook.appex"
min_macos="12.0"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

archs=()
installed="$(rustup target list --installed 2>/dev/null || true)"
for arch in aarch64 x86_64; do
  if grep -qx "$arch-apple-darwin" <<<"$installed"; then archs+=("$arch"); fi
done
if [[ ${#archs[@]} -eq 0 ]]; then
  archs=("$(uname -m | sed 's/arm64/aarch64/')")
fi

rm -rf "$appex" "$out/obj"
mkdir -p "$appex/Contents/MacOS" "$out/obj"

slices=()
for arch in "${archs[@]}"; do
  triple="$arch-apple-darwin"
  echo "quicklook: building for $triple"
  MACOSX_DEPLOYMENT_TARGET="$min_macos" cargo build --manifest-path "$root/Cargo.toml" \
    -p rcomp-quicklook --release --target "$triple" --locked
  swift_arch="${arch/aarch64/arm64}"
  slice="$out/obj/RcompQuickLook-$arch"
  xcrun swiftc -O \
    -target "$swift_arch-apple-macos$min_macos" \
    -application-extension \
    -module-name RcompQuickLook \
    -import-objc-header "$here/include/rcomp_quicklook.h" \
    "$here"/extension/*.swift \
    "$root/target/$triple/release/librcomp_quicklook.a" \
    -lc++ -liconv \
    -framework QuickLookUI -framework UniformTypeIdentifiers \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -o "$slice"
  slices+=("$slice")
done

lipo -create "${slices[@]}" -output "$appex/Contents/MacOS/RcompQuickLook"
sed "s/__VERSION__/$version/g" "$here/extension/Info.plist" > "$appex/Contents/Info.plist"

identity="${APPLE_SIGNING_IDENTITY:-}"
keychain=""
cleanup() {
  if [[ -n "$keychain" ]]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
  rm -f "$out/certificate.p12"
}
trap cleanup EXIT

if [[ -z "$identity" && -n "${APPLE_CERTIFICATE:-}" ]]; then
  keychain="$out/quicklook-signing.keychain-db"
  password="$(uuidgen)"
  security create-keychain -p "$password" "$keychain"
  security set-keychain-settings "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  base64 --decode <<<"$APPLE_CERTIFICATE" > "$out/certificate.p12"
  security import "$out/certificate.p12" -k "$keychain" \
    -P "${APPLE_CERTIFICATE_PASSWORD:-}" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
  identity="$(security find-identity -v -p codesigning "$keychain" \
    | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) .*/\1/p' | head -1)"
  if [[ -z "$identity" ]]; then
    echo "quicklook: no code-signing identity in APPLE_CERTIFICATE" >&2
    exit 1
  fi
fi

sign_args=(--force --entitlements "$here/extension/RcompQuickLook.entitlements")
if [[ -n "$identity" ]]; then
  sign_args+=(--sign "$identity" --options runtime --timestamp)
  if [[ -n "$keychain" ]]; then sign_args+=(--keychain "$keychain"); fi
else
  sign_args+=(--sign -)
fi
codesign "${sign_args[@]}" "$appex"
echo "quicklook: built $appex (${archs[*]})"
