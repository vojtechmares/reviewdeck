#!/bin/bash
#
# Cuts a release. This script only creates and pushes a tag - the release
# workflow on GitHub does the building, packaging and publishing.
#
#   ./scripts/release.sh                  next version from the commit log (svu next)
#   ./scripts/release.sh patch            force a patch, minor or major bump
#   ./scripts/release.sh prerelease       bump the prerelease portion
#   ./scripts/release.sh v1.2.3           an explicit version, no svu needed
#
# Flags:
#   -n, --dry-run   work out the version, print it, change nothing
#   -y, --yes       do not ask for confirmation
#   -h, --help      this message
#
# Environment:
#   REMOTE          git remote to push to (default: origin)
#   RELEASE_BRANCH  branch releases are cut from (default: main)
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REMOTE="${REMOTE:-origin}"
RELEASE_BRANCH="${RELEASE_BRANCH:-main}"
DRY_RUN=0
ASSUME_YES=0
BUMP=""

die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

usage() {
	sed -n '3,19p' "$0" | sed 's/^# \{0,1\}//'
}

while (($#)); do
	case "$1" in
	-n | --dry-run) DRY_RUN=1 ;;
	-y | --yes) ASSUME_YES=1 ;;
	-h | --help)
		usage
		exit 0
		;;
	-*) die "unknown flag: $1" ;;
	*)
		if [[ -n "$BUMP" ]]; then
			die "unexpected argument: $1"
		fi
		BUMP="$1"
		;;
	esac
	shift
done

git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repository"

echo "==> Checking the working tree"
[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty - commit or stash first"

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BRANCH" == "$RELEASE_BRANCH" ]] ||
	die "on '$BRANCH', releases are cut from '$RELEASE_BRANCH' (override with RELEASE_BRANCH)"

git remote get-url "$REMOTE" >/dev/null 2>&1 || die "no such remote: $REMOTE"

echo "==> Fetching $REMOTE"
git fetch --tags --prune "$REMOTE"

UPSTREAM="$REMOTE/$RELEASE_BRANCH"
git rev-parse --verify --quiet "$UPSTREAM" >/dev/null || die "$UPSTREAM does not exist"
[[ "$(git rev-parse HEAD)" == "$(git rev-parse "$UPSTREAM")" ]] ||
	die "$BRANCH and $UPSTREAM have diverged - push or pull first"

# svu reads the commit log to work out the bump. An explicit version does not
# need it, so it is only required for the derived cases.
need_svu() {
	command -v svu >/dev/null ||
		die "svu is not installed - brew install caarlos0/tap/svu, or pass an explicit version"
}

case "$BUMP" in
"" | next)
	need_svu
	VERSION="$(svu next)"
	;;
patch | minor | major | prerelease)
	need_svu
	VERSION="$(svu "$BUMP")"
	;;
v[0-9]* | [0-9]*) VERSION="v${BUMP#v}" ;;
*) die "expected patch, minor, major, prerelease or a version like v1.2.3, got '$BUMP'" ;;
esac

[[ "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
	die "'$VERSION' is not a semver version"

CURRENT="$(git describe --tags --abbrev=0 2>/dev/null || echo "v0.0.0")"

if [[ "$VERSION" == "$CURRENT" ]]; then
	die "no commits since $CURRENT ask for a new version - pass patch, minor or major to force one"
fi

if git rev-parse --verify --quiet "refs/tags/$VERSION" >/dev/null; then
	die "tag $VERSION already exists locally"
fi
if [[ -n "$(git ls-remote --tags "$REMOTE" "refs/tags/$VERSION")" ]]; then
	die "tag $VERSION already exists on $REMOTE"
fi

echo
echo "  $CURRENT -> $VERSION"
echo
if git rev-parse --verify --quiet "refs/tags/$CURRENT" >/dev/null; then
	git --no-pager log --oneline --no-decorate "$CURRENT..HEAD" | sed 's/^/  /'
else
	git --no-pager log --oneline --no-decorate -20 | sed 's/^/  /'
fi
echo

if ((DRY_RUN)); then
	echo "==> Dry run, stopping here"
	exit 0
fi

if ! ((ASSUME_YES)); then
	[[ -t 0 ]] || die "not a terminal - pass --yes to release without confirmation"
	read -r -p "Tag $VERSION and push to $REMOTE? [y/N] " REPLY
	[[ "$REPLY" =~ ^[Yy]$ ]] || die "aborted"
fi

echo "==> Tagging $VERSION"
git tag -a "$VERSION" -m "$VERSION"

echo "==> Pushing $VERSION to $REMOTE"
if ! git push "$REMOTE" "refs/tags/$VERSION"; then
	git tag -d "$VERSION" >/dev/null
	die "push failed, removed the local tag again"
fi

REPO_URL="$(git remote get-url "$REMOTE")"
REPO_URL="${REPO_URL%.git}"
REPO_URL="${REPO_URL/git@github.com:/https://github.com/}"
echo
echo "==> Released $VERSION"
echo "    The release workflow is building it: $REPO_URL/actions/workflows/release.yml"
