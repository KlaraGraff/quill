# 215 — Split Ask AI into Explain (inline) and Quote (side panel)

GitHub issue: https://github.com/yicheng47/quill/issues/215

## Motivation

The reader's selection context menu has a single **Ask AI Assistant** entry that opens the side AI panel with the selection attached as context. This conflates two very different user intents:

1. **Quick comprehension** — "what does this sentence mean here?" — should be inline, one-shot, no follow-up.
2. **Conversational exploration** — "let me ask the AI about this passage" — should be in the persistent side panel with the passage anchored as a citation.

The Lookup popover already covers #1 for *words*, but there is no sentence/passage equivalent. Users today either suffer the heavier side-panel flow for a trivial question or skip the AI entirely.

## Scope

Replace **Ask AI Assistant** in the reader selection context menu with two distinct entries:

### Explain
- Inline popover near the selection (modeled after `LookupPopover`).
- Streams an AI-generated explanation of the selected sentence/passage *in its context* (chapter, surrounding text, book title).
- One-shot. No follow-up chat. Closes on click-outside / Escape, same as Lookup.
- Available for any selection length. For short selections (≤5 words), **Look Up** still appears alongside **Explain**.

### Quote
- Opens the AI side panel (`AiPanel`) with the selected passage pinned as a **quote chip** above the composer.
- The quote chip is visible while the user types and is sent as a citation along with the message.
- After send, the quote becomes attached to that user turn (rendered as a block-quote in the message bubble) and the chip clears for the next message.
- **Reuse existing chat for the current reading session.** If a chat already exists for this book/session (whether the panel is currently open or closed), the quote chip attaches to that conversation — never start a new chat. The action becomes "open the existing chat + attach a quote chip" rather than "new chat seeded with the passage". Only when no chat exists yet for the session does Quote create one.

## Implementation Phases

### Phase 1 — Context menu split
- `src/components/ReaderContextMenu.tsx`: replace `onAskAI` prop with `onExplain` and `onQuote`. Icons: keep `Bot`/`Sparkles` for Explain (distinct from Look Up's `Sparkles`), use lucide `Quote` for Quote.
- `src/i18n/en.json` + `zh.json`: rename `contextMenu.askAI` → `contextMenu.explain`, add `contextMenu.quote`.
- `src/pages/Reader.tsx`: wire the two new callbacks (the existing `onAskAI` site at line ~1392 splits into two handlers).

### Phase 2 — Explain popover
- New component `src/components/ExplainPopover.tsx` based on `LookupPopover`'s layout (header, streaming body, footer actions: copy, save-as-note maybe).
- New backend command in `src-tauri/src/commands/ai.rs` (or extend the existing lookup streaming) that takes `{ text, sentence_context, book_title, chapter }` and streams an explanation.
- Settings: respect existing AI provider + `settings.lookup.nativeLanguage` for explanation language.

### Phase 3 — Quote chip in AI panel
- `src/components/AiPanel.tsx`: add a "pending quote" state above the composer — a dismissable chip showing a truncated preview of the passage.
- Update `useAiChat` hook (`src/hooks/`) to accept an optional `quote` payload on send; serialize it into the outgoing user message (rendered as a block-quote in the message bubble) and into the prompt context sent to the model.
- The existing `aiContext` flow (text + cfi) feeds the quote chip rather than being injected silently into a system message.
- **Session chat reuse:** Quote must attach to the existing session chat if one exists. Audit `useAiChat` / `AiPanel` state — if the per-book chat is keyed by `bookId` (or the equivalent session identifier), Quote should rehydrate that chat before attaching the chip, never reset history. If no session-keyed chat exists yet, fall back to creating one. The chat lifecycle is tied to the reading session, not to the panel's open/closed state.

### Phase 4 — Polish
- Keyboard: `Esc` closes the Explain popover and (separately) clears the pending quote chip when the composer has focus.
- Hover/active states match existing popover and chip patterns elsewhere in the app.
- i18n: all new strings (chip label, dismiss tooltip, popover title, empty state) in `en.json` and `zh.json`.

## Verification

- [ ] Selecting a sentence and clicking **Explain** shows an inline popover with a streaming explanation; closing it does *not* open the AI panel.
- [ ] Selecting a passage and clicking **Quote** opens the AI side panel with the passage shown as a chip above the composer.
- [ ] Sending a message with a pending quote renders the quote as a block-quote in the user turn, and the model's response references it.
- [ ] Quote chip is dismissable before sending.
- [ ] Pending quote does not reset an in-flight conversation.
- [ ] **If a chat already exists for the current reading session** (panel open *or* closed), Quote attaches the chip to that chat — no new chat is created and prior turns remain visible.
- [ ] If no chat exists yet for the session, Quote creates one and attaches the chip.
- [ ] Short selections (≤5 words) still show **Look Up** *and* **Explain** in the context menu.
- [ ] i18n: all new strings work in both English and Chinese.
- [ ] The old **Ask AI Assistant** entry no longer exists in the context menu.

## Open Questions

- Should Explain results be savable as notes (like Lookup → Save to Dict)? Probably yes — surfaces in the Notes panel.
- Should there be a setting to default the Quote action to "send immediately" (like Translate) for users who want a one-tap summary? Likely no in v1.
