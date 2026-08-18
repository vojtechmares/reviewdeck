# Domain docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

This is a **single-context** repo: one `CONTEXT.md` and one `docs/adr/` at the root, covering the whole app. The `src/main`, `src/preload`, `src/renderer` and `src/shared` split is an Electron process boundary, not a separate domain context.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root - the domain glossary.
- **`docs/adr/`** - read ADRs that touch the area you're about to work in.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The producer skill (`/grill-with-docs`) creates them lazily when terms or decisions actually get resolved.

## File structure

```
/
├── CONTEXT.md
├── docs/adr/
│   ├── 0001-....md
│   └── 0002-....md
└── src/
    ├── main/
    ├── preload/
    ├── renderer/
    └── shared/
```

If this repo ever grows genuinely separate contexts, switch to a `CONTEXT-MAP.md` at the root pointing at one `CONTEXT.md` per context, with context-scoped `docs/adr/` alongside each - and update this file.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal - either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/grill-with-docs`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) - but worth reopening because…_
