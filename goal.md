# Goal

Build an electron desktop app (for macOS only for the MVP).

You are building this yourself, do not ask user for directions. Use web search to search for anwers, follow common and/or well-known patterns, this is an MVP, not the perfect product.

## MVP

Key features:

- Sign in to several Git providers with multiple accounts (GitHub.com, GitHub.com Enterprise, GitLab.com, self-hosted GitLab, Forgejo, BitBucket, etc.)
- Agregate all requested code reviews, send notifications to user about newly requested code reviews
- Code review UI to view the diff (side by side or single window) with integrations to comment on lines of code and to leave a comment at the PR/MR itself
- PR/MR approval or requested changes
- UI for PR/MR checks (GitHub, GitLab, etc.) status checks: Passed, Running, Failed, Unknown; for Running, poll the status

## Dependencies

Prefer to implement features yourself rather than downloading random NPM packages.

Only well-known packages are allowed like React, jose, etc.

The standing exception is parsing and rendering content that other people wrote.
Markdown in a pull request description or a comment arrives as untrusted input, and a
hand-written markdown parser and HTML sanitizer would be a security liability rather
than a saving. That work is taken as a dependency: `react-markdown` with `remark-gfm`,
`rehype-raw` and `rehype-sanitize`. Syntax highlighting the diff falls under the same
exception when it lands.

Everything else - the diff parser, the provider adapters, the UI components - stays
hand-written.

## Techstack

Electron app with React, keep it simple stupid. Do not reinvent the wheel.

### UI

Use shadcn/ui with clean look (rounded corners, netrual color palette)

### Design

Go for liquid glass look but more "milky tones" – not too transparent, just translusive.
