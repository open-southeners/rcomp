#!/usr/bin/env bash
# Sign nested code that Tauri bundles but does not sign itself (the Quick Look
# extension, the bundled `rcomp` CLI): Tauri signs the app without --deep, so
# these must arrive signed.
#
#   macos-sign.sh [--entitlements FILE] [--identifier ID] PATH...
#
# Signs with the identity Tauri signs the app with, using the hardened runtime:
# $APPLE_SIGNING_IDENTITY when set, else the Developer ID in $APPLE_CERTIFICATE
# (imported into a throwaway keychain, as CI provides it), else ad-hoc.
set -euo pipefail

entitlements=""
identifier=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --entitlements) entitlements="$2"; shift 2 ;;
    --identifier) identifier="$2"; shift 2 ;;
    *) break ;;
  esac
done
if [[ $# -eq 0 ]]; then
  echo "usage: macos-sign.sh [--entitlements FILE] [--identifier ID] PATH..." >&2
  exit 2
fi

work="$(mktemp -d)"
identity="${APPLE_SIGNING_IDENTITY:-}"
keychain=""
search_list=()
cleanup() {
  if [[ ${#search_list[@]} -gt 0 ]]; then
    security list-keychains -d user -s "${search_list[@]}" >/dev/null 2>&1 || true
  fi
  if [[ -n "$keychain" ]]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

if [[ -z "$identity" && -n "${APPLE_CERTIFICATE:-}" ]]; then
  keychain="$work/signing.keychain-db"
  password="$(uuidgen)"
  security create-keychain -p "$password" "$keychain"
  security set-keychain-settings "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  base64 --decode <<<"$APPLE_CERTIFICATE" > "$work/certificate.p12"
  security import "$work/certificate.p12" -k "$keychain" \
    -P "${APPLE_CERTIFICATE_PASSWORD:-}" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain" >/dev/null
  # codesign resolves identities through the user's keychain search list;
  # `--keychain` alone is not honoured in headless CI sessions ("no identity
  # found"). Prepend ours, as Tauri does for the app, and restore on exit.
  while IFS= read -r kc; do
    kc="${kc#"${kc%%[![:space:]]*}"}"
    kc="${kc#\"}"
    search_list+=("${kc%\"}")
  done < <(security list-keychains -d user)
  security list-keychains -d user -s "$keychain" "${search_list[@]}"
  identity="$(security find-identity -v -p codesigning "$keychain" \
    | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) .*/\1/p' | head -1)"
  if [[ -z "$identity" ]]; then
    echo "macos-sign: no code-signing identity in APPLE_CERTIFICATE" >&2
    exit 1
  fi
fi

sign_args=(--force)
if [[ -n "$entitlements" ]]; then sign_args+=(--entitlements "$entitlements"); fi
if [[ -n "$identifier" ]]; then sign_args+=(--identifier "$identifier"); fi
if [[ -n "$identity" ]]; then
  sign_args+=(--sign "$identity" --options runtime --timestamp)
  if [[ -n "$keychain" ]]; then sign_args+=(--keychain "$keychain"); fi
else
  sign_args+=(--sign -)
fi
if ! codesign "${sign_args[@]}" "$@"; then
  echo "macos-sign: signing failed; code-signing identities visible to codesign:" >&2
  security find-identity -v -p codesigning >&2 || true
  exit 1
fi
echo "macos-sign: signed $*"
