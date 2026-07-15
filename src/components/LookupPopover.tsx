import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { X, Loader2, Sparkles, BookmarkPlus, Check, Copy, Settings, MessageSquareMore } from "lucide-react";
import { useTranslation } from "react-i18next";
import Markdown from "react-markdown";
import { LOOKUP_PROSE } from "./lookup-prose";
import { aiErrorMessageKey, getAiErrorCode, isAiSettingsError, type AiErrorCode } from "../utils/aiError";

const TRANSLATION_MARKER = "[[QUILL_TRANSLATION]]";

async function notifyReaderWindows(event: "lookup-record-changed" | "vocab-changed", detail: { bookId: string; cfi?: string }) {
  const windows = await WebviewWindow.getAll();
  await Promise.all(
    windows
      .filter((window) => window.label === `reader-${detail.bookId}`)
      .map((window) => emitTo(window.label, event, detail)),
  );
}

interface AiStreamChunk {
  delta: string;
  done: boolean;
  error?: string;
}

interface LookupPopoverProps {
  x: number;
  y: number;
  word: string;
  sentence: string;
  bookTitle?: string;
  bookAuthor?: string;
  chapter?: string;
  bookId: string;
  cfi?: string;
  onClose: () => void;
  onAskFollowUp?: (quote: string, cfi?: string) => void;
}

function useStreamingLookup(
  word: string,
  sentence: string,
  bookTitle: string | undefined,
  bookAuthor: string | undefined,
  chapter: string | undefined,
  kind: "definition" | "context"
) {
  const contentRef = useRef("");
  const [content, setContent] = useState("");
  const [streaming, setStreaming] = useState(true);
  const [aiError, setAiError] = useState<AiErrorCode | null>(null);
  const [translationLanguageNotConfigured, setTranslationLanguageNotConfigured] = useState(false);
  const [streamError, setStreamError] = useState(false);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const requestIdRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    contentRef.current = "";
    setContent("");
    setStreaming(true);
    setAiError(null);
    setTranslationLanguageNotConfigured(false);
    setStreamError(false);

    const run = async () => {
      const requestId = crypto.randomUUID();
      requestIdRef.current = requestId;

      unlistenRef.current = await listen<AiStreamChunk>(
        `ai-lookup-chunk-${requestId}`,
        (event) => {
          if (cancelled) return;
          if (event.payload.done) {
            if (event.payload.error) {
              const errorCode = getAiErrorCode(event.payload.error);
              if (isAiSettingsError(errorCode)) setAiError(errorCode);
              else setStreamError(true);
            }
            setStreaming(false);
            unlistenRef.current?.();
            unlistenRef.current = null;
            requestIdRef.current = null;
            return;
          }
          contentRef.current += event.payload.delta;
          setContent(contentRef.current);
        }
      );

      try {
        await invoke("ai_lookup", {
          word,
          sentence,
          bookTitle: bookTitle || null,
          bookAuthor: bookAuthor || null,
          chapter: chapter || null,
          requestId,
          kind,
        });
      } catch (err) {
        if (!cancelled) {
          const msg = String(err);
          const errorCode = getAiErrorCode(msg);
          if (isAiSettingsError(errorCode)) {
            setAiError(errorCode);
          } else if (msg.includes("LOOKUP_TRANSLATION_LANGUAGE_NOT_CONFIGURED")) {
            setTranslationLanguageNotConfigured(true);
          } else {
            setContent(`Error: ${msg}`);
          }
          setStreaming(false);
        }
        if (requestIdRef.current === requestId) {
          requestIdRef.current = null;
          unlistenRef.current?.();
          unlistenRef.current = null;
        }
      }
    };

    run();

    return () => {
      cancelled = true;
      if (requestIdRef.current) invoke("ai_cancel", { requestId: requestIdRef.current }).catch(() => {});
      requestIdRef.current = null;
      unlistenRef.current?.();
      unlistenRef.current = null;
    };
  }, [word, sentence, bookAuthor, bookTitle, chapter, kind]);

  return { content, contentRef, streaming, aiError, translationLanguageNotConfigured, streamError };
}

function splitDefinitionContent(content: string, streaming: boolean): {
  translationLine: string | null;
  definitionText: string;
} {
  if (content.startsWith(TRANSLATION_MARKER)) {
    const newline = content.indexOf("\n");
    if (newline === -1) {
      return {
        translationLine: null,
        definitionText: streaming ? "" : content.slice(TRANSLATION_MARKER.length).trimStart(),
      };
    }
    const translationLine = content.slice(TRANSLATION_MARKER.length, newline).trim();
    return {
      translationLine: translationLine || null,
      definitionText: content.slice(newline + 1).trimStart(),
    };
  }

  if (TRANSLATION_MARKER.startsWith(content)) {
    return { translationLine: null, definitionText: "" };
  }

  return { translationLine: null, definitionText: content };
}

function displayedDefinitionContent(content: string): string {
  const split = splitDefinitionContent(content, false);
  return [split.translationLine, split.definitionText].filter(Boolean).join("\n");
}

export default function LookupPopover({
  x,
  y,
  word,
  sentence,
  bookTitle,
  bookAuthor,
  chapter,
  bookId,
  cfi,
  onClose,
  onAskFollowUp,
}: LookupPopoverProps) {
  const { t } = useTranslation();
  const [saved, setSaved] = useState(false);
  const [copied, setCopied] = useState(false);
  const historySavedRef = useRef(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  // Two concurrent AI streams
  const definition = useStreamingLookup(word, sentence, bookTitle, bookAuthor, chapter, "definition");
  const context = useStreamingLookup(word, sentence, bookTitle, bookAuthor, chapter, "context");

  // Split the backend-marked translation from the definition stream.
  const { translationLine, definitionText } = splitDefinitionContent(definition.content, definition.streaming);

  const aiError = definition.aiError || context.aiError;
  const translationLanguageNotConfigured =
    definition.translationLanguageNotConfigured || context.translationLanguageNotConfigured;
  const streamError = definition.streamError || context.streamError;
  const hasConfigurationError = aiError !== null || translationLanguageNotConfigured;
  const allDone = !definition.streaming && !context.streaming;
  const hasContent = definition.content || context.content;

  useEffect(() => {
    if (!allDone || !hasContent || hasConfigurationError || streamError || historySavedRef.current) return;
    historySavedRef.current = true;
    invoke("save_lookup_record", {
      bookId,
      lookupText: word,
      contextSentence: sentence || null,
      chapter: chapter || null,
      cfi: cfi || null,
      definition: displayedDefinitionContent(definition.contentRef.current),
      contextExplanation: context.contentRef.current || null,
    }).then(() => {
      window.dispatchEvent(new CustomEvent("lookup-record-changed", { detail: { bookId, cfi } }));
      notifyReaderWindows("lookup-record-changed", { bookId, cfi: cfi || undefined }).catch(() => {});
    }).catch((err) => console.error("Failed to save lookup history:", err));
  }, [allDone, bookId, cfi, chapter, context.contentRef, definition.contentRef, hasConfigurationError, hasContent, sentence, streamError, word]);

  // Position clamping — re-run whenever the popover resizes (e.g. as content streams in)
  const [pos, setPos] = useState({ left: x, top: y });

  useEffect(() => {
    const el = popoverRef.current;
    if (!el) return;
    const clamp = () => {
      const rect = el.getBoundingClientRect();
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      let left = x;
      let top = y;
      if (left + rect.width > vw - 16) left = vw - rect.width - 16;
      if (left < 16) left = 16;
      if (top + rect.height > vh - 16) top = y - rect.height - 8;
      if (top < 16) top = 16;
      setPos({ left, top });
    };
    const observer = new ResizeObserver(clamp);
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Check if word is already saved
  useEffect(() => {
    invoke<string | null>("check_vocab_exists", { bookId, word }).then((id) => {
      if (id) setSaved(true);
    }).catch(() => {});
  }, [bookId, word]);

  const handleSave = async () => {
    try {
      await invoke("add_vocab_word", {
        bookId,
        word,
        definition: displayedDefinitionContent(definition.contentRef.current ?? ""),
        contextSentence: sentence || null,
        contextExplanation: context.contentRef.current || null,
        cfi: cfi || null,
      });
      setSaved(true);
      window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId, cfi } }));
      notifyReaderWindows("vocab-changed", { bookId, cfi: cfi || undefined }).catch(() => {});
    } catch (err) {
      console.error("Failed to save vocab word:", err);
    }
  };

  const handleCopy = () => {
    const fullText = [
      displayedDefinitionContent(definition.contentRef.current),
      context.contentRef.current,
    ]
      .filter(Boolean)
      .join("\n\n");
    navigator.clipboard.writeText(fullText);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  // Dismiss on Escape
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  // Dismiss on click outside — delay registration to avoid catching the
  // context-menu click that opened us
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const id = requestAnimationFrame(() => {
      document.addEventListener("mousedown", handler);
    });
    return () => {
      cancelAnimationFrame(id);
      document.removeEventListener("mousedown", handler);
    };
  }, [onClose]);

  return (
    <>
    <div className="fixed inset-0 z-40" onClick={onClose} />
    <div
      ref={popoverRef}
      className="fixed z-50 w-[440px] bg-bg-surface border border-border/80 rounded-xl shadow-context"
      style={{ left: pos.left, top: pos.top }}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-4 pt-3 pb-2.5 bg-accent-bg rounded-t-xl border-b border-border/40">
        <div className="flex items-center gap-2">
          <Sparkles size={16} className="text-accent-text" />
          <span className="text-[14px] font-medium text-accent-text tracking-[-0.15px]">
            {t("lookup.title")}
          </span>
        </div>
        <button
          onClick={onClose}
          className="size-6 flex items-center justify-center rounded hover:bg-bg-surface/60 cursor-pointer"
        >
          <X size={14} className="text-text-muted" />
        </button>
      </div>

      {/* Content */}
      <div className="px-4 pb-2 max-h-[360px] overflow-auto">
        {/* Word heading */}
        <div className="flex items-center gap-2 pt-3 pb-2">
          <h3 className="text-[20px] font-bold text-text-primary leading-6">{word}</h3>
        </div>

        {hasConfigurationError ? (
          <div className="flex flex-col items-center gap-2 py-4 text-center">
            <p className="text-[13px] text-text-muted">
              {translationLanguageNotConfigured
                ? t("lookup.translationLanguageNotConfigured")
                : aiError ? t(aiErrorMessageKey(aiError)) : null}
            </p>
            <button
              onClick={async () => {
                onClose();
                await invoke("open_settings_on_main", { section: translationLanguageNotConfigured ? "tools" : "ai" });
                const main = await WebviewWindow.getByLabel("main");
                await main?.setFocus();
              }}
              className="flex items-center gap-1.5 text-[13px] font-medium text-accent-text hover:opacity-70 cursor-pointer"
            >
              <Settings size={14} />
              {translationLanguageNotConfigured ? t("lookup.openSettings") : t("ai.openSettings")}
            </button>
          </div>
        ) : null}

        {!hasConfigurationError && streamError ? (
          <p className="py-3 text-[13px] text-text-muted">{t("ai.requestFailed")}</p>
        ) : null}

        {/* Definition section */}
        {!hasConfigurationError && !streamError && (definition.streaming && !definition.content ? (
          <div className="flex items-center gap-1.5 py-1">
            <Loader2 size={14} className="animate-spin text-text-muted" />
            <span className="text-[13px] text-text-muted">{t("lookup.lookingUp")}</span>
          </div>
        ) : (
          <>
            {translationLine && (
              <p className="text-[13px] text-accent-text mb-1.5">{translationLine}</p>
            )}
            <div className={`${LOOKUP_PROSE} text-[13px] text-text-primary`}>
              <Markdown>{definitionText}</Markdown>
              {definition.streaming && (
                <Loader2 size={12} className="inline-block ml-0.5 animate-spin text-text-muted" />
              )}
            </div>
          </>
        ))}

        {/* In this context — card */}
        {!hasConfigurationError && !streamError && (context.content || context.streaming) && (
          <div className="mt-3 mb-1 p-3 rounded-lg bg-bg-muted border border-border/50">
            <span className="block text-[12px] font-medium text-text-muted mb-1">
              {t("lookup.inContext")}
            </span>
            {context.streaming && !context.content ? (
              <div className="flex items-center gap-1.5 py-0.5">
                <Loader2 size={12} className="animate-spin text-text-muted" />
                <span className="text-[12px] text-text-muted">{t("lookup.analyzing")}</span>
              </div>
            ) : (
              <div className={`${LOOKUP_PROSE} text-[13px] text-text-secondary`}>
                <Markdown>{context.content}</Markdown>
                {context.streaming && (
                  <Loader2 size={12} className="inline-block ml-0.5 animate-spin text-text-muted" />
                )}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Footer — Save & Copy */}
      {allDone && hasContent && !hasConfigurationError && !streamError && (
        <div className="flex items-center justify-between px-4 py-2.5 border-t border-border/40">
          <div className="flex items-center gap-3">
            <button
              onClick={handleSave}
              disabled={saved}
              className="flex items-center gap-1.5 text-[13px] font-medium cursor-pointer text-accent-text hover:opacity-70 disabled:opacity-50 disabled:cursor-default"
            >
              {saved ? <Check size={14} /> : <BookmarkPlus size={14} />}
              {saved ? t("lookup.saved") : t("lookup.saveToDict")}
            </button>
            {onAskFollowUp && (
              <button
                onClick={() => {
                  const quote = [
                    `Word: ${word}`,
                    `Sentence: ${sentence}`,
                    `Definition: ${displayedDefinitionContent(definition.contentRef.current)}`,
                    context.contentRef.current ? `In context: ${context.contentRef.current}` : "",
                  ].filter(Boolean).join("\n\n");
                  onAskFollowUp(quote, cfi);
                  onClose();
                }}
                className="flex items-center gap-1.5 text-[13px] font-medium cursor-pointer text-text-secondary hover:text-accent-text"
              >
                <MessageSquareMore size={14} />
                {t("lookup.askFollowUp")}
              </button>
            )}
          </div>
          <button
            onClick={handleCopy}
            className="flex items-center gap-1.5 text-[13px] font-medium cursor-pointer text-text-muted hover:opacity-70"
          >
            {copied ? <Check size={14} /> : <Copy size={14} />}
            {copied ? t("lookup.copied") : t("lookup.copy")}
          </button>
        </div>
      )}
    </div>
    </>
  );
}
