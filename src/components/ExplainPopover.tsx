import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { X, Loader2, WandSparkles, Check, Copy, Settings } from "lucide-react";
import { useTranslation } from "react-i18next";
import Markdown from "react-markdown";
import { LOOKUP_PROSE } from "./lookup-prose";
import { aiErrorMessageKey, getAiErrorCode, isAiSettingsError, type AiErrorCode } from "../utils/aiError";

interface ExplainPopoverProps {
  x: number;
  y: number;
  text: string;
  sentence: string;
  bookTitle?: string;
  bookAuthor?: string;
  chapter?: string;
  bookId: string;
  cfi?: string;
  onClose: () => void;
}

interface AiStreamChunk {
  delta: string;
  done: boolean;
  error?: string;
}

function useExplainStream(
  passage: string,
  surrounding: string | undefined,
  bookTitle: string | undefined,
  bookAuthor: string | undefined,
  chapter: string | undefined
) {
  const contentRef = useRef("");
  const [content, setContent] = useState("");
  const [streaming, setStreaming] = useState(true);
  const [aiError, setAiError] = useState<AiErrorCode | null>(null);
  const [streamError, setStreamError] = useState(false);
  const unlistenRef = useRef<UnlistenFn | null>(null);
  const requestIdRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    contentRef.current = "";
    setContent("");
    setStreaming(true);
    setAiError(null);
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
        await invoke("ai_explain", {
          passage,
          surrounding: surrounding || null,
          bookTitle: bookTitle || null,
          bookAuthor: bookAuthor || null,
          chapter: chapter || null,
          requestId,
        });
      } catch (err) {
        if (!cancelled) {
          const msg = String(err);
          const errorCode = getAiErrorCode(msg);
          if (isAiSettingsError(errorCode)) {
            setAiError(errorCode);
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
  }, [passage, surrounding, bookAuthor, bookTitle, chapter]);

  return { content, contentRef, streaming, aiError, streamError };
}

export default function ExplainPopover({
  x,
  y,
  text,
  sentence,
  bookTitle,
  bookAuthor,
  chapter,
  onClose,
}: ExplainPopoverProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  const { content, contentRef, streaming, aiError, streamError } = useExplainStream(
    text,
    sentence,
    bookTitle,
    bookAuthor,
    chapter
  );

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
          <WandSparkles size={16} className="text-accent-text" />
          <span className="text-[14px] font-medium text-accent-text tracking-[-0.15px]">
            {t("explain.title")}
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
        {/* Selected passage */}
        <div className="border-l-2 border-[#c084fc] pl-3 pt-3 pb-1">
          <p className="text-[12px] italic text-text-muted line-clamp-3">{text}</p>
        </div>

        {aiError ? (
          <div className="flex flex-col items-center gap-2 py-4 text-center">
            <p className="text-[13px] text-text-muted">{t(aiErrorMessageKey(aiError))}</p>
            <button
              onClick={async () => {
                onClose();
                await invoke("open_settings_on_main", { section: "ai" });
                const main = await WebviewWindow.getByLabel("main");
                await main?.setFocus();
              }}
              className="flex items-center gap-1.5 text-[13px] font-medium text-accent-text hover:opacity-70 cursor-pointer"
            >
              <Settings size={14} />
              {t("ai.openSettings")}
            </button>
          </div>
        ) : streaming && !content ? (
          <div className="flex items-center gap-1.5 py-3">
            <Loader2 size={14} className="animate-spin text-text-muted" />
            <span className="text-[13px] text-text-muted">{t("explain.thinking")}</span>
          </div>
        ) : streamError ? (
          <p className="py-3 text-[13px] text-text-muted">{t("ai.requestFailed")}</p>
        ) : (
          <div className={`${LOOKUP_PROSE} text-[13px] text-text-primary pt-2.5`}>
            <Markdown>{content}</Markdown>
            {streaming && (
              <Loader2 size={12} className="inline-block ml-0.5 animate-spin text-text-muted" />
            )}
          </div>
        )}
      </div>

      {/* Footer — Copy */}
      {!streaming && content && !aiError && !streamError && (
        <div className="flex items-center justify-end px-4 py-2.5 border-t border-border/40">
          <button
            onClick={handleCopy}
            className="flex items-center gap-1.5 text-[13px] font-medium cursor-pointer text-text-muted hover:opacity-70"
          >
            {copied ? <Check size={14} /> : <Copy size={14} />}
            {copied ? t("explain.copied") : t("explain.copy")}
          </button>
        </div>
      )}
    </div>
    </>
  );
}
