# Lantern

[简体中文](README.md) · [English](README.en.md)

> You should not have to adapt to a fixed AI reading workflow.
>
> Lantern lets AI explain books in language you can understand, then lets you reshape the reading tools around your own goals.

Lantern is a desktop app for learning English through long-form reading, with macOS as its primary platform. It does more than add an “Ask AI” button to a reader: you decide how AI explains, how deeply it goes, what a learning card shows, and what happens when you select text.

From lightweight double-click lookups to AI workflows for complex sentences, literary analysis, exam preparation, or professional reading, Lantern can gradually become a reader that fits you.

This is an independently maintained personal edition based on the open-source [Quill](https://github.com/yicheng47/quill) project. It is not an official release of the original project.

## English explanations should match your level

Many reading tools offer an “English explanation” option but answer with near-native vocabulary and complex sentences. One unfamiliar word becomes a new paragraph that is even harder to understand.

Lantern uses your English proficiency as part of every explanation, rather than treating it as a display preference. Set your current CEFR level (A1–C2) in your learner profile, or record results for IELTS, TOEFL, TOEIC, Cambridge English, DET, CET-4, or CET-6 to estimate a learning level.

For each word lookup, phrase explanation, and passage interpretation, AI considers your level alongside the explanation language, translation target, and content density:

- **A1–A2:** Accurate Chinese meaning establishes the foundation, with concise English that fits your current level—rather than using harder English to explain simple English.
- **B1:** English explanations take priority, with essential Chinese support for abstract or easily misunderstood points.
- **B2–C2:** More natural and in-depth English explanations, with target-language translation available when wanted.
- Choosing “English-first” does not mean accepting explanations written for native speakers. AI deliberately controls vocabulary and sentence complexity so the explanation itself remains comprehensible input.

Start by understanding the text, then gradually move toward understanding English through English—without being interrupted by another explanation you cannot read.

## Built-in modules are a starting point—build your own when they are not enough

Lantern includes learning modules for contextual meaning, word information, common meanings, collocations, grammar, references, tone, writing patterns, memory aids, and more. Configure their visibility, order, default expansion state, and content density separately for words, phrases, and sentences/passages.

But presets should not limit your learning method.

When the included modules do not meet your needs, create your own AI modules:

- Name each module and write a prompt that is entirely yours.
- Generate content from the selected text, its context, and book information.
- Add it to word, phrase, or passage cards, where it can be shown, hidden, reordered, and expanded alongside built-in modules.
- Test and optimize prompts until the result fits your way of learning.
- Create custom selection actions, then bind them to a shortcut or double-click action for one-step access to your method.

For example, you can build modules for:

- complex-sentence breakdowns for IELTS or other exam preparation;
- rhetoric, narrative point of view, and tone in literary works;
- terminology, prerequisite knowledge, and real-world use in professional books;
- reusable expressions and rewriting suggestions for writing practice;
- minimal lookups that preserve immersion by showing only essential information.

## Decide what AI tells you—and when it tells you

- Design different learning cards for words, phrases, sentences, and passages.
- Show, hide, and reorder built-in or custom modules freely.
- Choose compact, standard, or detailed content density, and set example and key-term counts.
- Configure lookup language, explanation language, translation target, and whether to show a short gloss.
- Customize selection-menu actions and their order: look up, explain, translate, save, mark, copy, ask AI, or a custom action.
- Single-click to open the menu, double-click for a quick lookup, and assign shortcuts to selection actions.
- Preview cards instantly in Settings; when needed, call the active AI service to view a live result.

## Choose your own AI services

- Supports OpenAI-compatible APIs, Anthropic, Ollama, and optional OpenAI OAuth.
- Add multiple AI services, each with its own name, base URL, model, and priority.
- Use a custom compatible API, such as a self-hosted gateway or another OpenAI-compatible provider.
- Save multiple API keys for each service; before output begins, Lantern tries usable keys and services in priority order.
- Test connections and discover available models.
- API keys and OAuth tokens stay in a local-only credential database on the current device. They are never returned to the webview or included in sync.

## From reading to a learning loop

- Understand the current text with AI word lookup, phrase explanation, passage interpretation, and translation.
- Continue a conversation within the same book by carrying a word, source sentence, explanation, and context into the AI panel.
- Save vocabulary, lookup history, learning states, and notes, then return to the source location.
- Manage words as new, learning, or mastered, and review words when they are due.
- Export and import CSV or JSON vocabulary backups.
- Mark words automatically after lookup, either at the current occurrence or throughout the book.
- Customize the color, opacity, highlight, underline, bold treatment, and font of manual and automatic markers.

## Customize the reading experience too

- Import custom fonts and adjust typeface, spacing, margins, and reading layout.
- Choose a theme or set a custom page background and tint.
- Switch between scrolling and paginated reading; PDFs support zoom, single/two-page layouts, and scrolling/paginated modes.
- Use bookmarks, highlights, notes, the table of contents, and reading progress.
- Add lightweight learning marks without changing the source book.

## System requirements

| Platform | Supported range |
| --- | --- |
| macOS | **macOS 11 Big Sur or later**, on **Apple Silicon (M-series)** Macs only |
| Windows | **Windows 11 x64** installer available |
| Intel Mac | No Intel macOS installer is currently provided |
| Linux | No release build is currently provided |

macOS is the primary platform. Multi-device sync through a user-selected iCloud Drive folder is available only on macOS. The Windows build supports local reading and AI features but does not include this iCloud sync capability.

## Format capabilities

| Source format | Import behavior | Reading controls | Selection and manual highlights | Automatic vocabulary markers |
| --- | --- | --- | --- | --- |
| EPUB | Reads natively | Font, spacing, margins, scroll/paginated flow | Supported | Supported |
| TXT, Markdown, HTML | Original source is retained and converted to a stable internal EPUB | Same as EPUB | Supported | Supported |
| PDF | Reads natively | Theme, zoom, single/two-page layout, scroll/paginated flow | Supported when the PDF has a usable text layer | Not included in the first release |
| MOBI, AZW, AZW3, FB2, FBZ | Reads through Foliate's native parser | Reflow controls when the renderer supports them | Not currently exposed | Not supported |
| CBZ | Reads natively | Theme only | Not supported | Not supported |

Format support describes the current local import and reader integration. It does not imply DRM support or perfect rendering for every publisher-specific variant.

## Local-first data and sync

Library data stays local first. To sync across devices, choose a folder in your own iCloud Drive from Settings, then select the same folder on every Mac. The app stores its event log, books, and covers there.

This edition does not use the original Quill iCloud container. It does not claim compatibility with the original Quill iOS app or its private iCloud data.

## Download

Current builds and release notes are published at [KlaraGraff/lantern Releases](https://github.com/KlaraGraff/lantern/releases). macOS builds currently use a valid ad-hoc signature, so Gatekeeper will still require a first-run confirmation. The signing and notarization roadmap is documented in [macOS distribution](docs/guide/macos-distribution.md). Automatic updates are disabled until this fork has its own signed release channel.

## Development

Requirements: Node.js 22, npm, Rust, and the Tauri prerequisites for the target platform. The reader engine source is committed with the repository:

```bash
git clone https://github.com/KlaraGraff/lantern.git
cd lantern
npm ci
npm run tauri dev
```

Useful static checks:

```bash
npm exec tsc --noEmit
npm run lint
cd src-tauri && cargo check
```

Repository conventions are in [AGENTS.md](AGENTS.md).

## Attribution and license

Lantern is based on Quill by yicheng47. Original Quill copyright remains with its authors. This repository retains the original [MIT License](LICENSE), including its copyright notice.
