#!/bin/bash
#
# Builds Reviewdeck.app from the Rust workspace and packages it the way the
# release ships it: an ad-hoc signed, hardened-runtime bundle, a .dmg for people
# and a .zip for the Homebrew cask. CI runs it with --dir-only; the release
# workflow runs it with the version from the tag.
#
#   ./scripts/bundle.sh                     version from the Cargo workspace
#   ./scripts/bundle.sh --version 1.2.3     stamp an explicit version
#   ./scripts/bundle.sh --dir-only          the .app only, no .dmg or .zip
#   ./scripts/bundle.sh --out dist          write somewhere other than release/
#
# Produces, under the output directory (default: release):
#   mac-arm64/Reviewdeck.app
#   Reviewdeck-<version>-arm64.dmg     unless --dir-only
#   Reviewdeck-<version>-arm64.zip     unless --dir-only
#
# Environment:
#   MACOSX_DEPLOYMENT_TARGET   defaults to 12.0, the bundle's LSMinimumSystemVersion
#   CARGO_TARGET_DIR           honoured, as cargo honours it
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PRODUCT="Reviewdeck"
PACKAGE="reviewdeck"
TARGET="aarch64-apple-darwin"
ARCH="arm64"
VERSION=""
DIR_ONLY=0
OUT="release"

die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

usage() {
	sed -n '3,21p' "$0" | sed 's/^# \{0,1\}//'
}

while (($#)); do
	case "$1" in
	--version)
		[[ $# -ge 2 ]] || die "--version needs a value"
		VERSION="$2"
		shift
		;;
	--version=*) VERSION="${1#--version=}" ;;
	--dir-only) DIR_ONLY=1 ;;
	--out)
		[[ $# -ge 2 ]] || die "--out needs a value"
		OUT="$2"
		shift
		;;
	--out=*) OUT="${1#--out=}" ;;
	-h | --help)
		usage
		exit 0
		;;
	*) die "unexpected argument: $1" ;;
	esac
	shift
done

[[ "$(uname -s)" == "Darwin" ]] || die "Reviewdeck.app can only be built on macOS"
[[ -n "$OUT" ]] || die "--out cannot be empty"

# The tag is the released version and the workflow passes it in. Without one,
# the bundle says what the workspace says. `cargo pkgid` prints
# path+file:///…/crates/app#reviewdeck@0.1.0, and the version is after the
# last # or @.
if [[ -z "$VERSION" ]]; then
	PKGID="$(cargo pkgid -p "$PACKAGE")" || die "cargo could not resolve the $PACKAGE package"
	VERSION="${PKGID##*[#@]}"
fi
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
	die "'$VERSION' is not a semver version"
BUILD_VERSION="${VERSION%%-*}"

APP_DIR="$OUT/mac-$ARCH"
APP="$APP_DIR/$PRODUCT.app"
DMG="$OUT/$PRODUCT-$VERSION-$ARCH.dmg"
ZIP="$OUT/$PRODUCT-$VERSION-$ARCH.zip"

# The bundle claims macOS 12 in LSMinimumSystemVersion and the cask depends on
# Monterey, so the binary must not quietly require anything newer.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"

echo "==> Building $PACKAGE $VERSION for $TARGET"
# The JSON messages name the executable wherever it landed, which spares this
# script from second-guessing CARGO_TARGET_DIR or a build.target-dir config.
# Diagnostics still go to the terminal, rendered.
BUILD_LOG="$(cargo build --release -p "$PACKAGE" --target "$TARGET" \
	--message-format=json-render-diagnostics)" || die "cargo build failed"
BINARY="$(printf '%s\n' "$BUILD_LOG" |
	grep '"reason":"compiler-artifact"' |
	sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' |
	tail -n 1)"
[[ -n "$BINARY" && -x "$BINARY" ]] || die "cargo did not report the $PACKAGE executable"

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BINARY" "$APP/Contents/MacOS/$PRODUCT"
cp resources/icon.icns "$APP/Contents/Resources/icon.icns"
# Type and creator code. Nothing modern reads it, but Finder still expects it
# next to Info.plist in an application bundle.
printf 'APPL????' >"$APP/Contents/PkgInfo"

# LC_ALL=C: sed handles the UTF-8 copyright line as plain bytes. plutil then
# rewrites the result as canonical XML, which validates it and drops the
# template's comments.
PLIST="$APP/Contents/Info.plist"
LC_ALL=C sed \
	-e "s/@VERSION@/$VERSION/g" \
	-e "s/@BUILD_VERSION@/$BUILD_VERSION/g" \
	-e "s/@YEAR@/$(date +%Y)/g" \
	resources/Info.plist >"$PLIST"
plutil -convert xml1 "$PLIST" || die "Info.plist is malformed"
if grep -q '@[A-Z_]*@' "$PLIST"; then
	die "Info.plist still has an unfilled placeholder"
fi

# Ad-hoc, always. There is no Developer ID certificate, and pinning the identity
# keeps a local build the same shape as the release workflow instead of quietly
# picking up whatever happens to be in the keychain. Signing the bundle (not
# just the executable) seals Info.plist and the resources, and takes the
# identifier from CFBundleIdentifier instead of the linker's "reviewdeck-<hash>".
# --options runtime is the hardened runtime; the entitlements file says why it
# grants nothing.
echo "==> Signing (ad-hoc, hardened runtime)"
codesign --force --sign - --timestamp=none --options runtime \
	--entitlements resources/entitlements.mac.plist "$APP"
codesign --verify --strict "$APP" || die "$APP did not verify after signing"

if ((DIR_ONLY)); then
	echo "==> $APP"
	exit 0
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# The dmg is what a human downloads: the app next to a link to /Applications,
# so installing is one drag. ditto keeps the signature and extended attributes
# intact; cp -R does not promise to.
echo "==> Packaging $DMG"
rm -f "$DMG"
ditto "$APP" "$STAGE/$PRODUCT.app"
ln -s /Applications "$STAGE/Applications"
# hdiutil on CI runners now and then fails with "Resource busy" while something
# (Spotlight, XProtect) is still looking at the staging folder. A retry is the
# usual cure.
for ATTEMPT in 1 2 3; do
	if hdiutil create -quiet -volname "$PRODUCT $VERSION" -srcfolder "$STAGE" \
		-fs HFS+ -format UDZO -ov "$DMG"; then
		break
	fi
	((ATTEMPT < 3)) || die "hdiutil could not create $DMG"
	echo "    hdiutil failed, retrying ($ATTEMPT/3)"
	sleep 5
done

# The zip is what the Homebrew cask installs, so brew does not have to mount a
# disk image. --keepParent puts Reviewdeck.app at the root of the archive, and
# --sequesterRsrc keeps extended attributes in __MACOSX the way Finder does.
echo "==> Packaging $ZIP"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

echo "==> Done"
echo "    $APP"
echo "    $DMG"
echo "    $ZIP"
