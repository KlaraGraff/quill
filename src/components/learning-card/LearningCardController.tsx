import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ReaderInteraction, SerializableRect } from "../reader-interaction";
import { getResponsiveLearningCardWidth } from "./config";
import type {
  CardDesignConfigV1,
  LearningCardActionId,
  LearningCardNote,
  LearningCardResult,
  LearningModuleContent,
} from "./types";
import LearningCardView from "./LearningCardView";

interface LearningCardResponse extends LearningCardResult {
  provenance?: {
    profileId?: string;
    provider?: string;
    model?: string;
  };
}

interface BackendNote {
  id: string;
  content: string;
  updated_at: number;
  scope: "book" | "global";
}

interface LearningCardControllerProps {
  interaction: ReaderInteraction;
  bookId: string;
  bookTitle?: string;
  chapter?: string;
  config: CardDesignConfigV1;
  readerRect?: SerializableRect | DOMRect | null;
  onClose: () => void;
  onAskAi: (quote: string, location?: string) => void;
  onViewAllNotes?: () => void;
}

function moduleText(content: LearningModuleContent | undefined): string {
  if (!content) return "";
  return [
    content.heading,
    content.summary,
    ...(content.details ?? []),
    ...(content.items ?? []).flatMap((item) => [item.title, item.text, ...(item.examples ?? []).flatMap((example) => [example.source, example.target])]),
    content.quote,
  ].filter(Boolean).join("\n");
}

function projection(result: LearningCardResult) {
  const context = moduleText(result.modules.context_meaning);
  const wordInfo = moduleText(result.modules.word_info);
  return {
    definition: wordInfo || context,
    contextExplanation: context || null,
  };
}

function cardPosition(
  interaction: ReaderInteraction,
  readerRect: SerializableRect | DOMRect | null | undefined,
  width: number,
) {
  const reader = readerRect ?? {
    left: 0,
    top: 0,
    right: window.innerWidth,
    bottom: window.innerHeight,
    width: window.innerWidth,
    height: window.innerHeight,
  };
  const margin = 12;
  const maxHeight = Math.max(240, Math.min(window.innerHeight * 0.75, reader.height - margin * 2));
  const preferredRight = interaction.anchorRect.right + 8;
  const preferredLeft = interaction.anchorRect.left - width - 8;
  const left = preferredRight + width <= reader.right - margin
    ? preferredRight
    : preferredLeft >= reader.left + margin
      ? preferredLeft
      : Math.max(reader.left + margin, Math.min(
          interaction.anchorRect.left,
          reader.right - width - margin,
        ));
  const below = reader.bottom - interaction.anchorRect.bottom - margin;
  const above = interaction.anchorRect.top - reader.top - margin;
  const top = below >= Math.min(360, maxHeight) || below >= above
    ? Math.min(interaction.anchorRect.bottom + 8, reader.bottom - maxHeight - margin)
    : Math.max(reader.top + margin, interaction.anchorRect.top - maxHeight - 8);
  return { left, top: Math.max(reader.top + margin, top), maxHeight };
}

export default function LearningCardController({
  interaction,
  bookId,
  bookTitle,
  chapter,
  config,
  readerRect,
  onClose,
  onAskAi,
  onViewAllNotes,
}: LearningCardControllerProps) {
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [retry, setRetry] = useState(0);
  const [result, setResult] = useState<LearningCardResponse>({
    version: 1,
    kind: interaction.kind,
    sourceText: interaction.text,
    modules: {},
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<LearningCardNote[]>([]);
  const [noteEditorOpen, setNoteEditorOpen] = useState(false);
  const [noteDraft, setNoteDraft] = useState("");
  const [noteId, setNoteId] = useState<string | null>(null);
  const [noteSaving, setNoteSaving] = useState(false);
  const [noteScope, setNoteScope] = useState<"book" | "global">("book");
  const [collected, setCollected] = useState(false);
  const [copied, setCopied] = useState(false);

  const refreshNotes = useCallback(async () => {
    const values = await invoke<BackendNote[]>("list_context_notes", {
      bookId,
      word: interaction.kind === "word" ? interaction.text : null,
      location: interaction.kind === "word" ? null : interaction.location,
    });
    setNotes(values.map((note) => ({
      id: note.id,
      content: note.content,
      updatedAt: note.updated_at,
      scope: note.scope,
    })));
  }, [bookId, interaction.kind, interaction.location, interaction.text]);

  useEffect(() => {
    setResult({ version: 1, kind: interaction.kind, sourceText: interaction.text, modules: {} });
    setLoading(true);
    setError(null);
    const requestId = crypto.randomUUID();
    let active = true;
    invoke<LearningCardResponse>("ai_learning_card", {
      text: interaction.text,
      context: interaction.context,
      kind: interaction.kind,
      bookTitle: bookTitle || null,
      chapter: chapter || null,
      cardConfig: JSON.stringify(config),
      requestId,
    }).then((response) => {
      if (!active) return;
      setResult(response);
      setLoading(false);
      const projected = projection(response);
      invoke("save_lookup_record", {
        bookId,
        lookupText: interaction.text,
        contextSentence: interaction.context || null,
        chapter: chapter || null,
        cfi: interaction.location || null,
        definition: projected.definition,
        contextExplanation: projected.contextExplanation,
        resultJson: JSON.stringify(response),
        providerProfileId: response.provenance?.profileId || null,
        model: response.provenance?.model || null,
      }).then(() => {
        window.dispatchEvent(new CustomEvent("lookup-record-changed", { detail: { bookId, cfi: interaction.location } }));
      }).catch(() => {});
    }).catch((reason) => {
      if (!active) return;
      setError(reason instanceof Error ? reason.message : String(reason));
      setLoading(false);
    });
    return () => {
      active = false;
      invoke("ai_cancel", { requestId }).catch(() => {});
    };
  }, [bookId, bookTitle, chapter, config, interaction, retry]);

  useEffect(() => {
    refreshNotes().catch(() => {});
    if (interaction.kind === "word") {
      invoke<string | null>("check_vocab_exists", { bookId, word: interaction.text })
        .then((id) => setCollected(Boolean(id)))
        .catch(() => {});
    }
  }, [bookId, interaction.kind, interaction.text, refreshNotes]);

  useEffect(() => {
    const wrapper = wrapperRef.current;
    if (!wrapper) return;
    const focusable = wrapper.querySelector<HTMLElement>("button,[href],textarea,input,select,[tabindex]:not([tabindex='-1'])");
    focusable?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const items = Array.from(wrapper.querySelectorAll<HTMLElement>(
        "button:not(:disabled),[href],textarea:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex]:not([tabindex='-1'])",
      ));
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    wrapper.addEventListener("keydown", onKeyDown);
    return () => wrapper.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const bounds = readerRect ?? null;
  const availableWidth = bounds?.width ?? window.innerWidth;
  const width = getResponsiveLearningCardWidth(
    interaction.kind,
    config.cards[interaction.kind],
    availableWidth,
  );
  const position = cardPosition(interaction, bounds, width);
  const actionStates = useMemo(() => ({
    collect: { collected, disabled: loading || Boolean(error) },
    ask_ai: { disabled: false },
    note: { disabled: false },
    copy: { copied, disabled: loading || Boolean(error) },
  }), [collected, copied, error, loading]);

  const onAction = useCallback(async (action: LearningCardActionId) => {
    if (action === "ask_ai") {
      onAskAi(interaction.text, interaction.location);
      return;
    }
    if (action === "note") {
      setNoteId(null);
      setNoteDraft("");
      setNoteScope("book");
      setNoteEditorOpen(true);
      return;
    }
    if (action === "copy") {
      const content = [interaction.text, ...Object.values(result.modules).map(moduleText)].filter(Boolean).join("\n\n");
      await navigator.clipboard.writeText(content);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
      return;
    }
    if (action === "collect") {
      const projected = projection(result);
      await invoke("add_vocab_word", {
        bookId,
        word: interaction.text,
        definition: projected.definition,
        contextSentence: interaction.context || null,
        contextExplanation: projected.contextExplanation,
        cfi: interaction.location || null,
      });
      setCollected(true);
      window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId, cfi: interaction.location } }));
    }
  }, [bookId, interaction, onAskAi, result]);

  const saveNote = useCallback(async () => {
    if (!noteDraft.trim()) return;
    setNoteSaving(true);
    try {
      await invoke("save_note", {
        id: noteId,
        bookId,
        anchorKind: interaction.kind === "word" ? "word" : "selection",
        word: interaction.kind === "word" ? interaction.text : null,
        scope: interaction.kind === "word" ? noteScope : "book",
        location: interaction.location || null,
        selectedText: interaction.text,
        content: noteDraft.trim(),
      });
      setNoteEditorOpen(false);
      setNoteDraft("");
      setNoteId(null);
      await refreshNotes();
    } finally {
      setNoteSaving(false);
    }
  }, [bookId, interaction, noteDraft, noteId, noteScope, refreshNotes]);

  return (
    <div
      ref={wrapperRef}
      className="fixed z-[60]"
      style={{ left: position.left, top: position.top }}
    >
      <LearningCardView
        result={result}
        config={config}
        availableWidth={availableWidth}
        maxHeight={position.maxHeight}
        loading={loading}
        error={error}
        notes={notes}
        noteEditorOpen={noteEditorOpen}
        noteDraft={noteDraft}
        noteSaving={noteSaving}
        noteScope={noteScope}
        onNoteScopeChange={setNoteScope}
        actionStates={actionStates}
        onAction={onAction}
        onClose={onClose}
        onRetry={() => setRetry((value) => value + 1)}
        onNoteDraftChange={setNoteDraft}
        onNoteSave={saveNote}
        onNoteCancel={() => { setNoteEditorOpen(false); setNoteDraft(""); setNoteId(null); }}
        onNoteEdit={(note) => { setNoteId(note.id); setNoteDraft(note.content); setNoteScope(note.scope ?? "book"); setNoteEditorOpen(true); }}
        onNoteDelete={(note) => {
          invoke("delete_note", { id: note.id }).then(refreshNotes).catch(() => {});
        }}
        onViewAllNotes={onViewAllNotes}
      />
    </div>
  );
}
