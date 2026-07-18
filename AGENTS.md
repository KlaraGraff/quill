# Lantern Agent Guide

This file is the repo-wide guide for any coding assistant working on Lantern. Keep shared conventions here instead of putting them only in a tool-specific file such as `CLAUDE.md`.

## Product Context

Lantern is an AI-powered desktop ebook reader. It focuses on reading EPUB/PDF books, preserving a local library, and augmenting reading with AI lookup, explanations, translation, vocabulary, bookmarks, highlights, collections, and cross-device sync.

Core vocabulary:

- **Book**: a library item backed by an EPUB or PDF file.
- **Reader**: the reading surface for a book, including progress, layout, highlights, bookmarks, and AI panels.
- **Library**: the local SQLite materialized view plus book/cover blobs under the active data directory.
- **Sync**: iCloud-backed event-log sync for library state and shared book/cover files.
- **MCP**: Lantern's local MCP server/client integration surface for AI tools to inspect or modify the library.

## Stack

- Frontend: React 19, TypeScript, Tailwind CSS 4, Vite, React Router.
- Desktop/backend: Tauri 2, Rust, SQLite via `rusqlite`.
- Reader engine: `foliate-js` under `public/foliate-js`.
- Sync: iCloud container files, append-only event logs, snapshots, and watcher-driven replay.
- AI: OpenAI-compatible providers plus OAuth-backed OpenAI support.

## Project Map

- `src/`: React frontend.
- `src/components/`: shared UI, reader controls, settings, and library components.
- `src/components/settings/`: settings modal sections.
- `src/components/ui/`: common UI primitives.
- `src/pages/`: route-level screens such as library, reader, chats, vocabulary, and translations.
- `src/hooks/`: frontend data hooks and command wrappers.
- `src/i18n/`: translation JSON files.
- `src-tauri/src/commands/`: Tauri command handlers exposed to the frontend.
- `src-tauri/src/sync/`: iCloud sync engine, event log, peer manifests, replay, snapshots, and writer.
- `src-tauri/src/mcp/`: MCP server and tools.
- `src-tauri/src/ai/`: AI provider integrations.
- `public/foliate-js/`: vendored reader engine source, maintained with Lantern.
- `design/`: Pencil source files, including `design/quill-desktop.pen`.
- `docs/arch/`: architecture references.
- `docs/features/`: in-progress feature specs; shipped specs live in `docs/features/archive/`.
- `docs/impls/`: implementation plans; shipped plans live in `docs/impls/archive/`.
- `docs/guide/` and `docs/roadmap/`: user-facing guides and product planning notes.

## Working Copy

- The canonical local clone is `~/vibecoding/Lantern`. Work there.
- Do not create additional clones of this repo. Parallel clones drift — each keeps its own `main`, and agents working in different clones act on different worlds. If you are running in a clone other than the canonical one, say so and stop rather than working around it.
- Start every session with `git fetch origin && git status`. Another agent or session may have moved `main` since your context was built.
- More than one assistant works in this repo. If the working tree holds changes you did not make, they are probably another agent's in-flight work: inspect and preserve them. Do not revert, stash, or commit them as your own.

## Development Commands

- Install deps: `npm ci`.
- Start frontend dev server: `npm run dev`.
- Start Tauri app in dev: `npm run tauri dev`.
- Frontend typecheck: `npx tsc --noEmit`.
- Frontend lint: `npm run lint`.
- Frontend build: `npm run build`.
- Frontend unit tests: `npm run test:unit`.
- Rust check: `cd src-tauri && cargo check`.
- Rust tests: `cd src-tauri && cargo test`.
- Rust lint: `cd src-tauri && cargo clippy -- -D warnings`.
- Package desktop app: `npm run package`.

Prefer the smallest check that covers the change. For frontend changes, run typecheck and lint. For Rust behavior, run the relevant `cargo test` target; for sync changes, run sync-focused tests before broadening.

## Engineering Conventions

- Follow existing local patterns before adding new abstractions.
- Keep changes scoped to the request. Avoid unrelated refactors.
- Do not revert user changes. If the working tree is dirty, inspect first and preserve unrelated edits.
- Use structured APIs and parsers when available instead of ad hoc string manipulation.
- Keep comments rare and useful. Explain non-obvious intent, not mechanics.
- Keep UI aligned with `design/quill-desktop.pen` when a node or frame is referenced by the user.
- Keep `src/i18n/en.json` and `src/i18n/zh.json` in sync when adding user-facing strings.
- Use `ROW_CONTROL_WIDTH` or `ROW_CONTROL_WIDTH_COMPACT` for row controls in settings instead of local width literals.
- Treat sync and file-copy changes as data-safety sensitive. Preserve the invariant that the app must not repoint storage or disable sync until required local files are actually reachable.
- Do not add repo conventions only to an agent-specific file. Update this file and leave tool-specific files as compatibility entrypoints if needed.
- **Testing-stage compatibility:** do not add compatibility, migration, or rollback code for old app versions, old data, or historical schemas. After a data-model change, re-importing or resetting local test data is acceptable. Make an exception only when the user explicitly requires compatibility. This testing-stage policy ceases to apply once the user reports that the app has entered large-scale distribution; then assess compatibility, migration, and rollback needs based on data safety and upgrade experience.
- **Implementation judgment:** optimize for the requested capability rather than mechanically following a proposed implementation path. When an alternative achieves the same goal with materially better complexity, reliability, maintenance cost, or user experience, propose it briefly with its key tradeoff, and prefer it unless the user has explicitly required a particular path.

## Response Style

- Default to conclusion-first responses with the minimum sufficient information.
- For design alignment, retain only the conclusion, key rules, exceptions, and next step.
- Prefer compact Markdown tables for multiple rules or comparisons. Use short headers and one point per cell.
- Answer simple questions directly; do not force a table where prose is clearer.
- Use bold only to emphasize conclusions, conditions, and thresholds.
- By default, keep a response to one conclusion paragraph and one table. Add detail only when needed to preserve implementation-relevant boundary conditions, risks, or open questions.
- Do not repeat background, restate a point, or expand self-evident reasoning.

## Commit And PR Conventions

- **Default: commit straight to `main`.** This is a single-maintainer repo with no branch protection and no reviewers, so a PR adds a round trip without adding a reader. Run the checks that cover your change (see Development Commands), commit, push. CI runs on pushes to `main` and catches what you missed.
- Open a branch and PR only when the change is large or risky enough that you want CI to gate it *before* it lands, or when the user asks for one.
- **If you open a PR, carry it to done in the same turn:** wait for CI, merge when green, delete the branch, update local `main`. A green PR left open is an unfinished task, not a delivered one.
- Do not end a turn on "should I push?". If the checks pass and the change is what was asked for, push it. Stop and ask only when a check fails, the diff grew beyond what was asked, or the change is genuinely irreversible.
- Use focused commits with an imperative subject.
- Common scopes: `sync`, `commands`, `reader`, `library`, `settings`, `ai`, `mcp`, `ui`, `docs`, `release`.
- Example: `fix(sync): keep status reads off the webview thread`.
- Keep PR descriptions current when scope changes.
- Do not add tool-specific co-author trailers unless the user explicitly asks.

## Release Conventions

- **Never reuse a published version number.** Once a tag's artifacts have been downloadable, that version is burned — even when replacing a broken release, bump the patch version instead. Reused numbers produce identically named artifacts with different contents: on 2026-07-17 three different `Lantern_2.0.0_aarch64.dmg` binaries existed within hours, and telling them apart cost a full debugging round.
- **Identify builds by commit, not by filename.** Settings → About shows the running build's commit, build time, and channel (`app_build_info` command), with one-click diagnostics copy. Any bug report, acceptance run, or "which build am I on?" question starts by recording that commit.
- **Verify the released artifact, not just the CI status.** After publishing, download the actual asset from the release page and check it: expected size, About-page commit matches the tag, and on macOS whether Gatekeeper accepts it (`spctl -a -vv`). A green CI run only proves the build ran. Note: ad-hoc-signed artifacts (the current default — no signing secrets configured) are reported as "damaged" by macOS Gatekeeper on quarantined downloads; see `docs/impls/macos-distribution-gatekeeper-fix.md` for the distribution plan and the `xattr` workaround that unsigned releases must document in their notes.

## Notes For Agent Runtimes

This repository is intentionally agent-agnostic. Claude Code, Codex, or any other assistant should read `AGENTS.md` as the shared guide. Portable workflow skills live under `.agents/skills`. Tool-specific instruction/config files may exist only as compatibility entrypoints and should point back here when they contain shared guidance.
