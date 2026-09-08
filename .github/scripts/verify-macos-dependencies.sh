#!/usr/bin/env bash

set -euo pipefail

APP_PATH=${1:?Usage: verify-macos-dependencies.sh /path/to/App.app}
INFO_PLIST="${APP_PATH}/Contents/Info.plist"

if [ ! -f "${INFO_PLIST}" ]; then
  echo "::error::Missing app Info.plist at ${INFO_PLIST}"
  exit 1
fi

EXECUTABLE_NAME=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "${INFO_PLIST}")
EXECUTABLE_PATH="${APP_PATH}/Contents/MacOS/${EXECUTABLE_NAME}"

if [ ! -f "${EXECUTABLE_PATH}" ]; then
  echo "::error::Missing app executable at ${EXECUTABLE_PATH}"
  exit 1
fi

# For universal binaries, otool prints each architecture separately. Dependency
# rows are indented, while architecture header rows are not, so this extracts
# dependencies from every slice without mistaking the executable for a dylib.
NON_SYSTEM_DYLIBS=$(
  otool -L "${EXECUTABLE_PATH}" \
    | awk '/^[[:space:]]/ { print $1 }' \
    | awk '/^\// && !/^\/System\/Library\// && !/^\/usr\/lib\//'
)

if [ -n "${NON_SYSTEM_DYLIBS}" ]; then
  echo "::error::macOS bundle contains absolute non-system dylib references:"
  echo "${NON_SYSTEM_DYLIBS}"
  exit 1
fi

echo "All macOS dylib references are system-provided or bundle-relative."
