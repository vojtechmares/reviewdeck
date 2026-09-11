---
name: reviewdeck-release
description: Cut a Reviewdeck release from the local checkout - work out the next version with svu, tag it and push the tag so the release workflow builds, publishes and bumps the Homebrew cask. Use when the user asks to release, tag a release, cut a version, or bump the version of Reviewdeck.
---

# Reviewdeck release

Releasing is one script, `scripts/release.sh`. It only creates and pushes a
tag. Everything after that - tests, build, packaging, the GitHub release, the
Homebrew cask bump - is the `Release` workflow on GitHub, triggered by the tag.
Never create the tag by hand, never run `gh release create`, never touch the
tap: that is the workflow's job and doing it locally makes two releases fight.

## Quick start

```bash
./scripts/release.sh --dry-run          # what would happen, changes nothing
./scripts/release.sh --yes              # tag and push the version svu derives
./scripts/release.sh patch --yes        # force a bump: patch, minor or major
./scripts/release.sh v1.2.3 --yes       # an explicit version, svu not needed
```

`--yes` is required from an agent: the script asks for confirmation on a
terminal and refuses without one when stdin is not a terminal.

## Workflow

1. **Arguments.** The user may pass a bump (`patch`, `minor`, `major`,
   `prerelease`) or an explicit version (`v1.2.3`). Nothing means "derive it".
2. **Preconditions.** The script checks these itself and dies with a reason,
   but they are cheap to check up front and save a failed run:
   - clean working tree
   - on `main`, in sync with `origin/main` (push or pull first, ask if it is
     unclear which)
   - `svu` installed, unless the user gave an explicit version
     (`brew install caarlos0/tap/svu`)
3. **Dry run.** `./scripts/release.sh [bump] --dry-run`. It prints
   `current -> next` and the commits in between.
4. **Nothing to bump.** When svu finds no feat or fix since the last tag - only
   chore, docs, ci and the like - `svu next` returns the current version and the
   script dies with `no commits since vX.Y.Z ask for a new version`. Treat that
   as a **patch** release: rerun with `patch`. Only choose something else when
   the user said so.
5. **Confirm the version with the user** before tagging, showing the
   `current -> next` line and the commit list, unless the user already named
   the bump or version themselves. Pushing a tag publishes a release; it is not
   undone by deleting the tag.
6. **Release.** `./scripts/release.sh [bump] --yes`. The script prints the
   workflow URL at the end.
7. **Report.** Give the user the version and the link to the workflow run:

   ```bash
   gh run list --workflow release.yml --limit 1
   ```

   Optionally follow it with `gh run watch <id>`; the build takes several
   minutes. The cask in `vojtechmares/homebrew-tap` is bumped by the workflow's
   last job, so a green run means `brew upgrade --cask reviewdeck` works.

## When it goes wrong

- **Push failed.** The script deletes the local tag again; fix the cause and
  rerun.
- **Workflow failed after the tag was pushed.** Do not retag the same version.
  Fix the problem on `main` and cut the next patch.
- **Tag already exists.** Somebody released that version already; pick the next
  one or let svu derive it.
