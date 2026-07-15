import { useState, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { BookOpen, Database, Sparkles, Send, Loader2, Plus, ChevronDown, ChevronUp, Trash2, X, Square } from "lucide-react";
import { useAiChat } from "../hooks/useAiChat";
import { timeAgo } from "../utils/timeAgo";
import MessageBubble from "./MessageBubble";
import type { CitedSource } from "../hooks/useAiChat";
import IndexManagerModal from "./IndexManagerModal";

interface AiPanelProps {
  bookId?: string;
  bookTitle?: string;
  bookAuthor?: string;
  currentChapter?: string;
  context?: { text: string; cfi?: string; analysis?: string };
  initialChatId?: string;
  onContextConsumed?: () => void;
  onNavigateToCfi?: (cfi: string) => void;
  onNavigateToSource?: (source: CitedSource) => void;
}

export default function AiPanel({ bookId, bookTitle, bookAuthor, currentChapter, context, initialChatId, onContextConsumed, onNavigateToCfi, onNavigateToSource }: AiPanelProps) {
  const { t } = useTranslation();

  const SUGGESTED_PROMPTS = [
    t("ai.prompt.summarize"),
    t("ai.prompt.themes"),
    t("ai.prompt.characters"),
  ];
  const {
    messages, streaming, send, retryWithWholeBook, cancel, initialize,
    chatId, chats, titling, initializing, groundingStatus, summaryProgress, bookAiState,
    summariesAuto, spoilerGuardEnabled, setSpoilerGuardEnabled, prepareBookOverview, loadChat, deleteChat, renameChat, reset,
  } = useAiChat(bookId, { title: bookTitle, author: bookAuthor, chapter: currentChapter });

  const [input, setInput] = useState("");
  const [pendingQuote, setPendingQuote] = useState<{ text: string; cfi?: string; analysis?: string } | undefined>();
  const [pickerOpen, setPickerOpen] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [newChatFlash, setNewChatFlash] = useState(false);
  const [indexOpen, setIndexOpen] = useState(false);
  const messagesScrollRef = useRef<HTMLDivElement>(null);
  const followMessagesRef = useRef(true);
  const scrollFrameRef = useRef<number | null>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);

  const currentChat = chats.find((c) => c.id === chatId);

  // Initialize on mount / bookId change. Always loads the existing session
  // chat (or empty state when none) — Quote attaches to that chat rather than
  // starting a fresh one.
  useEffect(() => {
    initialize();
  }, [initialize]);

  // Load specific chat when navigating from ChatsPage
  useEffect(() => {
    if (initialChatId && chats.length > 0) {
      loadChat(initialChatId);
    }
  }, [initialChatId, chats.length, loadChat]);

  // A direct, frame-coalesced scroll avoids starting a smooth-scroll animation
  // for every streamed token. Stop following as soon as the reader scrolls up.
  useEffect(() => {
    followMessagesRef.current = true;
  }, [chatId]);

  useEffect(() => {
    if (!followMessagesRef.current || scrollFrameRef.current !== null) return;
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = null;
      const element = messagesScrollRef.current;
      if (element && followMessagesRef.current) {
        element.scrollTop = element.scrollHeight;
      }
    });
  }, [messages, chatId]);

  useEffect(() => () => {
    if (scrollFrameRef.current !== null) cancelAnimationFrame(scrollFrameRef.current);
  }, []);

  // Handle context from the "Quote" context-menu action — pin it as a pending
  // quote chip above the composer. Does NOT reset the chat or auto-send: the
  // quote attaches to the existing session conversation and rides along with
  // the user's next message.
  useEffect(() => {
    if (!context) return;
    setPendingQuote(context);
    onContextConsumed?.();
  }, [context, onContextConsumed]);

  // Focus title input when editing
  useEffect(() => {
    if (editingTitle) {
      titleInputRef.current?.focus();
      titleInputRef.current?.select();
    }
  }, [editingTitle]);

  const handleSend = () => {
    if (!input.trim() || streaming || initializing) return;
    followMessagesRef.current = true;
    send(input.trim(), pendingQuote?.text, pendingQuote?.cfi, pendingQuote?.analysis);
    setPendingQuote(undefined);
    setInput("");
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    } else if (e.key === "Escape" && pendingQuote) {
      e.preventDefault();
      setPendingQuote(undefined);
    }
  };

  const handleTitleSubmit = () => {
    if (titleDraft.trim() && chatId) {
      renameChat(chatId, titleDraft.trim());
    }
    setEditingTitle(false);
  };

  const handleNewChat = () => {
    if (bookId) {
      reset(); // Clears state; DB record created lazily on first send
      setPickerOpen(false);
      setInput("");
      setNewChatFlash(true);
      requestAnimationFrame(() => {
        requestAnimationFrame(() => setNewChatFlash(false));
      });
    }
  };

  const handleSelectChat = (id: string) => {
    loadChat(id);
    setPickerOpen(false);
  };

  return (
    <div className="flex flex-col h-full bg-bg-muted">
      {/* Header */}
      <div className="flex items-center justify-between px-4 h-[63px] border-b border-border shrink-0 relative">
        <div className="flex items-center gap-2.5 min-w-0">
          <Sparkles size={20} className="text-text-muted shrink-0" />
          {editingTitle ? (
            <input
              ref={titleInputRef}
              value={titleDraft}
              onChange={(e) => setTitleDraft(e.target.value)}
              onBlur={handleTitleSubmit}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleTitleSubmit();
                if (e.key === "Escape") setEditingTitle(false);
              }}
              className="text-[15px] font-semibold text-text-primary bg-transparent outline-none border-b border-accent w-full min-w-0"
            />
          ) : (
            <button
              onClick={() => setPickerOpen(!pickerOpen)}
              onDoubleClick={() => {
                if (titling) return;
                setTitleDraft(currentChat?.title || t("ai.newChat"));
                setEditingTitle(true);
              }}
              className="flex items-center gap-1.5 min-w-0 cursor-pointer"
            >
              {titling ? (
                <span className="flex items-center gap-1.5 text-[15px] font-semibold text-text-muted tracking-[-0.23px]">
                  <Loader2 size={14} className="animate-spin" />
                  {t("ai.generatingTitle")}
                </span>
              ) : (
                <span className="text-[15px] font-semibold text-text-primary tracking-[-0.23px] truncate">
                  {currentChat?.title || t("ai.newChat")}
                </span>
              )}
              {pickerOpen ? (
                <ChevronUp size={14} className="text-text-muted shrink-0" />
              ) : (
                <ChevronDown size={14} className="text-text-muted shrink-0" />
              )}
            </button>
          )}
        </div>
        <button
          type="button"
          aria-pressed={spoilerGuardEnabled}
          onClick={() => void setSpoilerGuardEnabled(!spoilerGuardEnabled)}
          disabled={!bookId}
          title={t(spoilerGuardEnabled ? "ai.spoilerGuard.bookOn" : "ai.spoilerGuard.bookOff")}
          aria-label={t(spoilerGuardEnabled ? "ai.spoilerGuard.bookOn" : "ai.spoilerGuard.bookOff")}
          className={`flex size-7 shrink-0 items-center justify-center rounded-lg hover:bg-bg-input disabled:opacity-40 ${spoilerGuardEnabled ? "text-accent-text" : "text-text-muted"}`}
        >
          <BookOpen size={15} />
        </button>
        <button
          type="button"
          onClick={() => setIndexOpen(true)}
          disabled={!bookId}
          title={t("indexManager.title")}
          className="flex size-7 shrink-0 items-center justify-center rounded-lg hover:bg-bg-input disabled:opacity-40"
        >
          <Database size={15} className="text-text-muted" />
        </button>
        <button
          onClick={handleNewChat}
          className="shrink-0 size-7 rounded-lg flex items-center justify-center hover:bg-bg-input cursor-pointer"
        >
          <Plus size={16} className="text-text-muted" />
        </button>

        {/* Chat picker dropdown */}
        {pickerOpen && (
          <div className="absolute top-[62px] left-3 right-3 bg-bg-surface border border-border rounded-[10px] shadow-popover z-50 overflow-hidden">
            <div className="max-h-[300px] overflow-auto pt-1">
              {chats.map((chat) => {
                const isActive = chat.id === chatId;
                return (
                  <div
                    key={chat.id}
                    className={`group flex items-center gap-2 w-full px-3 py-2.5 border-l-2 ${
                      isActive ? "border-accent bg-bg-input" : "border-transparent hover:bg-bg-input"
                    }`}
                  >
                    <button
                      onClick={() => handleSelectChat(chat.id)}
                      className="flex-1 flex flex-col gap-0.5 text-left cursor-pointer min-w-0"
                    >
                      <span className={`text-[13px] tracking-[-0.08px] truncate ${
                        isActive ? "font-semibold text-text-primary" : "font-normal text-text-primary"
                      }`}>
                        {chat.title}
                      </span>
                      <span className="text-[11px] font-medium text-text-muted tracking-[0.06px]">
                        {timeAgo(chat.updated_at)}
                      </span>
                    </button>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteChat(chat.id);
                        if (chats.length <= 1) setPickerOpen(false);
                      }}
                      className="opacity-0 group-hover:opacity-100 shrink-0 p-1 rounded hover:bg-bg-muted cursor-pointer transition-opacity"
                    >
                      <Trash2 size={13} className="text-text-muted" />
                    </button>
                  </div>
                );
              })}
              {chats.length === 0 && (
                <p className="px-3 py-3 text-[13px] text-text-muted">{t("ai.noChats")}</p>
              )}
            </div>
          </div>
        )}
      </div>
      {indexOpen && bookId && <IndexManagerModal bookId={bookId} onClose={() => setIndexOpen(false)} />}

      {/* Messages */}
      <div
        ref={messagesScrollRef}
        className="flex-1 overflow-auto px-4 py-4"
        onScroll={(event) => {
          const element = event.currentTarget;
          followMessagesRef.current = element.scrollHeight - element.scrollTop - element.clientHeight <= 72;
        }}
        onClick={() => pickerOpen && setPickerOpen(false)}
      >
        {messages.length === 0 ? (
          /* Empty state */
          <div className={`flex flex-col items-center justify-center h-full gap-3 transition-opacity duration-300 ${newChatFlash ? "opacity-0" : "opacity-100"}`}>
            <div className="size-14 rounded-full bg-bg-input flex items-center justify-center">
              <Sparkles size={28} className="text-text-muted" />
            </div>
            <h3 className="text-[16px] font-semibold text-text-primary tracking-[-0.31px]">
              {t("ai.startChat")}
            </h3>
            <p className="text-[13px] text-text-muted text-center tracking-[-0.08px] max-w-[215px]">
              {t("ai.startChatSub")}
            </p>
            <div className="flex flex-wrap justify-center gap-2 mt-2">
              {SUGGESTED_PROMPTS.map((prompt) => (
                <button
                  key={prompt}
                  onClick={() => {
                    if (initializing) return;
                    followMessagesRef.current = true;
                    send(prompt, pendingQuote?.text, pendingQuote?.cfi, pendingQuote?.analysis);
                    setPendingQuote(undefined);
                  }}
                  disabled={initializing}
                  className="px-3 py-1.5 rounded-full text-[12px] font-medium text-accent-text bg-accent-bg border border-accent/30 hover:opacity-80 cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-default"
                >
                  {prompt}
                </button>
              ))}
            </div>
            {bookAiState && !bookAiState.hasSummaries && !summariesAuto && (
              <button
                type="button"
                onClick={() => void prepareBookOverview()}
                disabled={bookAiState.indexStatus !== "ready" || summaryProgress?.phase === "sections" || summaryProgress?.phase === "book"}
                className="mt-1 text-[12px] font-medium text-accent-text hover:opacity-75 disabled:cursor-default disabled:opacity-50"
              >
                {t("ai.prepareOverview")}
              </button>
            )}
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {messages.map((msg) => (
              <MessageBubble key={msg.id} msg={msg} messages={messages} streaming={streaming} onNavigateToCfi={onNavigateToCfi} onNavigateToSource={onNavigateToSource} onRetryWithWholeBook={retryWithWholeBook} />
            ))}
            <div />
          </div>
        )}
      </div>

      {/* Input */}
      <div className="border-t border-border px-4 pt-[17px] pb-4 flex flex-col gap-2">
        {groundingStatus === "building" && (
          <p role="status" className="flex items-center gap-1.5 text-[12px] text-text-muted">
            <Loader2 size={12} className="animate-spin" />
            {t("ai.groundingPreparing")}
          </p>
        )}
        {summaryProgress && (summaryProgress.phase === "sections" || summaryProgress.phase === "book") && (
          <p role="status" className="flex items-center gap-1.5 text-[12px] text-text-muted">
            <Loader2 size={12} className="animate-spin" />
            {t("ai.overviewPreparing", { done: summaryProgress.done, total: summaryProgress.total })}
          </p>
        )}
        {/* Pending quote chip — passage to attach to the next message */}
        {pendingQuote && (
          <div className="flex items-start gap-2 px-2.5 py-2 rounded-lg bg-[rgba(192,132,252,0.12)] border-l-2 border-[#c084fc]">
            <p className="flex-1 text-[12px] italic text-text-muted line-clamp-2 tracking-[-0.08px]">
              {pendingQuote.text}
            </p>
            <button
              onClick={() => setPendingQuote(undefined)}
              title={t("aiPanel.quoteChip.dismiss")}
              aria-label={t("aiPanel.quoteChip.dismiss")}
              className="shrink-0 size-[18px] flex items-center justify-center rounded hover:bg-bg-input cursor-pointer"
            >
              <X size={13} className="text-text-muted" />
            </button>
          </div>
        )}
        <div className="flex gap-2 items-start">
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={t("ai.placeholder")}
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            rows={2}
            className="flex-1 h-[60px] bg-bg-input rounded-lg px-3 py-2 text-[14px] text-text-primary placeholder:text-text-placeholder tracking-[-0.15px] leading-5 outline-none border border-transparent focus:border-accent resize-none"
          />
          <button
            onClick={streaming ? cancel : handleSend}
            title={streaming ? t("ai.stop") : t("ai.send")}
            aria-label={streaming ? t("ai.stop") : t("ai.send")}
            disabled={!streaming && (!input.trim() || initializing)}
            className={`size-[60px] shrink-0 rounded-lg flex items-center justify-center cursor-pointer bg-accent text-white ${
              !streaming && (!input.trim() || initializing) ? "opacity-50" : ""
            }`}
          >
            {streaming ? (
              <Square size={14} fill="currentColor" />
            ) : (
              <Send size={16} />
            )}
          </button>
        </div>
        <p className="text-[12px] text-text-muted">
          {t("ai.sendHint")}
        </p>
      </div>
    </div>
  );
}
