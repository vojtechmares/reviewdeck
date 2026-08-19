#!/bin/bash
#
# Puts the release signing certificate somewhere codesign can reach it, and
# takes it out again afterwards. Both workflows run this before packaging.
#
#   ./scripts/import-signing-cert.sh            # import and trust
#   ./scripts/import-signing-cert.sh --remove   # undo it
#
# Environment:
#   CSC_LINK          base64 of the .p12, from the repository secret
#   CSC_KEY_PASSWORD  its password
#
# A fork cannot read the secrets, so an empty CSC_LINK is not an error: the
# build falls back to ad-hoc signing, which scripts/signing-identity.sh decides
# by looking at the keychain this script just did or did not populate.
#
# The certificate is imported into a keychain of its own rather than the login
# keychain, so cleanup is a single delete and nothing lingers on a self-hosted
# runner. Trust, though, is not a property of a keychain - it lives in a trust
# domain and has to be granted separately, or `security find-identity -v` calls
# the identity CSSMERR_TP_NOT_TRUSTED and electron-builder never finds it.
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
CERT="$ROOT/resources/reviewdeck-self-signed.pem"
KEYCHAIN="${RUNNER_TEMP:-/tmp}/reviewdeck-signing.keychain-db"
P12="${RUNNER_TEMP:-/tmp}/reviewdeck-signing.p12"

fail() {
	printf '::error::%s\n' "$*" >&2
	exit 1
}

sha1_of() {
	openssl x509 -noout -fingerprint -sha1 | sed 's/.*=//' | tr -d ':' | tr '[:upper:]' '[:lower:]'
}

if [[ "${1:-}" == "--remove" ]]; then
	rm -f "$P12"
	# The keychain is the marker for "this script ran here". Without it there is
	# nothing of ours to undo, and a developer machine that trusted the
	# certificate by hand keeps its own trust setting.
	if [[ ! -f "$KEYCHAIN" ]]; then
		echo "==> no signing keychain to remove"
		exit 0
	fi

	# The private key goes first. Everything after this point is tidying, and
	# none of it may fail a build that already has its artifacts.
	security delete-keychain "$KEYCHAIN" 2>/dev/null || true
	echo "==> signing keychain deleted"

	# Removing a trust setting asks for an authorisation that only an
	# interactive session can grant - unlike add-trusted-cert, which takes a
	# keychain and goes through under sudo - so on a runner it sits waiting for
	# a dialog nobody will ever click. Give it a moment and move on: what is
	# left behind is public material that signs nothing without the private key
	# deleted above, and the runner itself is thrown away minutes later.
	MARKER="${RUNNER_TEMP:-/tmp}/reviewdeck-trust-removed"
	rm -f "$MARKER"
	{
		if sudo -n security remove-trusted-cert -d "$CERT" >/dev/null 2>&1; then
			echo removed > "$MARKER"
		else
			echo refused > "$MARKER"
		fi
	} &
	REMOVER=$!
	# Off the job table, so killing it below does not print a "Killed: 9" notice
	# across the build log.
	disown "$REMOVER" 2>/dev/null || true
	for _ in 1 2 3 4 5; do
		[[ -f "$MARKER" ]] && break
		sleep 1
	done

	case "$(cat "$MARKER" 2>/dev/null)" in
	removed)
		echo "==> trust setting removed"
		;;
	refused)
		# No passwordless sudo, or nothing of ours in the admin trust domain -
		# a developer machine that trusted the certificate by hand lands here,
		# and its own trust setting is none of this script's business.
		echo "==> no admin trust setting of ours to remove"
		;;
	*)
		kill -9 "$REMOVER" 2>/dev/null || true
		pkill -9 -f "security remove-trusted-cert" 2>/dev/null || true
		echo "==> trust setting left in place - removing it wanted an interactive authorisation"
		;;
	esac
	rm -f "$MARKER"
	exit 0
fi

if [[ -z "${CSC_LINK:-}" ]]; then
	echo "==> no CSC_LINK in the environment - this build signs ad-hoc"
	exit 0
fi
[[ -n "${CSC_KEY_PASSWORD:-}" ]] || fail "CSC_LINK is set but CSC_KEY_PASSWORD is not"
[[ -f "$CERT" ]] || fail "no certificate at $CERT to check the secret against"

printf '%s' "$CSC_LINK" | base64 -d > "$P12" 2>/dev/null || fail "CSC_LINK is not valid base64"
chmod 600 "$P12"

# The secret and the committed certificate have to describe the same identity.
# If they drift - a rotated certificate, a secret pasted from the wrong item -
# every check downstream still passes while the artifact is signed by something
# nobody documented, so this is the place to catch it.
EXPECTED="$(sha1_of < "$CERT")"
ACTUAL="$(openssl pkcs12 -in "$P12" -nokeys -clcerts -passin env:CSC_KEY_PASSWORD 2>/dev/null |
	sha1_of 2>/dev/null || true)"
[[ -n "$ACTUAL" ]] || fail "could not read a certificate out of CSC_LINK - wrong CSC_KEY_PASSWORD, or a .p12 macOS cannot open"
[[ "$ACTUAL" == "$EXPECTED" ]] || fail "CSC_LINK holds certificate $ACTUAL but resources/reviewdeck-self-signed.pem is $EXPECTED"

KEYCHAIN_PASSWORD="$(openssl rand -base64 24)"
# A re-run of a failed job would otherwise trip over the keychain the last
# attempt left behind.
security delete-keychain "$KEYCHAIN" 2>/dev/null || true
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
# Without this the keychain re-locks after five minutes of idle and a long
# packaging run finds it shut.
security set-keychain-settings -lut 21600 "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"

# Prepend rather than replace: codesign searches the list, and dropping the
# login keychain from it breaks anything else on the runner that wanted it.
EXISTING="$(security list-keychains -d user | sed 's/[[:space:]]*"\(.*\)"/\1/')"
# shellcheck disable=SC2086
security list-keychains -d user -s "$KEYCHAIN" $EXISTING >/dev/null

security import "$P12" -k "$KEYCHAIN" -P "$CSC_KEY_PASSWORD" \
	-T /usr/bin/codesign -T /usr/bin/security >/dev/null
# codesign is non-interactive on a runner; without the partition list macOS
# would sit waiting for someone to click Allow.
security set-key-partition-list -S apple-tool:,apple:,codesign: \
	-s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null 2>&1

IDENTITY="$("$HERE/signing-identity.sh" --pinned)"

# Already trusted on a developer machine that trusted it by hand; only a fresh
# runner needs this, and it needs root because the system domain is where a
# trust setting applies to every keychain in the search list.
if ! security find-identity -v -p codesigning | grep -qF "\"$IDENTITY\""; then
	sudo -n security add-trusted-cert -d -r trustRoot -p codeSign \
		-k /Library/Keychains/System.keychain "$CERT" ||
		fail "could not trust $CERT - without it codesign reports CSSMERR_TP_NOT_TRUSTED and electron-builder signs nothing"
fi

rm -f "$P12"

security find-identity -v -p codesigning | grep -F "\"$IDENTITY\"" >/dev/null ||
	fail "$IDENTITY is still not a valid identity after importing and trusting it"

echo "==> imported $IDENTITY ($EXPECTED)"
