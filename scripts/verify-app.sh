#!/bin/bash
#
# Sanity checks a packaged Reviewdeck.app before it is shipped. Both CI and the
# release workflow run this; the release workflow also sets VERSION, which adds
# a check that the bundle carries the version the tag asked for.
#
#   ./scripts/verify-app.sh [path/to/Reviewdeck.app]
#
# Environment:
#   VERSION   expected CFBundleShortVersionString (optional)
#
set -euo pipefail

APP="${1:-release/mac-arm64/Reviewdeck.app}"
APP_ID="cz.mares.reviewdeck"
VERSION="${VERSION:-}"

fail() {
	# GitHub renders ::error:: as an annotation; elsewhere it is just a line.
	printf '::error::%s\n' "$*" >&2
	exit 1
}

[[ -d "$APP" ]] || fail "no app bundle at $APP"
[[ -x "$APP/Contents/MacOS/Reviewdeck" ]] || fail "$APP has no executable"

echo "==> $APP"

ARCHS="$(lipo -archs "$APP/Contents/MacOS/Reviewdeck")"
echo "    architectures: $ARCHS"
[[ "$ARCHS" == "arm64" ]] || fail "expected an arm64 binary, got $ARCHS"

plutil -lint "$APP/Contents/Info.plist" >/dev/null || fail "Info.plist is malformed"

STAMPED_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")"
echo "    bundle id: $STAMPED_ID"
[[ "$STAMPED_ID" == "$APP_ID" ]] || fail "bundle id is $STAMPED_ID, expected $APP_ID"

STAMPED_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
echo "    version: $STAMPED_VERSION"
if [[ -n "$VERSION" && "$STAMPED_VERSION" != "$VERSION" ]]; then
	fail "bundle says $STAMPED_VERSION, tag says $VERSION"
fi

# --deep is deprecated for signing but is still the way to walk a bundle's
# nested code on verify, and Electron nests plenty of it.
codesign --verify --deep --strict "$APP" || fail "$APP is not validly signed"

# The signature has to cover the app's own identity, not Electron's default -
# an unsigned or linker-signed-only bundle reports "Identifier=Electron" and
# seals nothing, and macOS will refuse to launch it once it is quarantined.
SIGNED_ID="$(codesign -dv "$APP" 2>&1 | sed -n 's/^Identifier=//p')"
[[ "$SIGNED_ID" == "$APP_ID" ]] || fail "signed as '$SIGNED_ID', expected '$APP_ID'"
echo "    signature: ad-hoc, sealed as $SIGNED_ID"

echo "==> OK"
