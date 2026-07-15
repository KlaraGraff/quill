import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
import { LearningCardStreamParser } from "./streaming";

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
  bookAuthor?: string;
  chapter?: string;
  config: CardDesignConfigV1;
  readerRect?: SerializableRect | DOMRect | null;
  onClose: () => void;
  onAskAi: (quote: string, location?: string, analysis?: string) => void;
  onViewAllNotes?: () => void;
  onLookupSuccess?: (interaction: ReaderInteraction) => void;
}

interface LearningCardStreamChunk {
  delta: string;
  reasoning_delta?: string;
  done: boolean;
  error?: string;
}

interface CardPoint {
  left: number;
  top: number;
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
  const availableHeight = Math.max(0, reader.height - margin * 2);
  const maxHeight = Math.min(window.innerHeight * 0.75, availableHeight);
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

function clampCardPoint(
  point: CardPoint,
  readerRect: SerializableRect | DOMRect | null | undefined,
  cardWidth: number,
  cardHeight: number,
): CardPoint {
  const reader = readerRect ?? {
    left: 0,
    top: 0,
    right: window.innerWidth,
    bottom: window.innerHeight,
  };
  const margin = 12;
  const minLeft = reader.left + margin;
  const minTop = reader.top + margin;
  const maxLeft = Math.max(minLeft, reader.right - cardWidth - margin);
  const maxTop = Math.max(minTop, reader.bottom - cardHeight - margin);
  return {
    left: Math.min(maxLeft, Math.max(minLeft, point.left)),
    top: Math.min(maxTop, Math.max(minTop, point.top)),
  };
}

export default function LearningCardController({
  interaction,
  bookId,
  bookTitle,
  bookAuthor,
  chapter,
  config,
  readerRect,
  onClose,
  onAskAi,
  onViewAllNotes,
  onLookupSuccess,
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
    const allowedModuleIds = new Set(
      config.cards[interaction.kind].modules
        .filter((module) => module.enabled)
        .map((module) => module.id),
    );
    const parser = new LearningCardStreamParser(allowedModuleIds);
    let active = true;
    let unlisten: UnlistenFn | undefined;

    const run = async () => {
      try {
        unlisten = await listen<LearningCardStreamChunk>(
          `ai-learning-card-chunk-${requestId}`,
          (event) => {
            if (!active || event.payload.done || !event.payload.delta) return;
            const streamedModules = parser.push(event.payload.delta);
            if (Object.keys(streamedModules).length === 0) return;
            setResult((current) => ({
              ...current,
              modules: { ...current.modules, ...streamedModules },
            }));
          },
        );
        if (!active) {
          unlisten();
          unlisten = undefined;
          return;
        }

        const response = await invoke<LearningCardResponse>("ai_learning_card", {
          text: interaction.text,
          context: interaction.context,
          kind: interaction.kind,
          bookTitle: bookTitle || null,
          bookAuthor: bookAuthor || null,
          chapter: chapter || null,
          cardConfig: JSON.stringify(config),
          requestId,
        });
        if (!active) return;
        setResult(response);
        setLoading(false);
        if (interaction.kind === "word") onLookupSuccess?.(interaction);
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
      } catch (reason) {
        if (!active) return;
        setError(reason instanceof Error ? reason.message : String(reason));
        setLoading(false);
      } finally {
        unlisten?.();
        unlisten = undefined;
      }
    };

    run();
    return () => {
      active = false;
      unlisten?.();
      unlisten = undefined;
      invoke("ai_cancel", { requestId }).catch(() => {});
    };
  }, [bookAuthor, bookId, bookTitle, chapter, config, interaction, onLookupSuccess, retry]);

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
  const initialPosition = cardPosition(interaction, bounds, width);
  const [position, setPosition] = useState<CardPoint>(() => ({
    left: initialPosition.left,
    top: initialPosition.top,
  }));
  const positionRef = useRef(position);
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    origin: CardPoint;
  } | null>(null);

  const updatePosition = useCallback((next: CardPoint) => {
    const cardRect = wrapperRef.current?.getBoundingClientRect();
    const clamped = clampCardPoint(
      next,
      bounds,
      cardRect?.width ?? width,
      cardRect?.height ?? initialPosition.maxHeight,
    );
    positionRef.current = clamped;
    setPosition((current) => (
      current.left === clamped.left && current.top === clamped.top ? current : clamped
    ));
  }, [bounds, initialPosition.maxHeight, width]);

  useEffect(() => {
    const wrapper = wrapperRef.current;
    if (!wrapper) return;
    const clampCurrent = () => updatePosition(positionRef.current);
    const observer = new ResizeObserver(clampCurrent);
    observer.observe(wrapper);
    window.addEventListener("resize", clampCurrent);
    clampCurrent();
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", clampCurrent);
    };
  }, [updatePosition]);

  const onDragPointerDown = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      origin: positionRef.current,
    };
  }, []);

  const onDragPointerMove = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    event.preventDefault();
    updatePosition({
      left: drag.origin.left + event.clientX - drag.startX,
      top: drag.origin.top + event.clientY - drag.startY,
    });
  }, [updatePosition]);

  const onDragPointerEnd = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }, []);

  const actionStates = useMemo(() => ({
    collect: { collected, disabled: loading || Boolean(error) },
    ask_ai: { disabled: loading || Boolean(error) },
    note: { disabled: false },
    copy: { copied, disabled: loading || Boolean(error) },
  }), [collected, copied, error, loading]);

  const onAction = useCallback(async (action: LearningCardActionId) => {
    if (action === "ask_ai") {
      onAskAi(interaction.text, interaction.location, JSON.stringify(result));
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
        maxHeight={initialPosition.maxHeight}
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
        onDragPointerDown={onDragPointerDown}
        onDragPointerMove={onDragPointerMove}
        onDragPointerEnd={onDragPointerEnd}
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
