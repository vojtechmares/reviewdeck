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
EXECUTABLE="Reviewdeck"
# LSMinimumSystemVersion, and what the cask's `depends_on macos: :monterey` promises.
MIN_OS="12.0"
VERSION="${VERSION:-}"
VERSION="${VERSION#v}"

fail() {
	# GitHub renders ::error:: as an annotation; elsewhere it is just a line.
	printf '::error::%s\n' "$*" >&2
	exit 1
}

PLIST="$APP/Contents/Info.plist"
EXE="$APP/Contents/MacOS/$EXECUTABLE"

[[ -d "$APP" ]] || fail "no app bundle at $APP"
[[ -f "$PLIST" ]] || fail "$APP has no Info.plist"
[[ -x "$EXE" ]] || fail "$APP has no executable"

echo "==> $APP"

plutil -lint "$PLIST" >/dev/null || fail "Info.plist is malformed"

plist() {
	/usr/libexec/PlistBuddy -c "Print :$1" "$PLIST" 2>/dev/null || true
}

STAMPED_ID="$(plist CFBundleIdentifier)"
echo "    bundle id: $STAMPED_ID"
[[ "$STAMPED_ID" == "$APP_ID" ]] || fail "bundle id is $STAMPED_ID, expected $APP_ID"

# LaunchServices starts whatever CFBundleExecutable names; a mismatch is a
# bundle that Finder shows and refuses to open.
STAMPED_EXE="$(plist CFBundleExecutable)"
[[ "$STAMPED_EXE" == "$EXECUTABLE" ]] ||
	fail "CFBundleExecutable is '$STAMPED_EXE', expected '$EXECUTABLE'"

STAMPED_VERSION="$(plist CFBundleShortVersionString)"
echo "    version: $STAMPED_VERSION"
[[ -n "$STAMPED_VERSION" ]] || fail "Info.plist has no CFBundleShortVersionString"
if [[ -n "$VERSION" && "$STAMPED_VERSION" != "$VERSION" ]]; then
	fail "bundle says $STAMPED_VERSION, tag says $VERSION"
fi
if grep -q '@[A-Z_]*@' "$PLIST"; then
	fail "Info.plist still has an unfilled template placeholder"
fi

ICON="$(plist CFBundleIconFile)"
[[ -f "$APP/Contents/Resources/${ICON%.icns}.icns" ]] ||
	fail "CFBundleIconFile names '$ICON', which is not in Contents/Resources"

ARCHS="$(lipo -archs "$EXE")"
echo "    architectures: $ARCHS"
[[ "$ARCHS" == "arm64" ]] || fail "expected an arm64 binary, got $ARCHS"

# The deployment target is baked into the binary; one newer than the plist
# claims is an app that installs on Monterey and then will not start.
BINARY_MIN_OS="$(vtool -show-build "$EXE" 2>/dev/null | awk '$1 == "minos" { print $2; exit }')"
echo "    minimum macOS: ${BINARY_MIN_OS:-unknown}"
[[ -n "$BINARY_MIN_OS" ]] || fail "could not read the binary's minimum macOS version"
if [[ "$(printf '%s\n%s\n' "$BINARY_MIN_OS" "$MIN_OS" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)" != "$MIN_OS" ]]; then
	fail "the binary needs macOS $BINARY_MIN_OS, the bundle promises $MIN_OS"
fi

# The hardened runtime's library validation is on (the entitlements do not
# disable it), which only works while everything the executable loads is
# Apple's. A third-party dylib would be refused at launch on a user's machine.
FOREIGN="$(otool -L "$EXE" | tail -n +2 | awk '{ print $1 }' |
	grep -v -e '^/System/Library/' -e '^/usr/lib/' || true)"
[[ -z "$FOREIGN" ]] || fail "the executable links non-system libraries: $FOREIGN"

codesign --verify --strict "$APP" || fail "$APP is not validly signed"

# The signature has to cover the app's own identity - a linker-signed-only
# executable reports "Identifier=reviewdeck-<hash>" and seals neither
# Info.plist nor the resources, and macOS will refuse to launch it once it is
# quarantined.
SIGNATURE="$(codesign -dv "$APP" 2>&1)"
SIGNED_ID="$(printf '%s\n' "$SIGNATURE" | sed -n 's/^Identifier=//p')"
[[ "$SIGNED_ID" == "$APP_ID" ]] || fail "signed as '$SIGNED_ID', expected '$APP_ID'"

# flags=0x10002(adhoc,runtime): ad-hoc, and with the hardened runtime.
FLAGS="$(printf '%s\n' "$SIGNATURE" | sed -n 's/^CodeDirectory .*flags=[^(]*(\([^)]*\)).*/\1/p')"
[[ ",$FLAGS," == *",adhoc,"* ]] || fail "expected an ad-hoc signature, flags are '$FLAGS'"
[[ ",$FLAGS," == *",runtime,"* ]] || fail "the hardened runtime is off, flags are '$FLAGS'"
echo "    signature: ad-hoc, hardened runtime, sealed as $SIGNED_ID"

echo "==> OK"
