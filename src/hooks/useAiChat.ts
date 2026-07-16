import { useState, useCallback, useRef, useEffect, useLayoutEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getAiErrorCode } from "../utils/aiError";
import { useSettings } from "./useSettings";

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  context?: string;
  contextCfi?: string;
  contextAnalysis?: string;
  reasoning?: string;
  sources?: CitedSource[];
  spoilerGuard?: SpoilerGuardMetadata;
  dbId?: string;
}

export interface SpoilerGuardMetadata {
  active: boolean;
  wholeBookIntent: boolean;
  progress: number;
}

interface AiChatResult {
  sources: CitedSource[];
  spoilerGuard: SpoilerGuardMetadata;
}

export interface CitedSource {
  marker: string;
  chunkId: string;
  sectionIndex: number;
  sectionHref?: string;
  sectionTitle?: string;
  snippet: string;
  charStart?: number;
  charEnd?: number;
}

interface ChatRecord {
  id: string;
  book_id: string;
  title: string;
  model: string | null;
  pinned: boolean;
  metadata: string | null;
  created_at: number;
  updated_at: number;
}

interface ChatMsgRecord {
  id: string;
  chat_id: string;
  role: string;
  content: string;
  context: string | null;
  metadata: string | null;
  created_at: number;
  updated_at: number;
}

interface AiStreamChunk {
  delta: string;
  reasoning_delta?: string;
  done: boolean;
  error?: string;
}

interface GroundingStatusEvent {
  status: "building" | "unavailable";
}

interface SummaryProgressEvent {
  done: number;
  total: number;
  phase: "sections" | "book" | "done" | "error";
}

interface BookAiState {
  indexStatus: "ready" | "building" | "failed" | "unsupported" | "missing";
  hasSummaries: boolean;
  summariesStale: boolean;
}

interface ChatMessageMetadata {
  cfi?: string;
  analysis?: string;
  reasoning?: string;
  sources?: CitedSource[];
  spoilerGuard?: SpoilerGuardMetadata;
}

function parseSpoilerGuard(value: unknown): SpoilerGuardMetadata | undefined {
  if (!value || typeof value !== "object") return undefined;
  const guard = value as Record<string, unknown>;
  if (typeof guard.active !== "boolean" || typeof guard.wholeBookIntent !== "boolean") {
    return undefined;
  }
  return {
    active: guard.active,
    wholeBookIntent: guard.wholeBookIntent,
    progress: typeof guard.progress === "number" ? guard.progress : 0,
  };
}

function parseAiChatResult(value: unknown): AiChatResult {
  // Compatibility with v1.5 development builds that returned sources directly.
  if (Array.isArray(value)) {
    return {
      sources: parseCitedSources(value) ?? [],
      spoilerGuard: { active: false, wholeBookIntent: false, progress: 0 },
    };
  }
  if (!value || typeof value !== "object") {
    return {
      sources: [],
      spoilerGuard: { active: false, wholeBookIntent: false, progress: 0 },
    };
  }
  const result = value as Record<string, unknown>;
  return {
    sources: parseCitedSources(result.sources) ?? [],
    spoilerGuard: parseSpoilerGuard(result.spoilerGuard)
      ?? { active: false, wholeBookIntent: false, progress: 0 },
  };
}

function parseCitedSources(value: unknown): CitedSource[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const sources = value.filter((item): item is CitedSource => {
    if (!item || typeof item !== "object") return false;
    const source = item as Record<string, unknown>;
    return typeof source.marker === "string"
      && typeof source.chunkId === "string"
      && typeof source.sectionIndex === "number"
      && typeof source.snippet === "string";
  });
  return sources.length > 0 ? sources : undefined;
}

function parseMessageMetadata(metadata: string | null): ChatMessageMetadata {
  if (!metadata) return {};
  try {
    const parsed: unknown = JSON.parse(metadata);
    if (!parsed || typeof parsed !== "object") return {};
    const value = parsed as Record<string, unknown>;
    return {
      cfi: typeof value.cfi === "string" ? value.cfi : undefined,
      analysis: typeof value.analysis === "string" ? value.analysis : undefined,
      reasoning: typeof value.reasoning === "string" ? value.reasoning : undefined,
      sources: parseCitedSources(value.sources),
      spoilerGuard: parseSpoilerGuard(value.spoilerGuard),
    };
  } catch {
    return {};
  }
}

function serializeMessageMetadata(metadata: ChatMessageMetadata): string | null {
  const compact: ChatMessageMetadata = {};
  if (metadata.cfi) compact.cfi = metadata.cfi;
  if (metadata.analysis) compact.analysis = metadata.analysis;
  if (metadata.reasoning) compact.reasoning = metadata.reasoning;
  if (metadata.sources?.length) compact.sources = metadata.sources;
  if (metadata.spoilerGuard) compact.spoilerGuard = metadata.spoilerGuard;
  return Object.keys(compact).length > 0 ? JSON.stringify(compact) : null;
}

function messageContentForApi(message: ChatMessage): string {
  const context: string[] = [];
  if (message.context) {
    context.push(`[Selected passage]\n${message.context}\n[/Selected passage]`);
  }
  if (message.contextAnalysis) {
    context.push(
      `[Existing learning-card analysis]\n${message.contextAnalysis}\n[/Existing learning-card analysis]`,
    );
  }
  return context.length > 0
    ? `${context.join("\n\n")}\n\n${message.content}`
    : message.content;
}

/** Derive a short title from the user's first message (truncated at word boundary). */
/** Fallback: truncate user message into a short title. */
function deriveTitle(userMsg: string): string {
  let title = userMsg
    .replace(/^Explain this passage:\s*"?/i, "")
    .replace(/^Explain\s*/i, "")
    .replace(/"$/, "")
    .trim();
  if (title.length > 40) {
    const cut = title.lastIndexOf(" ", 40);
    title = title.substring(0, cut > 10 ? cut : 40) + "...";
  }
  return title;
}

/** Generate a chat title using a dedicated AI call with per-request event channel.
 *  Titles from the user's first message alone so it can run concurrently with
 *  the response stream instead of waiting for it to finish. */
async function generateAiTitle(
  userMsg: string,
): Promise<string | null> {
  const requestId = `title-${crypto.randomUUID()}`;
  const eventName = `ai-title-chunk-${requestId}`;
  let title = "";
  let finished = false;
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  let unlisten: UnlistenFn | undefined;
  try {
    let resolveResult: (value: string | null) => void = () => {};
    const resultPromise = new Promise<string | null>((resolve) => { resolveResult = resolve; });
    const finish = (value: string | null) => {
      if (finished) return;
      finished = true;
      resolveResult(value);
    };
    unlisten = await listen<AiStreamChunk>(eventName, (event) => {
      if (!event.payload.done) {
        title += event.payload.delta;
        return;
      }
      if (event.payload.error) return finish(null);
      title = title.replace(/^["']|["']$/g, "").replace(/[.!]$/, "").trim();
      if (title.length > 50) title = title.substring(0, 50).trim() + "...";
      finish(title || null);
    });
    timeoutId = setTimeout(() => finish(null), 15000);
    invoke("ai_generate_title", {
      userMessage: userMsg,
      assistantMessage: "",
      requestId,
    }).catch(() => finish(null));
    const result = await resultPromise;
    return result;
  } finally {
    if (timeoutId) clearTimeout(timeoutId);
    unlisten?.();
    if (!title || !finished) invoke("ai_cancel", { requestId }).catch(() => {});
  }
}

let msgIdCounter = 0;
function nextMsgId() {
  return `local-${Date.now()}-${++msgIdCounter}`;
}

interface BookContext {
  title?: string;
  author?: string;
  chapter?: string;
}

export function useAiChat(bookId?: string, bookContext?: BookContext) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [titling, setTitling] = useState(false);
  // True from mount until the first initialize() resolves. Consumers gate
  // sending on this so a message can't be sent (and lazily create a *new*
  // chat) before the existing session chat has loaded.
  const [initializing, setInitializing] = useState(true);
  const [chatId, setChatId] = useState<string | null>(null);
  const [chats, setChats] = useState<ChatRecord[]>([]);
  const [groundingStatus, setGroundingStatus] = useState<GroundingStatusEvent["status"] | null>(null);
  const [summaryProgress, setSummaryProgress] = useState<SummaryProgressEvent | null>(null);
  const [bookAiState, setBookAiState] = useState<BookAiState | null>(null);
  const { settings, save: saveSetting } = useSettings();
  const spoilerSettingKey = bookId ? `book_spoiler_guard_${bookId}` : null;
  const spoilerGuardEnabled = spoilerSettingKey
    ? settings[spoilerSettingKey] === "on"
      || (settings[spoilerSettingKey] !== "off" && settings.ai_spoiler_guard !== "false")
    : settings.ai_spoiler_guard !== "false";

  const unlistenRef = useRef<UnlistenFn | null>(null);
  const groundingUnlistenRef = useRef<UnlistenFn | null>(null);
  const summaryRequestIdRef = useRef<string | null>(null);
  const initializedBookRef = useRef<string | null>(null);
  const messagesRef = useRef<ChatMessage[]>([]);
  const chatIdRef = useRef<string | null>(null);
  const streamingRef = useRef(false);
  const activeRequestIdRef = useRef<string | null>(null);
  const activeAssistantIdRef = useRef<string | null>(null);
  const activeReplacementRef = useRef<ChatMessage | null>(null);
  const initializingRef = useRef(true);
  const streamGenerationRef = useRef(0);
  const streamFrameCleanupRef = useRef<(() => void) | null>(null);
  const initializationGenerationRef = useRef(0);
  const titleGenerationRef = useRef(0);
  const bookIdRef = useRef(bookId);
  const mountedRef = useRef(false);

  useLayoutEffect(() => {
    bookIdRef.current = bookId;
  }, [bookId]);

  const stopActiveStream = useCallback((cancelBackend = true) => {
    const requestId = activeRequestIdRef.current;
    streamGenerationRef.current += 1;
    streamFrameCleanupRef.current?.();
    streamFrameCleanupRef.current = null;
    unlistenRef.current?.();
    unlistenRef.current = null;
    groundingUnlistenRef.current?.();
    groundingUnlistenRef.current = null;
    activeRequestIdRef.current = null;
    activeAssistantIdRef.current = null;
    streamingRef.current = false;
    if (mountedRef.current) {
      setStreaming(false);
      setGroundingStatus(null);
    }
    if (cancelBackend && requestId) {
      invoke("ai_cancel", { requestId }).catch(() => {});
    }
  }, []);

  // Keep the ref in lockstep with the state so send() (which reads refs to
  // avoid stale closures) can refuse while initialization is in flight.
  const setInitializingSynced = (v: boolean) => {
    initializingRef.current = v;
    if (mountedRef.current) setInitializing(v);
  };

  // Cleanup stream listener on unmount
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      stopActiveStream();
    };
  }, [stopActiveStream]);

  // Reset initialization when bookId changes
  useEffect(() => {
    initializationGenerationRef.current += 1;
    titleGenerationRef.current += 1;
    if (bookId && initializedBookRef.current && initializedBookRef.current !== bookId) {
      stopActiveStream();
      initializedBookRef.current = null;
      setTitling(false);
    }
  }, [bookId, stopActiveStream]);

  const refreshBookAiState = useCallback(async () => {
    if (!bookId) {
      setBookAiState(null);
      return null;
    }
    try {
      const next = await invoke<BookAiState>("get_book_ai_state", { bookId });
      if (bookIdRef.current === bookId) setBookAiState(next);
      return next;
    } catch {
      return null;
    }
  }, [bookId]);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void refreshBookAiState();
    if (!bookId) return () => { disposed = true; };
    listen<SummaryProgressEvent>(`ai-summary-progress-${bookId}`, (event) => {
      if (disposed) return;
      setSummaryProgress(event.payload);
      if (event.payload.phase === "done" || event.payload.phase === "error") {
        summaryRequestIdRef.current = null;
        void refreshBookAiState();
      }
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [bookId, refreshBookAiState]);

  const prepareBookOverview = useCallback(async () => {
    if (!bookId || summaryRequestIdRef.current) return;
    const state = await refreshBookAiState();
    if (!state || state.indexStatus !== "ready" || (state.hasSummaries && !state.summariesStale)) return;
    const requestId = `summary-${crypto.randomUUID()}`;
    summaryRequestIdRef.current = requestId;
    setSummaryProgress({ done: 0, total: 0, phase: "sections" });
    try {
      await invoke("ai_prepare_book", { bookId, requestId });
    } catch {
      summaryRequestIdRef.current = null;
      setSummaryProgress({ done: 0, total: 0, phase: "error" });
    }
  }, [bookId, refreshBookAiState]);

  const updateMessages = (updater: ChatMessage[] | ((prev: ChatMessage[]) => ChatMessage[])) => {
    if (!mountedRef.current) return;
    setMessages((prev) => {
      const next = typeof updater === "function" ? updater(prev) : updater;
      messagesRef.current = next;
      return next;
    });
  };

  const refreshChats = useCallback(async (bid: string) => {
    try {
      const result = await invoke<ChatRecord[]>("list_chats", { bookId: bid });
      if (mountedRef.current && bookIdRef.current === bid) setChats(result);
      return result;
    } catch {
      return [];
    }
  }, []);

  const loadChat = useCallback(async (id: string) => {
    // Stop any active stream
    if (streamingRef.current || activeRequestIdRef.current) stopActiveStream();
    titleGenerationRef.current += 1;
    setTitling(false);
    const targetBookId = bookIdRef.current;

    // Set target immediately so rapid clicks can be detected
    setChatId(id);
    chatIdRef.current = id;

    try {
      const msgs = await invoke<ChatMsgRecord[]>("list_chat_messages", { chatId: id });

      // Stale check: if user switched to another chat while we were loading, bail
      if (
        !mountedRef.current
        || chatIdRef.current !== id
        || bookIdRef.current !== targetBookId
      ) return;

      const mapped: ChatMessage[] = msgs.map((m) => {
        const metadata = parseMessageMetadata(m.metadata);
        return {
          id: m.id,
          role: m.role as "user" | "assistant",
          content: m.content,
          context: m.context ?? undefined,
          contextCfi: metadata.cfi,
          contextAnalysis: metadata.analysis,
          reasoning: metadata.reasoning,
          sources: metadata.sources,
          spoilerGuard: metadata.spoilerGuard,
          dbId: m.id,
        };
      });
      updateMessages(mapped);
    } catch (err) {
      console.error("Failed to load chat messages:", err);
    }
  }, [stopActiveStream]);

  const createChat = useCallback(async (bid: string) => {
    try {
      const chat = await invoke<ChatRecord>("create_chat", { bookId: bid, title: null, model: null });
      if (!mountedRef.current || bookIdRef.current !== bid) return chat;
      setChatId(chat.id);
      chatIdRef.current = chat.id;
      updateMessages([]);
      // Mark book as initialized so a later initialize() call won't reload a stale chat.
      initializedBookRef.current = bid;
      await refreshChats(bid);
      return chat;
    } catch (err) {
      console.error("Failed to create chat:", err);
      return null;
    }
  }, [refreshChats]);

  const initialize = useCallback(async (bid?: string) => {
    const targetBook = bid || bookId;
    if (!targetBook) { setInitializingSynced(false); return; }
    if (initializedBookRef.current === targetBook) { setInitializingSynced(false); return; }
    const generation = initializationGenerationRef.current + 1;
    initializationGenerationRef.current = generation;
    initializedBookRef.current = targetBook;
    setInitializingSynced(true);

    try {
      const chatList = await refreshChats(targetBook);
      if (
        !mountedRef.current
        || initializationGenerationRef.current !== generation
        || bookIdRef.current !== targetBook
      ) return;
      if (chatList.length > 0) {
        await loadChat(chatList[0].id);
      } else {
        // No chats yet — show empty state without creating a DB record.
        // A chat will be created lazily on first send.
        setChatId(null);
        chatIdRef.current = null;
        updateMessages([]);
      }
    } finally {
      if (initializationGenerationRef.current === generation) {
        setInitializingSynced(false);
      }
    }
  }, [bookId, refreshChats, loadChat]);

  const send = useCallback(
    async (
      content: string,
      context?: string,
      contextCfi?: string,
      contextAnalysis?: string,
      options?: { spoilerOverride?: boolean; replaceAssistantId?: string },
    ) => {
      // Refuse while the session chat is still loading — otherwise the lazy
      // chat-creation path below would spawn a *new* chat and miss the
      // existing one. Belt-and-suspenders alongside the UI gate.
      if (initializingRef.current || streamingRef.current) return;

      setGroundingStatus(null);
      if (settings.ai_summaries_auto !== "false") void prepareBookOverview();

      const requestId = crypto.randomUUID();
      const requestGeneration = streamGenerationRef.current + 1;
      streamGenerationRef.current = requestGeneration;
      activeRequestIdRef.current = requestId;
      streamingRef.current = true;
      setStreaming(true);
      const isRequestActive = () => (
        mountedRef.current
        && activeRequestIdRef.current === requestId
        && streamGenerationRef.current === requestGeneration
      );

      let currentChatId = chatIdRef.current;
      const currentBookId = bookId;

      // Lazy chat creation: if no chat exists yet, create one now
      const isNewChat = !currentChatId;
      if (!currentChatId && currentBookId) {
        const chat = await createChat(currentBookId);
        if (!isRequestActive()) return;
        if (!chat) {
          stopActiveStream(false);
          return;
        }
        currentChatId = chat.id;
      }
      if (!currentChatId) {
        stopActiveStream(false);
        return;
      }

      const replacingAssistant = options?.replaceAssistantId
        ? messagesRef.current.find((message) => message.id === options.replaceAssistantId && message.role === "assistant")
        : undefined;
      const replacementIndex = replacingAssistant
        ? messagesRef.current.findIndex((message) => message.id === replacingAssistant.id)
        : -1;
      const previousUser = replacementIndex > 0 && messagesRef.current[replacementIndex - 1]?.role === "user"
        ? messagesRef.current[replacementIndex - 1]
        : undefined;
      if (options?.replaceAssistantId && (!replacingAssistant || !previousUser)) {
        stopActiveStream(false);
        return;
      }
      activeReplacementRef.current = replacingAssistant ?? null;

      // New chat: generate the title from the user's message, concurrently with
      // the response stream (not after it), showing a loading state until it
      // lands. Falls back to a truncated title if the AI call fails.
      if (isNewChat && currentBookId) {
        const titleSource = context || content;
        const titleGeneration = titleGenerationRef.current + 1;
        titleGenerationRef.current = titleGeneration;
        setTitling(true);
        generateAiTitle(titleSource).then(async (aiTitle) => {
          try {
            // Only auto-title if the chat is still untitled — the user (or a
            // synced rename) may have renamed it while generation was pending.
            const chat = await invoke<ChatRecord>("get_chat", { chatId: currentChatId });
            if (chat.title === "New chat") {
              const title = aiTitle || deriveTitle(titleSource);
              if (title) {
                await invoke("rename_chat", { chatId: currentChatId, title });
                await refreshChats(currentBookId);
              }
            }
          } catch { /* ignore */ } finally {
            if (mountedRef.current && titleGenerationRef.current === titleGeneration) {
              setTitling(false);
            }
          }
        });
      }

      const userMessage: ChatMessage = previousUser ?? {
        id: nextMsgId(),
        role: "user",
        content,
        context,
        contextCfi,
        contextAnalysis,
      };

      const assistantId = replacingAssistant?.id ?? nextMsgId();
      const assistantMessage: ChatMessage = {
        id: assistantId,
        role: "assistant",
        content: "",
        dbId: replacingAssistant?.dbId,
      };
      activeAssistantIdRef.current = assistantId;

      const apiHistory = replacingAssistant
        ? messagesRef.current.slice(0, replacementIndex)
        : [...messagesRef.current, userMessage];
      updateMessages((prev) => replacingAssistant
        ? prev.map((message) => message.id === assistantId ? assistantMessage : message)
        : [...prev, userMessage, assistantMessage]);

      // Persist user message
      if (!replacingAssistant) {
        try {
          const meta = serializeMessageMetadata({ cfi: contextCfi, analysis: contextAnalysis });
          const saved = await invoke<ChatMsgRecord>("save_chat_message", {
            chatId: currentChatId,
            role: "user",
            content,
            context: context || null,
            metadata: meta,
          });
          userMessage.dbId = saved.id;
        } catch (err) {
          console.error("Failed to save user message:", err);
        }
      }
      if (!isRequestActive()) return;

      // Accumulate full assistant content for persistence
      let fullContent = "";
      let fullReasoning = "";
      let citedSources: CitedSource[] = [];
      let spoilerGuard: SpoilerGuardMetadata | undefined;
      let chatResultPromise: Promise<AiChatResult> | null = null;
      let pendingContent = "";
      let pendingReasoning = "";
      let updateFrame: number | null = null;

      const flushStreamUpdate = () => {
        if (updateFrame !== null) {
          cancelAnimationFrame(updateFrame);
          updateFrame = null;
        }
        if (!pendingContent && !pendingReasoning) return;
        const contentDelta = pendingContent;
        const reasoningDelta = pendingReasoning;
        pendingContent = "";
        pendingReasoning = "";
        if (!isRequestActive()) return;
        updateMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? {
                  ...m,
                  content: m.content + contentDelta,
                  reasoning: `${m.reasoning ?? ""}${reasoningDelta}` || undefined,
                }
              : m,
          ),
        );
      };

      const scheduleStreamUpdate = () => {
        if (updateFrame !== null) return;
        updateFrame = requestAnimationFrame(() => {
          updateFrame = null;
          if (!isRequestActive()) {
            pendingContent = "";
            pendingReasoning = "";
            return;
          }
          flushStreamUpdate();
        });
      };

      const discardStreamUpdate = () => {
        if (updateFrame !== null) {
          cancelAnimationFrame(updateFrame);
          updateFrame = null;
        }
        pendingContent = "";
        pendingReasoning = "";
      };
      streamFrameCleanupRef.current = discardStreamUpdate;

      let streamUnlisten: UnlistenFn | null = null;
      const finishRequest = () => {
        if (!isRequestActive()) return false;
        if (streamFrameCleanupRef.current === discardStreamUpdate) {
          streamFrameCleanupRef.current = null;
        }
        if (unlistenRef.current === streamUnlisten) unlistenRef.current = null;
        groundingUnlistenRef.current?.();
        groundingUnlistenRef.current = null;
        streamUnlisten?.();
        streamUnlisten = null;
        activeRequestIdRef.current = null;
        if (activeAssistantIdRef.current === assistantId) activeAssistantIdRef.current = null;
        streamingRef.current = false;
        if (mountedRef.current) {
          setStreaming(false);
          setGroundingStatus(null);
        }
        return true;
      };

      try {
        const registeredGroundingUnlisten = await listen<GroundingStatusEvent>(
          `ai-grounding-status-${requestId}`,
          (event) => {
            if (isRequestActive()) setGroundingStatus(event.payload.status);
          },
        );
        groundingUnlistenRef.current = registeredGroundingUnlisten;
        if (!isRequestActive()) {
          registeredGroundingUnlisten();
          return;
        }
      } catch {
        // The status hint is optional; streaming chat remains usable without it.
      }

      try {
        const registeredUnlisten = await listen<AiStreamChunk>(
          `ai-stream-chunk-${requestId}`,
          async (event) => {
            if (!isRequestActive()) return;
            if (event.payload.done) {
              flushStreamUpdate();

              if (event.payload.error) {
                const errorCode = getAiErrorCode(event.payload.error) ?? "AI_STREAM_FAILED";
                updateMessages((prev) =>
                  prev.map((m) =>
                    m.id === assistantId
                      ? replacingAssistant ?? { ...m, content: fullContent || errorCode }
                      : m
                  )
                );
                activeReplacementRef.current = null;
                finishRequest();
                return;
              }

              try {
                const result = await chatResultPromise;
                if (!result) throw new Error("AI_CHAT_RESULT_MISSING");
                if (!isRequestActive()) return;
                citedSources = result.sources;
                spoilerGuard = result.spoilerGuard;
                updateMessages((previous) => previous.map((message) => (
                  message.id === assistantId
                    ? { ...message, sources: citedSources, spoilerGuard }
                    : message
                )));
              } catch (err) {
                if (!isRequestActive()) return;
                const errorContent = getAiErrorCode(err) ?? "AI_STREAM_FAILED";
                updateMessages((previous) => previous.map((message) => (
                  message.id === assistantId
                    ? replacingAssistant ?? { ...message, content: errorContent }
                    : message
                )));
                activeReplacementRef.current = null;
                finishRequest();
                return;
              }

              finishRequest();
              activeReplacementRef.current = null;

              // Only a provider-confirmed completed stream may become history.
              if (fullContent) {
                try {
                  const metadata = serializeMessageMetadata({
                    reasoning: fullReasoning,
                    sources: citedSources,
                    spoilerGuard,
                  });
                  if (replacingAssistant?.dbId) {
                    await invoke("replace_chat_message", {
                      messageId: replacingAssistant.dbId,
                      content: fullContent,
                      metadata,
                    });
                  } else {
                    const saved = await invoke<ChatMsgRecord>("save_chat_message", {
                      chatId: currentChatId,
                      role: "assistant",
                      content: fullContent,
                      context: null,
                      metadata,
                    });
                    assistantMessage.dbId = saved.id;
                    updateMessages((previous) => previous.map((message) => (
                      message.id === assistantId ? { ...message, dbId: saved.id } : message
                    )));
                  }
                } catch (err) {
                  console.error("Failed to save assistant message:", err);
                }
              }

              return;
            }

            const contentDelta = event.payload.delta || "";
            const reasoningDelta = event.payload.reasoning_delta || "";
            fullContent += contentDelta;
            fullReasoning += reasoningDelta;
            pendingContent += contentDelta;
            pendingReasoning += reasoningDelta;
            scheduleStreamUpdate();
          },
        );
        streamUnlisten = registeredUnlisten;
        if (!isRequestActive()) {
          registeredUnlisten();
          discardStreamUpdate();
          return;
        }
        unlistenRef.current = registeredUnlisten;
      } catch (err) {
        if (!isRequestActive()) return;
        const errorContent = getAiErrorCode(err) ?? "AI_STREAM_FAILED";
        updateMessages((prev) => prev.map((message) => (
          message.id === assistantId
            ? replacingAssistant ?? { ...message, content: errorContent }
            : message
        )));
        activeReplacementRef.current = null;
        finishRequest();
        return;
      }

      // Build API messages from current ref (avoids stale closure)
      // Include source quotes and any generated learning-card analysis inline so
      // follow-up questions retain the full context without duplicating it in UI.
      const apiMessages = apiHistory
        .filter((message) => message.role === "user" || message.content.trim().length > 0)
        .map((m) => ({
          role: m.role,
          content: messageContentForApi(m),
        }));

      try {
        if (!isRequestActive()) return;
        chatResultPromise = invoke<unknown>("ai_chat", {
          messages: apiMessages,
          bookId: bookId ?? null,
          bookTitle: bookContext?.title ?? null,
          bookAuthor: bookContext?.author ?? null,
          currentChapter: bookContext?.chapter ?? null,
          requestId,
          spoilerOverride: options?.spoilerOverride ?? null,
        }).then(parseAiChatResult);
        const result = await chatResultPromise;
        citedSources = result.sources;
        spoilerGuard = result.spoilerGuard;
        if (isRequestActive()) {
          updateMessages((previous) => previous.map((message) => (
            message.id === assistantId
              ? { ...message, sources: citedSources, spoilerGuard }
              : message
          )));
        }
      } catch (err) {
        if (!isRequestActive()) return;
        flushStreamUpdate();
        const errorContent = getAiErrorCode(err) ?? "AI_STREAM_FAILED";
        updateMessages((prev) =>
          prev.map((m) =>
            m.id === assistantId
              ? replacingAssistant ?? { ...m, content: errorContent }
              : m
          )
        );
        activeReplacementRef.current = null;
        finishRequest();
      }
    },
    [bookId, bookContext?.title, bookContext?.author, bookContext?.chapter, createChat, prepareBookOverview, refreshChats, settings.ai_summaries_auto, stopActiveStream]
  );

  const retryWithWholeBook = useCallback((assistantId: string) => {
    const assistantIndex = messagesRef.current.findIndex((message) => message.id === assistantId);
    const userMessage = assistantIndex > 0 ? messagesRef.current[assistantIndex - 1] : undefined;
    if (!userMessage || userMessage.role !== "user") return;
    void send(
      userMessage.content,
      userMessage.context,
      userMessage.contextCfi,
      userMessage.contextAnalysis,
      { spoilerOverride: true, replaceAssistantId: assistantId },
    );
  }, [send]);

  const setSpoilerGuardEnabled = useCallback(async (enabled: boolean) => {
    if (!spoilerSettingKey) return;
    await saveSetting(spoilerSettingKey, enabled ? "on" : "off");
  }, [saveSetting, spoilerSettingKey]);

  const cancel = useCallback(() => {
    if (!activeRequestIdRef.current && !streamingRef.current) return;
    const assistantId = activeAssistantIdRef.current;
    const replacement = activeReplacementRef.current;
    activeReplacementRef.current = null;
    stopActiveStream();
    if (!assistantId || !mountedRef.current) return;
    setMessages((current) => {
      const next = replacement
        ? current.map((message) => message.id === assistantId ? replacement : message)
        : current.filter((message) => (
            message.id !== assistantId
            || Boolean(message.content.trim())
            || Boolean(message.reasoning?.trim())
          ));
      messagesRef.current = next;
      return next;
    });
  }, [stopActiveStream]);

  const deleteChat = useCallback(async (id: string) => {
    const currentBookId = bookId;
    if (!currentBookId) return;

    // Cancel active stream if deleting the streaming chat
    if (id === chatIdRef.current && streamingRef.current) {
      stopActiveStream();
    }

    await invoke("delete_chat", { chatId: id });
    const updatedChats = await refreshChats(currentBookId);

    if (id === chatIdRef.current) {
      if (updatedChats.length > 0) {
        await loadChat(updatedChats[0].id);
      } else {
        // No chats left — show empty state, lazy create on next send
        setChatId(null);
        chatIdRef.current = null;
        updateMessages([]);
      }
    }
  }, [bookId, refreshChats, loadChat, stopActiveStream]);

  const renameChat = useCallback(async (id: string, title: string) => {
    await invoke("rename_chat", { chatId: id, title });
    setChats((prev) => prev.map((c) => c.id === id ? { ...c, title } : c));
  }, []);

  const reset = useCallback(async () => {
    if (!bookId) return;
    // Stop any active stream
    if (streamingRef.current || activeRequestIdRef.current) stopActiveStream();
    titleGenerationRef.current += 1;
    setTitling(false);
    // Show empty state, lazy create on next send
    setChatId(null);
    chatIdRef.current = null;
    updateMessages([]);
  }, [bookId, stopActiveStream]);

  return {
    messages,
    streaming,
    groundingStatus,
    summaryProgress,
    bookAiState,
    summariesAuto: settings.ai_summaries_auto !== "false",
    spoilerGuardEnabled,
    setSpoilerGuardEnabled,
    prepareBookOverview,
    cancel,
    titling,
    initializing,
    send,
    retryWithWholeBook,
    reset,
    initialize,
    chatId,
    chats,
    loadChat,
    createChat,
    deleteChat,
    renameChat,
  };
}
