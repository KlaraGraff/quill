# Security Notes

## Local Credentials

Quill Personal stores API keys and OAuth tokens in the local-only `secrets.db`. Routine saves, model discovery, connection tests, reading lookups, and AI chats read that database directly and do not access the operating-system credential store. Credential values are filtered from settings APIs, never returned to the webview, never written to logs, and never included in library sync, snapshots, or the MCP surface.

The database uses SQLite `secure_delete=ON` with `journal_mode=DELETE`, avoiding a long-lived WAL history of replaced credentials. On Unix platforms, `secrets.db` and its transient SQLite journal are restricted to the current user (`0600`). `secure_delete` applies to SQLite-managed pages; it cannot erase Time Machine copies, filesystem snapshots, storage-device history, or data already copied by another process. This is intentionally local plaintext storage: another process already running as the same operating-system user may be able to read it. The tradeoff avoids repeated Keychain authorization prompts, similar to common local developer credential files. Disk encryption and a protected user account remain important.

### Upgrading From The v1.4 Vault

Versions using the v1.4 encrypted vault may leave rows in `encrypted_secrets`, protected by the historical `com.ryoyamada.quill` Keychain master key. Quill does not request that key during startup or ordinary AI use. AI settings instead show a metadata-only migration notice. Only after the user clicks **Import locally** and accepts the explanatory dialog does Quill read old Keychain items. Every readable value is written to local storage in one transaction and only its successfully recovered encrypted row is removed. A newer local value always wins over an older encrypted copy. If one row is corrupt or the v1.4 master key is missing, that row remains untouched while independent older per-item credentials can still be recovered; the command reports a partial migration and keeps the reminder visible. Canceling or denying a system prompt stops the migration before committing the current batch.

The historical master-key item is not deleted automatically because deletion can trigger another operating-system prompt. Once all encrypted rows have migrated, Quill no longer reads that item. Credentials saved in still older per-item Keychain namespaces are offered through the same explicit migration action.

Credential sync is not implemented. In particular, credentials are not placed in the iCloud event log or snapshot. Encrypted credential sync requires a stable signed application identity and a formally provisioned iCloud Keychain access group; the current ad-hoc distribution cannot safely provide that identity.

## Content Security Policy

The application deliberately keeps `style-src 'unsafe-inline'` in its Tauri CSP. The React reader and vendored Foliate engine apply reader-theme and pagination styles at runtime, including style attributes and injected style blocks. Script execution remains restricted to `script-src 'self'`; no raw AI or book HTML is executed. Removing inline styles requires a Foliate-compatible nonce or stylesheet-API migration and must be verified against EPUB, PDF, and text reader flows before changing the policy.
