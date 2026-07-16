import { useEffect, useLayoutEffect, useMemo, useRef } from "react";
import {
  BookmarkPlus,
  Copy,
  Highlighter,
  Languages,
  MessageSquareMore,
  Sparkles,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  READER_CONTEXT_MENU_KEY_EVENT,
  readerMenuActivationIndex,
  readerMenuFocusIndex,
  type ReaderContextMenuKeyDetail,
  type InteractionKind,
  type SerializableRect,
} from "./reader-interaction";

export type ReaderMenuAction = "primary" | "ask-ai" | "save" | "highlight" | "translate" | "copy" | `custom_${string}`;

interface ReaderContextMenuProps {
  anchorRect: SerializableRect;
  text: string;
  kind: InteractionKind;
  marked?: boolean;
  hasBookWordMark?: boolean;
  markStateLoading?: boolean;
  showTranslate?: boolean;
  order?: ReaderMenuAction[];
  onClose: () => void;
  onCopy: () => void;
  onExplain: () => void;
  onQuote: () => void;
  onLookup: () => void;
  onTranslate: () => void;
  onSave: () => void;
  onToggleMark?: () => void;
  onRemoveBookWordMark?: () => void;
  customActions?: Array<{ id: `custom_${string}`; name: string }>;
  onCustomAction?: (id: `custom_${string}`) => void;
}

export default function ReaderContextMenu({
  anchorRect,
  text,
  kind,
  marked = false,
  hasBookWordMark = false,
  markStateLoading = false,
  showTranslate = false,
  order = ["primary", "ask-ai", "save", "highlight", "copy"],
  onClose,
  onCopy,
  onExplain,
  onQuote,
  onLookup,
  onTranslate,
  onSave,
  onToggleMark,
  onRemoveBookWordMark,
  customActions = [],
  onCustomAction,
}: ReaderContextMenuProps) {
  const { t } = useTranslation();
  const menuRef = useRef<HTMLDivElement>(null);
  const actions = useMemo(() => {
    const values = [...order];
    if (showTranslate && !values.includes("translate")) values.splice(1, 0, "translate");
    return values.filter((action) => action !== "highlight" || onToggleMark);
  }, [onToggleMark, order, showTranslate]);

  useEffect(() => {
    // Moving focus on mount hides WebKit's native reader selection.
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) onClose();
    };
    const handleMenuKey = (key: string, shiftKey = false, modified = false) => {
      const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role='menuitem']:not(:disabled)") ?? []);
      if (key === "Escape") {
        onClose();
        return true;
      }
      const current = items.indexOf(document.activeElement as HTMLButtonElement);
      const activation = readerMenuActivationIndex(key, current, items.length, modified);
      if (activation !== null) {
        items[activation]?.click();
        return true;
      }
      const next = readerMenuFocusIndex(key, current, items.length, shiftKey, modified);
      if (next === null) return false;
      items[next]?.focus();
      return true;
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!handleMenuKey(
        event.key,
        event.shiftKey,
        event.altKey || event.ctrlKey || event.metaKey,
      )) return;
      event.preventDefault();
    };
    const handleReaderKey = (event: Event) => {
      const detail = (event as CustomEvent<ReaderContextMenuKeyDetail>).detail;
      if (handleMenuKey(detail.key, detail.shiftKey, detail.modified)) detail.handled = true;
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleKeyDown);
    window.addEventListener(READER_CONTEXT_MENU_KEY_EVENT, handleReaderKey);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener(READER_CONTEXT_MENU_KEY_EVENT, handleReaderKey);
    };
  }, [onClose]);

  useLayoutEffect(() => {
    const element = menuRef.current;
    if (!element) return;
    const positionMenu = () => {
      const rect = element.getBoundingClientRect();
      const gap = 8;
      const roomRight = window.innerWidth - anchorRect.right - gap;
      const roomLeft = anchorRect.left - gap;
      const canPlaceBeside = roomRight >= rect.width || roomLeft >= rect.width;
      const left = roomRight >= rect.width
        ? anchorRect.right + gap
        : roomLeft >= rect.width
          ? anchorRect.left - rect.width - gap
          : Math.max(gap, Math.min(anchorRect.right - rect.width, window.innerWidth - rect.width - gap));
      const top = canPlaceBeside
        ? Math.max(gap, Math.min(anchorRect.top, window.innerHeight - rect.height - gap))
        : anchorRect.bottom + gap + rect.height <= window.innerHeight
          ? anchorRect.bottom + gap
          : Math.max(gap, anchorRect.top - rect.height - gap);
      element.style.left = `${left}px`;
      element.style.top = `${top}px`;
    };
    positionMenu();
    const observer = new ResizeObserver(positionMenu);
    observer.observe(element);
    window.addEventListener("resize", positionMenu);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", positionMenu);
    };
  }, [anchorRect]);

  const primaryIsLookup = kind !== "passage";
  const definitions: Record<string, { label: string; icon: typeof Sparkles; run: () => void }> = {
    primary: {
      label: kind === "word"
        ? t("contextMenu.lookUp", { defaultValue: "查词" })
        : primaryIsLookup
          ? t("contextMenu.definePhrase", { defaultValue: "释义" })
          : t("contextMenu.interpretPassage", { defaultValue: "解读" }),
      icon: Sparkles,
      run: primaryIsLookup ? onLookup : onExplain,
    },
    "ask-ai": {
      label: t("contextMenu.askAi", { defaultValue: "问 AI" }),
      icon: MessageSquareMore,
      run: onQuote,
    },
    save: {
      label: t("contextMenu.save", { defaultValue: "收藏" }),
      icon: BookmarkPlus,
      run: onSave,
    },
    highlight: {
      label: marked
        ? kind === "word"
          ? t("contextMenu.removeCurrentMark", { defaultValue: "取消当前标记" })
          : t("contextMenu.removeHighlight", { defaultValue: "取消标记" })
        : t("contextMenu.mark", { defaultValue: "标记" }),
      icon: Highlighter,
      run: onToggleMark ?? (() => {}),
    },
    translate: {
      label: t("contextMenu.translateOnly", { defaultValue: "仅翻译" }),
      icon: Languages,
      run: onTranslate,
    },
    copy: {
      label: t("contextMenu.copy"),
      icon: Copy,
      run: onCopy,
    },
    ...Object.fromEntries(customActions.map((action) => [action.id, {
      label: action.name,
      icon: Sparkles,
      run: () => onCustomAction?.(action.id),
    }])),
  };

  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label={text}
      className="fixed z-50 w-[220px] rounded-md border border-border bg-bg-surface py-1 shadow-context"
      style={{ left: anchorRect.right, top: anchorRect.bottom + 8 }}
    >
      {actions.map((action) => {
        const definition = definitions[action];
        if (!definition) return null;
        const Icon = definition.icon;
        const showRemoveBookWordMark = action === "highlight"
          && kind === "word"
          && marked
          && hasBookWordMark
          && onRemoveBookWordMark;
        return (
          <div key={action}>
            <button
              type="button"
              role="menuitem"
              onClick={definition.run}
              disabled={action === "highlight" && markStateLoading}
              aria-busy={action === "highlight" && markStateLoading ? true : undefined}
              className="mx-1 flex h-9 w-[calc(100%-8px)] items-center gap-3 rounded-sm px-3 text-left text-[13px] font-medium text-text-primary hover:bg-accent-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:cursor-wait disabled:opacity-50"
            >
              <Icon size={16} className="shrink-0 text-text-muted" />
              <span className="min-w-0 flex-1 truncate">{definition.label}</span>
              {action === "copy" ? (
                <span className="text-[11px] text-text-muted">{navigator.platform.includes("Mac") ? "⌘C" : "Ctrl+C"}</span>
              ) : null}
            </button>
            {showRemoveBookWordMark && (
              <button
                type="button"
                role="menuitem"
                onClick={onRemoveBookWordMark}
                className="mx-1 flex h-9 w-[calc(100%-8px)] items-center gap-3 rounded-sm px-3 text-left text-[13px] font-medium text-text-primary hover:bg-accent-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
              >
                <Highlighter size={16} className="shrink-0 text-text-muted" />
                <span className="min-w-0 flex-1 truncate">
                  {t("contextMenu.removeBookWordMark", { defaultValue: "取消全书同词标记" })}
                </span>
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
