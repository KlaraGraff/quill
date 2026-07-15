# Quill Personal

Quill Personal is a macOS reading app for learning English through original books. It combines a warm, long-form reading surface with contextual AI lookup, a persistent reading conversation, vocabulary study, and in-text learning markers.

This is an independently maintained personal edition based on the open-source [Quill](https://github.com/yicheng47/quill) project. It is not an official release of the original project.

## What It Supports

- Import support for EPUB, PDF, TXT, Markdown, HTML, MOBI/AZW/AZW3, FB2/FBZ, and CBZ.
- Contextual word lookup, passage explanation, translation, and an expandable AI conversation panel per book.
- Saved vocabulary, lookup history, learning states, and optional markers in reflowable EPUB text.
- Multiple API keys per provider profile. Before output begins, unavailable keys are tried in configured priority order.
- OpenAI-compatible APIs, Anthropic, Ollama, and optional OpenAI OAuth.
- Local-first library data. API keys and OAuth tokens remain in a local-only credential database, are never returned to the webview, and do not participate in sync.
- Optional multi-device sync through a user-selected folder in iCloud Drive.

### Format Capabilities

| Source format | Import behavior | Reading controls | Selection and manual highlights | Automatic vocabulary markers |
| --- | --- | --- | --- | --- |
| EPUB | Reads natively | Font, spacing, margins, scroll/paginated flow | Supported | Supported |
| TXT, Markdown, HTML | Original source is retained and converted to a stable internal EPUB | Same as EPUB | Supported | Supported |
| PDF | Reads natively | Theme, zoom, single/two-page layout, scroll/paginated flow | Supported when the PDF has a usable text layer | Not included in the first release |
| MOBI, AZW, AZW3, FB2, FBZ | Reads through Foliate's native parser | Reflow controls when the renderer supports them | Not currently exposed | Not supported |
| CBZ | Reads natively | Theme only | Not supported | Not supported |

Format support describes the current local import and reader integration. It does not imply DRM support or perfect rendering for every publisher-specific variant.

## Sync

Quill Personal does not use the original Quill iCloud container. To sync, choose a folder inside your own iCloud Drive from Settings, then select that same folder on each Mac. The app stores its event log, books, and covers inside that folder.

This edition currently targets the macOS desktop app. It does not claim compatibility with the original Quill iOS app or its private iCloud data.

## Download

Current builds and release notes are published at [KlaraGraff/quill Releases](https://github.com/KlaraGraff/quill/releases). macOS builds currently use a valid ad-hoc signature, so Gatekeeper will still require a first-run confirmation. The signing and notarization roadmap is documented in [macOS distribution](docs/guide/macos-distribution.md). Automatic updates are disabled until this fork has its own signed release channel.

## Development

Requirements: Node.js 22, npm, Rust, and the Tauri prerequisites for the target platform. Clone with the reader engine submodule:

```bash
git clone --recurse-submodules https://github.com/KlaraGraff/quill.git
cd quill
npm ci
npm run tauri dev
```

For an existing checkout, initialize it once with `git submodule update --init --recursive`.

Useful static checks:

```bash
npm exec tsc --noEmit
npm run lint
cd src-tauri && cargo check
```

Repository conventions are in [AGENTS.md](AGENTS.md).

## Attribution And License

Quill Personal is based on Quill by yicheng47. Original Quill copyright remains with its authors. This repository retains the original [MIT License](LICENSE), including its copyright notice.
