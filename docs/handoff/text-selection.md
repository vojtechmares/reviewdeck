# Handoff: selectable and copyable text in the native app

## The task

Make static text in the Rust/gpui app selectable with the mouse and copyable with Cmd+C,
the way it was in the Electron app. gpui draws text but has no selection for anything that
is not an input, so this has to be built in the app. The user finds not being able to copy
code, comments and titles annoying; it is the most visible regression of the rewrite.

## Context

- Branch `worktree-rewrite-it-in-rust` (worktree `.claude/worktrees/rewrite-it-in-rust`):
  the Rust + gpui 0.2.2 rewrite of the Electron app. The Electron sources (`src/`, `test/`)
  are still in the tree and are the specification for behaviour. README describes the
  native app.
- gpui does not provide this and is not going to: Zed builds selection itself, on top of
  gpui, in its own `markdown` crate (not published on crates.io, so it cannot be a
  dependency). Its `MarkdownElement` is the reference implementation to read:
  https://github.com/zed-industries/zed/blob/main/crates/markdown/src/markdown.rs
- Related Zed threads: #5236 (selectable hover text, fixed in Zed's code by #12918 and
  #14518), #16745 (REPL output not selectable, open), #43614 / #43242 (markdown preview
  selection), open PRs #56452 and #64112 (rich-text copy on top of it).

## What the Electron app allowed

Everything in the page was selectable as browser text, except what the TSX marks
`select-none`:

- diff gutters with line numbers, the `+`/`-` marker column and the hunk header rows
  (`src/renderer/src/components/DiffView.tsx`, lines ~578, 684, 795, 827);
- avatar initials (`src/renderer/src/components/ui/avatar.tsx`).

The selection colour is `::selection { background: var(--ring) }`
(`src/renderer/src/index.css` ~196), which is `colors.ring` in `crates/app/src/ui/theme.rs`.

Worth matching, in priority order:

1. **Diff code lines**: the main reason people copy.
2. **Markdown bodies**: pull request descriptions, thread comments, drafts.
3. **Pull request header text**: the title, branch names, repository.
4. Optional: other prose, such as check names, dialog text and card titles. Cards were
   `<button>`s, which browsers do not start a selection in, so leave the deck cards alone.

## How text is drawn today

These are the places the selection layer has to hook into. Read them first.

- `crates/app/src/ui/code.rs`: `code_line` returns a `StyledText` for one highlighted
  line. `WrappedCode` (around line 331) is a custom `Element` that wraps monospace code by
  columns the way Chromium does (`wrap_points`, around line 269). It shapes its own
  `ShapedLine`s, so it needs its own hit testing (index by column is easy: monospace).
- `crates/app/src/ui/diff_view.rs`: one virtualised `list()`. Rows are file headers, hunk
  headers, split/unified code rows (gutter + `WrappedCode`), inline `ThreadCard`s and
  drafts, and the comment composer. Gutter presses start the comment-range drag
  (`begin` / `extend` / `pick`). That must keep working and must win over text selection
  in the gutter.
- `crates/app/src/ui/markdown_view.rs`: `MarkdownView` renders a parsed `Document` into
  nested blocks. Paragraph-level text becomes one `StyledText` with runs, wrapped in
  `InteractiveText` when it holds links (`text_element`, around line 444). Code blocks go
  through `code.rs`. A link click must still open the link when the mouse did not drag.
  The `<summary>` toggle must still toggle.
- `crates/app/src/ui/pull_view.rs` around line 1052: the title is a `StyledText` with
  highlights. Branch and label badges are kit `Badge`s.
- `crates/app/src/ui/thread_view.rs` and `draft_view.rs` hold one `MarkdownView` per
  comment body.

gpui 0.2.2 APIs that make this possible (source under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-0.2.2/`):

- `StyledText::layout() -> &TextLayout` (`src/elements/text.rs:167`). `TextLayout` has
  `index_for_position`, `position_for_index`, `bounds`, `line_height`, `len` and `text`
  (lines 483-606). It is valid after prepaint, so use it in paint and in mouse handlers.
- `window.paint_quad` for the selection highlight, painted before the text.
- Mouse down/move/up through `window.on_mouse_event` or element listeners; `click_count`
  on `MouseDownEvent` gives double and triple click.
- `cx.write_to_clipboard(ClipboardItem::new_string(..))`.
- The Edit menu uses the text field's own actions: `text_input::{Copy, SelectAll}` with
  `OsAction`s (`crates/app/src/main.rs` ~437-443, `crates/app/src/ui/components/input.rs`
  ~54-150). A selectable region needs a focus handle and `on_action` handlers for those two
  actions, so Cmd+C and the menu item work. It also needs a key context, so Cmd+A selects
  within that region and not the whole window.

## Suggested design

Recommended rather than settled; check it against Zed's `MarkdownElement` before
committing to it.

- **A `Selection` model per surface**: one for each `MarkdownView`, one for the diff, one
  for the pull header. It holds `anchor` and `head` as `(fragment, byte offset)` in
  document order, plus a `selecting` flag.
- **A fragment registry, rebuilt each frame**: every selectable text element registers its
  laid-out text (a `TextLayout` or a `WrappedCode` layout), its bounds and its plain text,
  in document order. Hit testing goes by y to the fragment, then by x to the index. That
  lets a drag cross paragraphs, list items and diff rows, which are separate elements.
- **Mouse**: down starts (unless on a gutter, a link press without drag, or a button);
  move extends; up ends. Double click selects a word, triple click a line in code or a
  block in prose. A plain click with no drag clears the selection.
- **Painting**: quads in `colors.ring` behind the selected range, per visual line,
  respecting `WrappedCode`'s wrap points.
- **Copy**: join fragments with `\n` between blocks and lines. In the diff copy only code
  content: no gutter numbers or markers, matching Electron's `select-none`.
- **Split diff**: decide whether a drag stays in the column where it started (GitHub's
  behaviour, and what people expect when copying code) or interleaves both columns (what
  the browser did with a table). Recommend staying in the column; confirm with the user.
- **Virtualisation**: the diff `list()` drops off-screen rows. Keep the selection in model
  coordinates (hunk, line, side, byte), not in registered fragments, so it survives
  scrolling and copy works for rows that are not on screen.

Pitfalls already known in this codebase are in the kit module docs
(`crates/app/src/ui/components/mod.rs`): `gap` needs `flex()`, never toggle `display` from
a hover style, hover colours do not reach already-shaped text, and a deferred element
inside another deferred element panics. Focus rings follow `:focus-visible` through
`components::focus_visible`, so a selectable region should not draw one.

## Verifying it

- **Unit and interaction tests**: gpui `TestAppContext` / `VisualTestContext` with
  `simulate_mouse_down/move/up`, then assert the selection model and the clipboard
  (`cx.read_from_clipboard()`). Existing tests show the harness:
  `crates/app/src/ui/components/tests.rs`, `diff_view.rs` and `markdown_view.rs` tests.
- **Visual check**: run in demo mode with a scratch data dir, never the user's real data:
  `REVIEWDECK_DATA_DIR=<scratch dir> REVIEWDECK_DEMO=1 cargo run -p reviewdeck`. The
  debug-only scene hook (`crates/app/src/scene.rs`, `REVIEWDECK_SCENE`) can be extended
  with a `select:` step to screenshot a selection deterministically.
- **Screenshots against Electron**: the previous session compared both apps with
  `screencapture -l <window id>`. Electron ran with `--user-data-dir=<scratch>` and was
  driven over `--remote-debugging-port`. Captures only work while the screen is unlocked
  and the window is visible: an occluded gpui window stops drawing and the capture is a
  stale frame.
- **Updates when done**: change the doc comment at the top of `markdown_view.rs` ("text
  selection is not available") and the README "Limitations" bullet about selecting text.

## The user's own data

The user runs the release build against a copy of their Electron data at
`~/Library/Application Support/reviewdeck-rust`:

```sh
./scripts/bundle.sh --dir-only
open -n --env "REVIEWDECK_DATA_DIR=$HOME/Library/Application Support/reviewdeck-rust" \
  release/mac-arm64/Reviewdeck.app
```

Do not point a development run at
`~/Library/Application Support/reviewdeck`: that is the Electron app's live vault, and the
Rust app would move its tokens into the Keychain and out of that file.

## Open threads from the rewrite, not part of this task

- Deleting the Electron sources was blocked by the permission classifier and is waiting on
  the user's decision.
- About 30 agent worktrees under `.claude/worktrees/` and large scratch build directories
  are waiting for the user's cleanup decision.
- A final full screenshot pass was interrupted by the screen locking.

## Suggested skills

- `tdd`: build the selection model test-first (hit testing, word and line expansion,
  copy text, the split-column rule).
- `run`: launch the app in demo mode to try the interaction by hand.
- `diagnose`: if selection fights the existing gutter drag or link clicks.
- `code-review` and `simplify`: before committing.
- `workflow-authoring`: only if the user asks to fan the work out across agents.
