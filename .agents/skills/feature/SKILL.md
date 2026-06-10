---
name: feature
description: Create, list, or manage feature specs and GitHub issues
---

# Feature Management

Manage Quill's feature pipeline: specs live in `docs/features/`, tracking lives in GitHub Issues with the `feature` label.

## Usage

`/feature <action> [args]`

### Actions

#### `new <name>`
Create a new feature from scratch.

1. Ask the user to describe the feature (motivation, scope, key decisions).
2. **Pick a priority** (see Priority below). If the user didn't state one, propose one and confirm before filing. Don't file unlabeled.
3. Create a GitHub Issue with labels `feature` and the chosen `P0`/`P1`/`P2`/`P3`:
   - Title: `feat: <short description>`
   - Body: Motivation, Scope summary, Implementation Phases, and a placeholder note that the spec path will be added after the issue number is assigned.
   - Command shape: `gh issue create --label feature --label P1 --title "…" --body "…"`
4. Use the GitHub issue number as the spec number. Create `docs/features/{issue-number}-{slug}.md` with sections: Motivation, Scope, Implementation Phases, Verification, and a `GitHub issue: <issue-url>` line near the top.
5. Edit the GitHub Issue body to replace the placeholder with the final spec path. The issue body and spec must cross-link each other.
6. Update `docs/features/README.md` to include the new spec.
7. Report the spec path, issue URL, and assigned priority.

#### `list`
Show all features and their status.

1. Fetch open features with priority + metadata as JSON so they can be sorted: `gh issue list --label feature --state open --limit 50 --json number,title,labels,createdAt,assignees`
2. List all spec files in `docs/features/` (excluding README).
3. Sort the rows by priority (`P0` first, then `P1`, `P2`, `P3`, then unlabeled-by-priority last). Within a priority bucket, sort by spec number ascending.
4. Present a combined view: **Priority**, feature name, **#** (issue), spec file (if exists), **Created**. Issues without a P-label show `—` in priority and a callout asking the user to triage them.
5. If the user asks for closed/shipped features too, repeat with `--state all` and add a **State** column.

#### `close <issue-number>`
Mark a feature as shipped.

1. Close the GitHub Issue: `gh issue close <number>`
2. If a matching spec file exists in `docs/features/`, move it to `docs/features/archive/` with `git mv`. Shipped code is the source of truth, but the spec stays around as the "what we were going for" record.
3. Update `docs/features/README.md` to drop the archived entry from the index.

#### `prioritize <issue-number> <P0|P1|P2|P3>`

Set or change the priority of an existing feature.

1. Remove any existing P-label on the issue, then add the new one: `gh issue edit <number> --remove-label P0 --remove-label P1 --remove-label P2 --remove-label P3 --add-label <priority>` (Removing all four is safe — `gh` ignores remove-label for labels not present.)
2. Confirm the new priority.

#### `spec <issue-number-or-name>`
Open or create a spec for an existing feature issue.

1. If a spec file already exists, show its path.
2. If not, fetch the issue, use the issue number as the spec number, and create `docs/features/{issue-number}-{slug}.md` following the same format as `new`, pre-populated from the issue body.
3. Edit the issue body to include the new spec path if it does not already reference it.

## Labels

- `feature` — all feature issues use this label
- `bug` — for bug reports (not managed by this skill)
- `P0` / `P1` / `P2` / `P3` — priority, exactly one per issue

## Priority

Every feature gets exactly one priority label. Rubric:

- **P0** — Required for the next ship; nothing else moves until this lands. Rare for features.
- **P1** — Wanted this cycle; blocks an active user workflow or has a stakeholder commitment.
- **P2** — Real product win, but no urgency. Pick up when the P1 queue is clear.
- **P3** — Idea / nice-to-have / "if we ever revisit X." OK to sit indefinitely; closing as `wontfix` later is fine.

When in doubt between two levels, pick the lower-urgency one and say why; over-labeling P0/P1 dilutes the signal.

## Conventions

- Spec files are numbered by their GitHub issue number: `276-reset-all-data.md` for issue `#276`. The heading must use the same number.
- Slugs are lowercase kebab-case derived from the feature name.
- Specs for shipped features move to `docs/features/archive/` — the implementation is the source of truth, but the spec stays as the "what we were going for" record.
- The `docs/features/README.md` index only lists in-progress/planned specs.

## Notes

- Do not commit or push unless the user explicitly asks.
- When creating issues, always update the issue body with the final spec file path after the issue number is known.
- When creating specs, always include a reference to the GitHub issue URL.
