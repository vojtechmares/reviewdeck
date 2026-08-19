#!/bin/bash
#
# Prints the code signing identity to hand electron-builder.
#
# The identity is pinned in electron-builder.yml so a build never picks up
# whatever else happens to sit in the keychain - on a maintainer machine that
# would be an Apple Development certificate, which is not what ships. A machine
# without the certificate, a fork's CI or a contributor's laptop, cannot produce
# that signature at all, and an unsigned arm64 bundle will not launch, so the
# fallback there is ad-hoc rather than nothing.
#
#   ./scripts/signing-identity.sh            # identity to sign with, or "-"
#   ./scripts/signing-identity.sh --pinned   # what the config asks for
#
# The chosen identity goes to stdout and the reasoning to stderr, so the whole
# thing fits inside a command substitution.
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG="${CONFIG:-$HERE/../electron-builder.yml}"

[[ -f "$CONFIG" ]] || {
	printf '::error::no electron-builder config at %s\n' "$CONFIG" >&2
	exit 1
}

# Only mac.identity is quoted in this file, so a line match is enough and the
# alternative is a YAML parser for one string.
PINNED="$(sed -n 's/^[[:space:]]*identity:[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$CONFIG" | head -1)"
[[ -n "$PINNED" ]] || {
	printf '::error::%s has no quoted mac.identity to pin\n' "$CONFIG" >&2
	exit 1
}

if [[ "${1:-}" == "--pinned" ]]; then
	printf '%s\n' "$PINNED"
	exit 0
fi

# -v lists identities that are valid *and* trusted. A self-signed certificate
# that has been imported but not trusted is reported as CSSMERR_TP_NOT_TRUSTED
# and left out - and electron-builder, which filters this same list, would not
# see it either, so matching its view is the point.
if security find-identity -v -p codesigning | grep -qF "\"$PINNED\""; then
	printf '%s\n' "$PINNED"
else
	printf '==> %s is not in the keychain, signing ad-hoc instead\n' "$PINNED" >&2
	printf '%s\n' "-"
fi
