import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, Loader2, Settings } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import Markdown, { defaultUrlTransform } from "react-markdown";
import type { ChatMessage, CitedSource } from "../hooks/useAiChat";
import { aiErrorMessageKey, isAiErrorCode, isAiSettingsError } from "../utils/aiError";
import {
  citedSourcesInContent,
  citationMarkerFromHref,
  markdownWithCitationLinks,
} from "./citation-markers";

interface MessageBubbleProps {
  msg: ChatMessage;
  messages: ChatMessage[];
  streaming: boolean;
  onNavigateToCfi?: (cfi: string) => void;
  onNavigateToSource?: (source: CitedSource) => void;
  onRetryWithWholeBook?: (assistantId: string) => void;
}

function CitationChip({ source, onClick }: { source: CitedSource; onClick?: () => void }) {
  const number = source.marker.replace(/^S/, "");
  const tooltip = [source.sectionTitle, source.snippet].filter(Boolean).join("\n");
  return (
    <button
      type="button"
      title={tooltip}
      aria-label={`Source ${number}`}
      onClick={onClick}
      className="mx-0.5 inline-flex h-5 min-w-5 translate-y-[-1px] items-center justify-center rounded border border-accent/35 bg-accent-bg px-1 text-[10px] font-semibold leading-none text-accent-text align-super hover:opacity-75"
    >
      {number}
    </button>
  );
}

export default function MessageBubble({ msg, messages, streaming, onNavigateToCfi, onNavigateToSource, onRetryWithWholeBook }: MessageBubbleProps) {
  const { t } = useTranslation();
  const isLast = msg === messages[messages.length - 1];
  const [reasoningExpanded, setReasoningExpanded] = useState<boolean | null>(null);

  if (msg.role === "assistant") {
    const errorCode = isAiErrorCode(msg.content) ? msg.content : null;
    if (errorCode) {
      const needsSettings = isAiSettingsError(errorCode);
      return (
        <div className="bg-bg-surface border border-border rounded-lg px-[13px] py-[13px] max-w-[85%]">
          <p className={`text-[14px] text-text-muted ${needsSettings ? "mb-2" : ""}`}>
            {t(aiErrorMessageKey(errorCode))}
          </p>
          {needsSettings && (
            <button
              onClick={async () => {
                await invoke("open_settings_on_main", { section: "ai" });
                const main = await WebviewWindow.getByLabel("main");
                await main?.setFocus();
              }}
              className="flex items-center gap-1.5 text-[13px] font-medium text-accent-text hover:opacity-70 cursor-pointer"
            >
              <Settings size={14} />
              {t("ai.openSettings")}
            </button>
          )}
        </div>
      );
    }
    const hasReasoning = Boolean(msg.reasoning?.trim());
    const reasoningInProgress = streaming && isLast && !msg.content;
    const reasoningOpen = reasoningExpanded ?? reasoningInProgress;
    const sources = msg.sources ?? [];
    const citedSources = citedSourcesInContent(msg.content, sources);
    return (
      <div className="bg-bg-surface border border-border rounded-lg px-[13px] py-[13px] max-w-[85%]">
        {hasReasoning && (
          <div className={msg.content ? "mb-2 border-b border-border pb-2" : ""}>
            <button
              type="button"
              aria-expanded={reasoningOpen}
              onClick={() => setReasoningExpanded(!reasoningOpen)}
              className="flex w-full items-center gap-1.5 text-left text-[12px] font-medium text-text-muted hover:text-text-primary cursor-pointer"
            >
              {reasoningOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
              {reasoningInProgress && <Loader2 size={12} className="animate-spin" />}
              <span>{t(reasoningInProgress ? "ai.reasoningStreaming" : "ai.reasoning")}</span>
            </button>
            {reasoningOpen && (
              <div className="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap text-[12px] leading-[18px] text-text-muted">
                {msg.reasoning}
              </div>
            )}
          </div>
        )}
        {streaming && !msg.content && isLast && !hasReasoning ? (
          <span className="flex items-center gap-1.5 text-[14px] text-text-muted">
            <Loader2 size={14} className="animate-spin" />
            {t("ai.thinking")}
          </span>
        ) : msg.content ? (
          <div className="prose prose-sm max-w-none text-[14px] text-text-primary leading-5 tracking-[-0.15px] [&_h1]:text-[16px] [&_h2]:text-[15px] [&_h3]:text-[14px] [&_h1]:font-semibold [&_h2]:font-semibold [&_h3]:font-semibold [&_h1]:mt-3 [&_h1]:mb-1 [&_h2]:mt-3 [&_h2]:mb-1 [&_h3]:mt-2 [&_h3]:mb-1 [&_p]:my-1.5 [&_ul]:my-1 [&_ol]:my-1 [&_li]:my-0.5 [&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:italic [&_blockquote]:text-text-muted [&_code]:bg-bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:rounded [&_code]:text-[13px] [&_pre]:bg-bg-muted [&_pre]:p-3 [&_pre]:rounded-lg [&_pre]:overflow-x-auto [&_strong]:font-semibold [&_em]:italic [&_hr]:border-border [&_a]:text-accent [&_a]:underline">
            <Markdown
              urlTransform={(url) => (
                url.startsWith("quill-citation:") ? url : defaultUrlTransform(url)
              )}
              components={{
                a: ({ href, children }) => {
                  const marker = citationMarkerFromHref(href);
                  const source = marker ? sources.find((candidate) => candidate.marker === marker) : undefined;
                  return source
                    ? <CitationChip source={source} onClick={() => onNavigateToSource?.(source)} />
                    : <a href={href}>{children}</a>;
                },
              }}
            >
              {markdownWithCitationLinks(msg.content, sources)}
            </Markdown>
            {streaming && msg.content && isLast && (
              <Loader2 size={14} className="inline-block ml-1 animate-spin text-text-muted" />
            )}
          </div>
        ) : null}
        {citedSources.length > 0 && (
          <div className="mt-2 flex items-center gap-1 border-t border-border pt-2">
            <span className="mr-1 text-[11px] text-text-muted">{t("ai.sources")}</span>
            {citedSources.map((source) => (
              <CitationChip key={source.marker} source={source} onClick={() => onNavigateToSource?.(source)} />
            ))}
          </div>
        )}
        {msg.spoilerGuard?.active && !(streaming && isLast) && (
          msg.spoilerGuard.wholeBookIntent ? (
            <div className="mt-2 flex flex-wrap items-center justify-between gap-2 border-t border-border pt-2 text-[11px] text-text-muted">
              <span>{t("ai.spoilerGuard.notice", { progress: msg.spoilerGuard.progress })}</span>
              {onRetryWithWholeBook && msg.dbId && isLast && (
                <button
                  type="button"
                  onClick={() => onRetryWithWholeBook(msg.id)}
                  className="font-medium text-accent-text hover:opacity-75"
                >
                  {t("ai.spoilerGuard.retryWholeBook")}
                </button>
              )}
            </div>
          ) : (
            <span
              title={t("ai.spoilerGuard.badgeHint")}
              className="mt-2 inline-flex rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted"
            >
              {t("ai.spoilerGuard.badge")}
            </span>
          )
        )}
      </div>
    );
  }

  return (
    <div className="flex justify-end">
      <div className="max-w-[85%] flex flex-col gap-1.5">
        {msg.context && (
          <button
            onClick={() => msg.contextCfi && onNavigateToCfi?.(msg.contextCfi)}
            className={`border-l-2 border-[#c084fc] pl-3 pt-0.5 text-left ${
              msg.contextCfi && onNavigateToCfi ? "cursor-pointer hover:opacity-70" : "cursor-default"
            }`}
          >
            <p className="text-[12px] italic text-text-muted line-clamp-2">
              {msg.context}
            </p>
          </button>
        )}
        <div className="bg-[rgba(192,132,252,0.15)] rounded-lg px-[13px] py-[13px]">
          <p className="text-[14px] text-text-primary leading-5 tracking-[-0.15px]">
            {msg.content}
          </p>
        </div>
      </div>
    </div>
  );
}
