import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  X,
  Loader2,
  Languages,
  Check,
  Copy,
  ChevronDown,
  ChevronUp,
  Settings,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { aiErrorMessageKey, getAiErrorCode, isAiSettingsError, type AiErrorCode } from "../utils/aiError";

interface TranslationPopoverProps {
  x: number;
  y: number;
  text: string;
  context?: string;
  bookId: string;
  bookTitle?: string;
  bookAuthor?: string;
  chapter?: string;
  cfi?: string;
  onClose: () => void;
}

interface AiStreamChunk {
  delta: string;
  done: boolean;
  error?: string;
}

const LANG_NAMES: Record<string, string> = {
  en: "English",
  zh: "Chinese",
  ja: "Japanese",
  ko: "Korean",
  es: "Spanish",
  fr: "French",
  de: "German",
  pt: "Portuguese",
  ru: "Russian",
  ar: "Arabic",
  it: "Italian",
};

function useStreamingTranslation(
  text: string,
  context: string | undefined,
  bookId: string,
  bookTitle: string | undefined,
  bookAuthor: string | undefined,
  chapter: string | undefined,
  cfi: string | undefined
) {
  const contentRef = useRef("");
  const [content, setContent] = useState("");
  const [streaming, setStreaming] = useState(true);
  const [aiError, setAiError] = useState<AiErrorCode | null>(null);
  const [languageNotConfigured, setLanguageNotConfigured] = useState(false);
  const [targetLang, setTargetLang] = useState("");
  const [streamError, setStreamError] = useState(false);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const requestIdRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    contentRef.current = "";
    setContent("");
    setStreaming(true);
    setAiError(null);
    setLanguageNotConfigured(false);
    setTargetLang("");
    setStreamError(false);

    // Fetch target language for display
    invoke<Record<string, string>>("get_all_settings").then((s) => {
      if (cancelled) return;
      const lang = s.translation_language || s.language || "en";
      setTargetLang(lang);
    }).catch(() => {});

    const run = async () => {
      const requestId = crypto.randomUUID();
      requestIdRef.current = requestId;

      unlistenRef.current = await listen<AiStreamChunk>(
        `ai-translate-chunk-${requestId}`,
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
        await invoke("ai_translate_passage", {
          text,
          context: context || null,
          bookId,
          bookTitle: bookTitle || null,
          bookAuthor: bookAuthor || null,
          chapter: chapter || null,
          targetLanguage: null,
          requestId,
        });
      } catch (err) {
        if (!cancelled) {
          const msg = String(err);
          const errorCode = getAiErrorCode(msg);
          if (isAiSettingsError(errorCode)) {
            setAiError(errorCode);
          } else if (msg.includes("TRANSLATION_LANGUAGE_NOT_CONFIGURED")) {
            setLanguageNotConfigured(true);
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
  }, [text, context, bookAuthor, bookId, bookTitle, cfi, chapter]);

  return { content, contentRef, streaming, aiError, languageNotConfigured, targetLang, streamError };
}

export default function TranslationPopover({
  x,
  y,
  text,
  context,
  bookId,
  bookTitle,
  bookAuthor,
  chapter,
  cfi,
  onClose,
}: TranslationPopoverProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  const { content, contentRef, streaming, aiError, languageNotConfigured, targetLang, streamError } =
    useStreamingTranslation(text, context, bookId, bookTitle, bookAuthor, chapter, cfi);

  const allDone = !streaming;
  const hasContent = !!content;
  const hasConfigurationError = aiError !== null || languageNotConfigured;

  // Position clamping
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

  const handleCopy = () => {
    navigator.clipboard.writeText(contentRef.current);
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

  // Dismiss on click outside
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

  const langName = LANG_NAMES[targetLang] || targetLang;

  return (
    <>
    <div className="fixed inset-0 z-40" onClick={onClose} />
    <div
      ref={popoverRef}
      className="fixed z-50 w-[520px] bg-bg-surface border border-border/80 rounded-xl shadow-context"
      style={{ left: pos.left, top: pos.top }}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-4 pt-3 pb-2.5 bg-accent-bg rounded-t-xl border-b border-border/40">
        <div className="flex items-center gap-2">
          <Languages size={16} className="text-accent-text" />
          <span className="text-[14px] font-medium text-accent-text tracking-[-0.15px]">
            {t("translation.title")}
          </span>
          {langName && (
            <span className="text-[13px] text-accent-text/60">{langName}</span>
          )}
        </div>
        <button
          onClick={onClose}
          className="size-6 flex items-center justify-center rounded hover:bg-bg-surface/60 cursor-pointer"
        >
          <X size={14} className="text-text-muted" />
        </button>
      </div>

      {/* Content */}
      <div className="px-4 pb-2 max-h-[420px] overflow-auto">
        {/* Original text — collapsible, collapsed by default */}
        <div className="relative pt-3 pb-2">
          <button
            onClick={() => setExpanded(!expanded)}
            className="absolute top-3 right-0 size-6 flex items-center justify-center rounded hover:bg-bg-muted cursor-pointer"
          >
            {expanded ? (
              <ChevronUp size={14} className="text-text-muted" />
            ) : (
              <ChevronDown size={14} className="text-text-muted" />
            )}
          </button>
          {expanded ? (
            <div className="pr-7 max-h-[120px] overflow-auto">
              <p className="text-[13px] text-text-muted italic leading-[1.55]">
                {text}
              </p>
            </div>
          ) : (
            <p className="text-[13px] text-text-muted italic leading-[1.55] line-clamp-2 pr-7">
              {text}
            </p>
          )}
        </div>

        <div className="h-px bg-border/60 mb-3" />

        {/* Not configured state */}
        {hasConfigurationError ? (
          <div className="flex flex-col items-center gap-2 py-4 text-center">
            <p className="text-[13px] text-text-muted">
              {languageNotConfigured
                ? t("translation.languageNotConfigured")
                : aiError ? t(aiErrorMessageKey(aiError)) : null}
            </p>
            <button
              onClick={async () => {
                onClose();
                await invoke("open_settings_on_main", { section: languageNotConfigured ? "tools" : "ai" });
                const main = await WebviewWindow.getByLabel("main");
                await main?.setFocus();
              }}
              className="flex items-center gap-1.5 text-[13px] font-medium text-accent-text hover:opacity-70 cursor-pointer"
            >
              <Settings size={14} />
              {languageNotConfigured ? t("translation.openSettings") : t("ai.openSettings")}
            </button>
          </div>
        ) : null}

        {/* Translation body */}
        {!hasConfigurationError && !streamError &&
          (streaming && !content ? (
            <div className="flex items-center gap-1.5 py-1">
              <Loader2 size={14} className="animate-spin text-text-muted" />
              <span className="text-[13px] text-text-muted">
                {t("translation.translating")}
              </span>
            </div>
          ) : (
            <p className="text-[13px] text-text-primary leading-[1.55]">
              {content}
              {streaming && (
                <Loader2
                  size={12}
                  className="inline-block ml-0.5 animate-spin text-text-muted"
                />
              )}
            </p>
          ))}
        {!hasConfigurationError && streamError && (
          <p className="py-3 text-[13px] text-text-muted">{t("ai.requestFailed")}</p>
        )}
      </div>

      {/* Footer */}
      {allDone && hasContent && !hasConfigurationError && !streamError && (
        <div className="flex items-center justify-end px-4 py-2.5 border-t border-border/40">
          <button
            onClick={handleCopy}
            className="flex items-center gap-1.5 text-[13px] font-medium cursor-pointer text-text-muted hover:opacity-70"
          >
            {copied ? <Check size={14} /> : <Copy size={14} />}
            {copied ? t("translation.copied") : t("translation.copy")}
          </button>
        </div>
      )}
    </div>
    </>
  );
}
