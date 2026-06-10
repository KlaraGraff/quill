# 276 - Reset All App Data

GitHub issue: https://github.com/yicheng47/quill/issues/276

## Motivation

Quill has no way to wipe local data from within the app. Users who want a fresh start, need to recover from corrupted state, or want to remove personal data — books, highlights, vocabulary, chat history, reading progress, API keys — before handing off a machine have to manually locate and delete the app data directory.

A reset action in Settings makes this safe and discoverable. Because it is irreversible and destroys everything, it must be guarded by a double confirmation so it cannot be triggered by a stray click.

## Scope

In scope:

- A destructive **Reset all data** action in Settings (General section), styled as a danger row clearly separated from regular settings.
- Reset clears everything local:
  - `quill.db` — books, collections, highlights, bookmarks, vocabulary, chat history, reading progress, and settings (including its `-wal`/`-shm` sidecars).
  - `secrets.db` — API keys and OAuth tokens.
  - `books/` and `covers/` blob directories.
- **Double confirmation** before anything runs:
  1. A warning dialog itemizing exactly what will be deleted, with Cancel as the default action.
  2. A second step requiring the user to type a literal confirmation keyword (`RESET`) to enable the destructive button. The keyword stays literal across locales.
- If Library Sync is enabled, reset disables sync and clears local data only. The warning dialog states that the iCloud copy is not deleted and the data would re-download if sync is re-enabled.
- After reset, the app relaunches (or fully re-initializes) into a pristine first-launch state with default settings.

Out of scope:

- Deleting CloudKit/server-side sync data.
- Selective reset (only vocab, only chat history, etc.).
- Backup-before-reset — covered by [32 — Library Backup](32-library-backup.md).

## Implementation Phases

1. Backend `reset_app_data` command.
   - Tear down open database handles, delete `quill.db` (+ sidecars), `secrets.db`, and the `books/` and `covers/` directories under the app data dir.
   - Disable Library Sync state before deleting, so a relaunch does not immediately re-pull from iCloud.
   - Recreate empty directories and a fresh database so the app can re-initialize cleanly.
   - Unit tests: command removes all data files, succeeds when directories are partially missing, leaves the data dir in a fresh-install state.

2. Settings UI entry point.
   - Danger row at the bottom of the General section, following the standard 73px row pattern but with destructive (red) styling for the action button.

3. Double-confirm dialog flow.
   - Modal 1: warning with itemized deletion list, sync caveat when sync is enabled, Cancel as default.
   - Modal 2: type-to-confirm keyword gate; destructive button stays disabled until the keyword matches.
   - On confirm: invoke `reset_app_data`, then relaunch the app into the fresh state.

4. i18n + QA.
   - All strings in `en.json` / `zh.json`.
   - Verify reset from a populated library (books, highlights, vocab, chats, API keys configured) lands in a clean first-launch state, in both light and dark themes.

## Verification

- Reset row appears in General settings with destructive styling; activating it never deletes anything without completing both confirmation steps.
- Cancelling at either step leaves all data intact.
- Completing both steps removes `quill.db`, `secrets.db`, `books/`, and `covers/`, and the app relaunches with an empty library, default settings, and no stored API keys.
- With Library Sync enabled, the warning mentions the iCloud copy, and reset disables sync before clearing local data.
- All dialog and row strings are localized in English and Chinese.
