# Reviewdeck

A macOS desktop app that collects every code review waiting on you - across every Git host you
work with - into one deck, and lets you read the diff, comment on lines, and approve without
opening four browser tabs.

Built for the freelance situation: several Git organisations spread over GitHub.com, GitLab.com,
a client's self-hosted GitLab, a Forgejo instance and Bitbucket, with a different account on each.

![Reviewdeck](docs/screenshot.png)

## What it does

- **Multiple accounts, multiple hosts.** GitHub.com and GitHub Enterprise Server, GitLab.com and
  self-hosted GitLab, Forgejo and Gitea, Bitbucket Cloud. Add as many accounts as you like,
  including several on the same host.
- **One aggregated deck.** Everything where your review has been requested, newest activity first,
  filterable by account, check status or free text.
- **Native notifications** when a new review request appears, with a menu-bar count of what is
  still waiting.
- **A real diff viewer** - side by side or unified, with inline comments on any line and existing
  review comments anchored where they were left.
- **Threaded conversations.** Replies sit under what they answer, and where the host supports it
  you can reply and resolve without leaving the app.
- **Descriptions and comments as Markdown**, GitHub-flavoured and rendered through a
  sanitizer, so collapsible bot reports, tables and checklists read the way their author
  meant them to.
- **Approve or request changes** from the app, plus ordinary pull-request comments.
- **Hand a review to your agent.** One button copies a ready-to-paste Claude prompt - title,
  base branch, open threads and the exact fetch command for that host. Nothing is spawned: it
  is your own shell that runs it, so a shell alias works, and the command name is configurable
  per account.
- **CI status at a glance.** Passed / Running / Failed / Unknown per pull request, with the
  per-check breakdown, and a faster background poll while anything is still running.

## Install

Apple silicon, macOS 12 or later.

```sh
brew install --cask vojtechmares/tap/reviewdeck
```

Or take the `.dmg` from the [latest release](https://github.com/vojtechmares/reviewdeck/releases/latest)
and drag `Reviewdeck.app` into Applications.

Reviewdeck is ad-hoc signed but not notarised - there is no Apple Developer ID behind it - so
Gatekeeper will not launch a copy it saw arrive from the internet until the quarantine flag is
gone. The cask clears it for you; a manual download needs right-click → Open once, or:

```sh
xattr -dr com.apple.quarantine /Applications/Reviewdeck.app
```

## Running it

```sh
pnpm install
pnpm dev            # development, with hot reload
pnpm build          # typecheck + production bundle
pnpm dist           # ad-hoc signed .dmg and .zip in release/
pnpm dist:dir       # unpacked .app in release/mac-arm64/
pnpm test           # unit tests for the diff parser, the markdown pipeline and provider helpers
pnpm icon           # regenerate resources/icon.icns
```

The pnpm version is pinned in `package.json` under `packageManager`, so `corepack enable` is
enough to get the right one.

To poke at the UI without connecting a real account, run with `REVIEWDECK_DEMO=1` - the deck fills
with fixtures and the diff viewer renders a sample pull request.

## Signing in

Reviewdeck authenticates with **personal access tokens**, not OAuth. OAuth would need an
application registered up front on every instance, which is impossible for the self-hosted GitLab
or company Forgejo you were handed a login to last week. A token works everywhere, immediately.

Tokens are encrypted with Electron's `safeStorage`, which is backed by the macOS Keychain, and are
written to `~/Library/Application Support/reviewdeck/reviewdeck.json`. They never cross into the
renderer process - the UI only ever asks the main process to make a call on its behalf.

| Provider | Where to create one | Scopes needed |
| --- | --- | --- |
| GitHub / GHES | Settings → Developer settings → Personal access tokens | `repo`, `read:org` |
| GitLab | Settings → Access tokens | `api` |
| Forgejo / Gitea | Settings → Applications | issue: Read and write, repository: Read and write, user: Read |
| Bitbucket Cloud | Personal settings → App passwords | Account: Read, Pull requests: Write |

Bitbucket app passwords authenticate as a username/password pair, so it asks for your username too.

## How it fits together

```
src/
├── main/                 Node side: no UI, owns all network and all secrets
│   ├── index.ts          window, tray, menu, lifecycle
│   ├── deck.ts           the sync engine: fan-out, notifications, check polling
│   ├── store.ts          accounts + settings on disk, tokens via safeStorage
│   ├── http.ts           fetch wrapper: timeouts, pagination, readable errors
│   ├── ipc.ts            every channel the renderer may call
│   └── providers/        one adapter per host, behind a single interface
├── preload/index.ts      the contextBridge surface - the only way in
├── shared/               types, the diff parser and the markdown pipeline, used by both sides
└── renderer/src/         React UI
```

The renderer never talks to a Git host. It asks the main process, which holds the tokens and does
the HTTP. Context isolation is on, node integration is off, and the renderer runs under a CSP with
`connect-src 'self'`, so a malicious pull request title has nowhere to go.

The one exception is images, and it is not really one: a screenshot pasted into a private GitLab
merge request needs a token, and an image tag cannot carry a header. Those sources are pointed at
a `reviewdeck-image:` scheme the main process serves, which fetches them with the right account's
token and streams the bytes back. A request is only served when the host it names belongs to the
account it names, so no token can reach a host it does not belong to; everything else loads over
ordinary HTTPS with no credential involved.

### Adding a provider

Implement the `Provider` interface in `src/main/providers/types.ts` - connect, list review
requests, load a diff, refresh checks, submit a review, comment, comment on a line - and register
it in `src/main/providers/index.ts`. Everything above that layer is provider-agnostic.

Replying to a thread and resolving one are optional: leave them out on a host that cannot do
them, and say so through each thread's capability flags. `src/main/providers/threads.ts` is
where every host's comment shape becomes the one thread shape the renderer knows about, and it
takes no imports beyond types so it stays reachable from the test suite.

### Notes on the provider APIs

Each host makes a different part of this hard:

- **GitHub** aggregates cheaply (`search/issues?q=review-requested:@me`) but needs a follow-up call
  per pull request for the diff stats, and check runs and legacy commit statuses are two separate
  endpoints that both have to be merged.
- **GitLab** is the friendliest: `scope=reviews_for_me` does the aggregation server-side. In
  exchange, line comments need the base/start/head SHAs from the MR versions endpoint, and there is
  no single "submit review" call - approving and commenting are separate requests.
- **Forgejo / Gitea** has `review_requested=true` on its issue search, and attaches inline comments
  to a review rather than to the pull request.
- **Bitbucket** has no "awaiting my review" endpoint at all - `/pullrequests/{user}` returns what
  you *authored*. So it walks your workspaces, lists each repository's open pull requests and keeps
  the ones naming you as a reviewer, bounded to 120 repositories so a large account cannot stall a
  sync.

## Design

Milky glass: macOS vibrancy supplies the blur, and the renderer paints a heavy translucent film on
top so text stays crisp over any wallpaper. Neutral graphite palette, generous rounded corners,
colour reserved for status. The window is deliberately *not* `transparent: true` - that disables
vibrancy and reads as see-through rather than frosted.

## Limitations

This is an MVP.

- macOS only.
- Bitbucket Server (the self-hosted one) uses a different API and is not supported; Bitbucket Cloud is.
- Diffs are not syntax highlighted.
- Replying to a thread and resolving one work on GitLab. On GitHub, Forgejo and Bitbucket every
  comment still arrives as a thread of its own, with no reply or resolve offered - GitHub's
  review threads and their resolution live only in its GraphQL API.

Features are implemented here rather than pulled in, with one standing exception: parsing
and rendering content that other people wrote. Markdown goes through `react-markdown`,
`remark-gfm`, `rehype-raw` and `rehype-sanitize`, because a hand-written markdown parser
and HTML sanitizer standing between an untrusted pull request body and the app would be a
liability rather than a saving. Raw HTML is sanitized, not stripped, and the sanitized
tree becomes React elements - no HTML string is ever handed to the DOM.

## Releasing

Tags drive everything. `scripts/release.sh` only creates and pushes the tag; the
[release workflow](.github/workflows/release.yml) does the rest.

```sh
./scripts/release.sh              # next version from the commit log (needs svu)
./scripts/release.sh minor        # force a patch/minor/major bump
./scripts/release.sh v1.2.3       # an explicit version
./scripts/release.sh -n           # work out the version and stop
```

On a `v*` tag the workflow refuses anything that is not on `main`, stamps the version out of the
tag, tests, builds, packages, publishes a GitHub release carrying the `.dmg`, the `.zip` and
`checksums.txt`, and then rewrites `Casks/reviewdeck.rb` in
[vojtechmares/homebrew-tap](https://github.com/vojtechmares/homebrew-tap) to point at the new zip.
A prerelease tag (`v1.2.3-rc.1`) publishes the release but leaves the tap on the last stable
version.

The `version` in `package.json` is not the released version - the tag is, and the build stamps it
in - so cutting a release needs no commit.

The tap lives in another repository, which `github.token` cannot reach, so the workflow needs one
secret: `HOMEBREW_TAP_TOKEN`, a PAT with `contents: write` on the tap. `scripts/bump-cask.sh`
renders the cask and can be run by hand against a published release:

```sh
VERSION=1.2.3 SHA256=<sha256 of the zip> DRY_RUN=1 ./scripts/bump-cask.sh
```

## Licence

MIT
