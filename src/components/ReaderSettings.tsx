import { useState, useRef, useEffect, useCallback, useLayoutEffect } from "react";
import { useTranslation } from "react-i18next";
import { Sun, Check, ScrollText, BookOpen, File, Files, Keyboard, Loader2, MousePointer2, Trash2 } from "lucide-react";
import Toggle from "./ui/Toggle";
import Select from "./ui/Select";
import {
  bindingFromKeyboardEvent,
  bindingFromMouseEvent,
  formatPageTurnBinding,
} from "./page-turn-bindings";
import {
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  fonts,
  getReaderThemes,
  type ReaderCapabilities,
  type ReaderFont,
  type ReaderCustomTheme,
  type ReaderTheme,
} from "./reader-settings";

const sliderClass =
  "w-full h-1 cursor-pointer appearance-none rounded-full bg-border [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-3.5 [&::-webkit-slider-thumb]:h-3.5 [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-bg-surface [&::-webkit-slider-thumb]:border [&::-webkit-slider-thumb]:border-border [&::-webkit-slider-thumb]:shadow-sm";

export type ReadingMode = "scrolling" | "paginated";
export type PageColumns = 1 | 2;
export type PageTurnAnimation = "none" | "slide" | "fade" | "cover";

export interface ReaderSettingsState {
  theme: ReaderTheme;
  customTheme: ReaderCustomTheme;
  font: ReaderFont;
  fontSize: number; // px
  brightness: number; // 0-100
  readingMode: ReadingMode;
  pageColumns: PageColumns; // 1 = single page, 2 = two pages side by side
  pageTurnAnimation: PageTurnAnimation;
  showChapterProgress: boolean;
  showBookProgress: boolean;
  showPageNumbers: boolean;
  previousPageBinding: string;
  nextPageBinding: string;
  lineSpacing: number; // multiplier, e.g. 1.5
  charSpacing: number; // percentage, 0 = normal
  wordSpacing: number; // percentage, 0 = normal
  margins: number; // percentage of the available reading width
  showLookupMarkers: boolean;
  showNewVocabMarkers: boolean;
  showLearningMarkers: boolean;
  showMasteredMarkers: boolean;
}

interface ReaderSettingsProps {
  open: boolean;
  onClose: () => void;
  anchorRef: React.RefObject<HTMLElement | null>;
  settings: ReaderSettingsState;
  onSettingsChange: (settings: ReaderSettingsState) => void;
  capabilities: ReaderCapabilities;
  onClearLookupMarks?: () => Promise<void>;
}

type BindingDirection = "previous" | "next";

function PageTurnBindingButton({
  direction,
  value,
  active,
  onActivate,
  onChange,
}: {
  direction: BindingDirection;
  value: string;
  active: boolean;
  onActivate: (direction: BindingDirection | null) => void;
  onChange: (value: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const suppressContextMenuUntilRef = useRef(0);

  useEffect(() => {
    if (!active) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onActivate(null);
        return;
      }
      const binding = bindingFromKeyboardEvent(event);
      if (!binding) return;
      event.preventDefault();
      event.stopPropagation();
      onChange(binding);
      onActivate(null);
    };
    const onMouseDown = (event: MouseEvent) => {
      const binding = bindingFromMouseEvent(event);
      if (!binding) return;
      if (event.button === 2) suppressContextMenuUntilRef.current = Date.now() + 800;
      event.preventDefault();
      event.stopPropagation();
      onChange(binding);
      onActivate(null);
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("mousedown", onMouseDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("mousedown", onMouseDown, true);
    };
  }, [active, onActivate, onChange]);

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      if (!active && Date.now() > suppressContextMenuUntilRef.current) return;
      suppressContextMenuUntilRef.current = 0;
      event.preventDefault();
      event.stopPropagation();
    };
    window.addEventListener("contextmenu", onContextMenu, true);
    return () => window.removeEventListener("contextmenu", onContextMenu, true);
  }, [active]);

  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={() => onActivate(active ? null : direction)}
      className={`inline-flex h-8 min-w-[92px] items-center justify-center gap-1.5 rounded-md border px-2 text-[12px] font-medium transition-colors ${
        active
          ? "border-accent bg-accent-bg text-accent-text"
          : "border-border bg-bg-input text-text-secondary hover:border-accent/50"
      }`}
    >
      {value.startsWith("mouse:") ? <MousePointer2 size={13} /> : <Keyboard size={13} />}
      <span>{active ? t("readerSettings.pressBinding") : formatPageTurnBinding(value, i18n.language)}</span>
    </button>
  );
}

export default function ReaderSettings({
  open,
  onClose,
  anchorRef,
  settings,
  onSettingsChange,
  capabilities,
  onClearLookupMarks,
}: ReaderSettingsProps) {
  const { t } = useTranslation();
  const popoverRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ top: 0, right: 8, maxHeight: 0 });
  const [clearLookupConfirm, setClearLookupConfirm] = useState(false);
  const [clearLookupBusy, setClearLookupBusy] = useState(false);
  const [clearLookupError, setClearLookupError] = useState(false);
  const [capturingBinding, setCapturingBinding] = useState<BindingDirection | null>(null);

  useLayoutEffect(() => {
    if (!open) return;
    const updatePosition = () => {
      if (!anchorRef.current) return;
      const rect = anchorRef.current.getBoundingClientRect();
      const top = Math.max(8, rect.bottom + 4);
      const maxRight = Math.max(8, window.innerWidth - 320 - 8);
      setPosition({
        top,
        right: Math.max(8, Math.min(window.innerWidth - rect.right, maxRight)),
        maxHeight: Math.max(0, Math.min(760, window.innerHeight - top - 8)),
      });
    };
    updatePosition();
    window.addEventListener("resize", updatePosition);
    return () => window.removeEventListener("resize", updatePosition);
  }, [open, anchorRef]);

  useEffect(() => {
    if (!open) return;
    const handleClick = (e: MouseEvent) => {
      if (
        popoverRef.current &&
        !popoverRef.current.contains(e.target as Node) &&
        anchorRef.current &&
        !anchorRef.current.contains(e.target as Node)
      ) {
        onClose();
      }
    };
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [open, onClose, anchorRef]);

  useEffect(() => {
    if (open) return;
    setClearLookupConfirm(false);
    setClearLookupError(false);
    setCapturingBinding(null);
  }, [open]);

  const update = useCallback((partial: Partial<ReaderSettingsState>) => {
    onSettingsChange({ ...settings, ...partial });
  }, [onSettingsChange, settings]);

  const themeLabels: Record<string, string> = {
    original: t("readerSettings.themeOriginal"),
    paper: t("readerSettings.themeSepia"),
    quiet: t("readerSettings.themeGray"),
    dark: t("readerSettings.themeDark"),
    custom: t("readerSettings.themeCustom"),
  };

  if (!open) return null;

  return (
    <div
      ref={popoverRef}
      data-reader-settings
      className="fixed z-50 flex w-[320px] max-w-[calc(100dvw-16px)] flex-col overflow-y-auto rounded-lg border border-border bg-bg-surface shadow-popover"
      style={{ top: position.top, right: position.right, maxHeight: position.maxHeight }}
    >
      {/* Font size toggle */}
      {capabilities.supportsReflowSettings && (<div className="flex items-center h-[60px] px-4 border-b border-border-light">
        <button
          onClick={() => update({ fontSize: Math.max(FONT_SIZE_MIN, settings.fontSize - 2) })}
          className="flex-1 flex items-center justify-center h-7 border-r border-border cursor-pointer text-text-primary hover:bg-bg-input"
        >
          <span className="text-[14px] font-medium">A-</span>
        </button>
        <span className="flex-1 flex items-center justify-center text-[14px] font-medium text-text-primary">
          {settings.fontSize}px
        </span>
        <button
          onClick={() => update({ fontSize: Math.min(FONT_SIZE_MAX, settings.fontSize + 2) })}
          className="flex-1 flex items-center justify-center h-9 border-l border-border cursor-pointer text-text-primary hover:bg-bg-input"
        >
          <span className="text-[20px] font-medium tracking-[-0.45px]">A+</span>
        </button>
      </div>)}

      {/* Brightness slider */}
      <div className="flex items-center gap-3 h-[42px] px-4 border-b border-border-light">
        <Sun size={14} className="text-text-muted shrink-0" />
        <input
          type="range"
          min={0}
          max={100}
          value={settings.brightness}
          onChange={(e) => update({ brightness: Number(e.target.value) })}
          className={`flex-1 ${sliderClass}`}
        />
        <Sun size={18} className="text-text-muted shrink-0" />
      </div>

      {/* Theme selector */}
      <div className={`flex items-center justify-center gap-5 h-[78px] ${capabilities.supportsReflowSettings ? "border-b border-border-light" : ""}`}>
        {getReaderThemes().map((theme) => (
          <button
            key={theme.id}
            onClick={() => update({ theme: theme.id })}
            className="flex flex-col items-center gap-1.5 cursor-pointer"
          >
            <div
              className={`size-8 rounded-full ${theme.color} flex items-center justify-center ${
                settings.theme === theme.id ? "ring-2 ring-accent ring-offset-2 ring-offset-bg-surface" : ""
              }`}
              style={theme.id === "custom" ? { backgroundColor: settings.customTheme.color } : undefined}
            >
              {settings.theme === theme.id && (
                <Check
                  size={14}
                  className={theme.id === "dark" || theme.id === "quiet" ? "text-white" : "text-accent"}
                />
              )}
            </div>
            <span className="text-[10px] font-medium text-text-muted tracking-[0.12px]">
              {themeLabels[theme.id]}
            </span>
          </button>
        ))}
      </div>

      {/* Font family — hidden for PDFs */}
      {capabilities.supportsReflowSettings && (
      <div className="px-4 py-3 border-b border-border-light">
        <p className="text-[11px] font-medium text-text-muted tracking-[0.5px] uppercase mb-2">{t("readerSettings.font")}</p>
        <Select
          value={settings.font}
          onChange={(v) => update({ font: v as ReaderFont })}
          options={fonts.map((f) => ({ value: f.id, label: f.label }))}
        />
      </div>
      )}

      {capabilities.supportsContinuousScroll && (<div className="px-4 py-3 border-b border-border-light">
        <p className="text-[11px] font-medium text-text-muted tracking-[0.5px] uppercase mb-2">{t("readerSettings.readingMode")}</p>
        <div className="flex gap-2">
          <button
            onClick={() => update({ readingMode: "scrolling" })}
            className={`flex-1 flex flex-col items-center gap-1.5 py-2.5 rounded-lg border cursor-pointer transition-colors ${
              settings.readingMode === "scrolling"
                ? "border-accent bg-accent-bg text-accent"
                : "border-border bg-bg-surface text-text-primary hover:bg-bg-input"
            }`}
          >
            <ScrollText size={20} />
            <span className="text-[12px] font-medium">{t("readerSettings.scrolling")}</span>
          </button>
          <button
            onClick={() => update({ readingMode: "paginated" })}
            className={`flex-1 flex flex-col items-center gap-1.5 py-2.5 rounded-lg border cursor-pointer transition-colors ${
              settings.readingMode === "paginated"
                ? "border-accent bg-accent-bg text-accent"
                : "border-border bg-bg-surface text-text-primary hover:bg-bg-input"
            }`}
          >
            <BookOpen size={20} />
            <span className="text-[12px] font-medium">{t("readerSettings.pageTurning")}</span>
          </button>
        </div>
      </div>)}

      {/* Page columns — only formats whose renderer supports a spread. */}
      {capabilities.supportsSpread && settings.readingMode === "paginated" && (<div className="px-4 py-3 border-b border-border-light">
        <p className="text-[11px] font-medium text-text-muted tracking-[0.5px] uppercase mb-2">{t("readerSettings.pageLayout")}</p>
        <div className="flex gap-2">
          <button
            onClick={() => update({ pageColumns: 1 })}
            className={`flex-1 flex flex-col items-center gap-1.5 py-2.5 rounded-lg border cursor-pointer transition-colors ${
              settings.pageColumns === 1
                ? "border-accent bg-accent-bg text-accent"
                : "border-border bg-bg-surface text-text-primary hover:bg-bg-input"
            }`}
          >
            <File size={20} />
            <span className="text-[12px] font-medium">{t("readerSettings.singlePage")}</span>
          </button>
          <button
            onClick={() => update({ pageColumns: 2 })}
            className={`flex-1 flex flex-col items-center gap-1.5 py-2.5 rounded-lg border cursor-pointer transition-colors ${
              settings.pageColumns === 2
                ? "border-accent bg-accent-bg text-accent"
                : "border-border bg-bg-surface text-text-primary hover:bg-bg-input"
            }`}
          >
            <Files size={20} />
            <span className="text-[12px] font-medium">{t("readerSettings.twoPages")}</span>
          </button>
        </div>
        <p className="mt-2 text-[11px] leading-4 text-text-muted">
          {t("readerSettings.twoPagesHint")}
        </p>
      </div>)}

      {settings.readingMode === "paginated" && (
        <div className="border-b border-border-light px-4 py-3">
          <p className="mb-2 text-[11px] font-medium uppercase text-text-muted">{t("readerSettings.pageTurnAnimation")}</p>
          <Select
            value={settings.pageTurnAnimation}
            onChange={(value) => update({ pageTurnAnimation: value as PageTurnAnimation })}
            options={[
              { value: "slide", label: t("readerSettings.animationSlide") },
              { value: "fade", label: t("readerSettings.animationFade") },
              { value: "cover", label: t("readerSettings.animationCover") },
              { value: "none", label: t("readerSettings.animationNone") },
            ]}
          />
        </div>
      )}

      <div className="border-b border-border-light px-4 py-3">
        <p className="mb-2 text-[11px] font-medium uppercase text-text-muted">{t("readerSettings.progressDisplay")}</p>
        <div className="flex flex-col gap-2.5">
          <div className="flex items-center justify-between gap-4">
            <span className="text-[13px] text-text-primary">{t("readerSettings.chapterProgressAlways")}</span>
            <Toggle
              label={t("readerSettings.chapterProgressAlways")}
              checked={settings.showChapterProgress}
              onChange={(checked) => update({ showChapterProgress: checked })}
            />
          </div>
          <div className="flex items-center justify-between gap-4">
            <span className="text-[13px] text-text-primary">{t("readerSettings.bookProgress")}</span>
            <Toggle
              label={t("readerSettings.bookProgress")}
              checked={settings.showBookProgress}
              onChange={(checked) => update({ showBookProgress: checked })}
            />
          </div>
          {settings.readingMode === "paginated" && (
            <div className="flex items-center justify-between gap-4">
              <span className="text-[13px] text-text-primary">{t("readerSettings.pageNumbers")}</span>
              <Toggle
                label={t("readerSettings.pageNumbers")}
                checked={settings.showPageNumbers}
                onChange={(checked) => update({ showPageNumbers: checked })}
              />
            </div>
          )}
        </div>
      </div>

      <div className="border-b border-border-light px-4 py-3">
        <p className="text-[11px] font-medium uppercase text-text-muted">{t("readerSettings.pageTurnBindings")}</p>
        <p className="mb-3 mt-1 text-[11px] leading-4 text-text-muted">{t("readerSettings.pageTurnBindingsHint")}</p>
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-3">
            <span className="text-[13px] text-text-primary">{t("readerSettings.previousPage")}</span>
            <PageTurnBindingButton
              direction="previous"
              value={settings.previousPageBinding}
              active={capturingBinding === "previous"}
              onActivate={setCapturingBinding}
              onChange={(value) => update({
                previousPageBinding: value,
                ...(value === settings.nextPageBinding
                  ? { nextPageBinding: settings.previousPageBinding }
                  : {}),
              })}
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="text-[13px] text-text-primary">{t("readerSettings.nextPage")}</span>
            <PageTurnBindingButton
              direction="next"
              value={settings.nextPageBinding}
              active={capturingBinding === "next"}
              onActivate={setCapturingBinding}
              onChange={(value) => update({
                nextPageBinding: value,
                ...(value === settings.previousPageBinding
                  ? { previousPageBinding: settings.nextPageBinding }
                  : {}),
              })}
            />
          </div>
        </div>
      </div>

      {capabilities.supportsReflowSettings && (<div className="px-4 py-3 flex flex-col gap-4">
        <p className="text-[11px] font-medium text-text-muted tracking-[0.5px] uppercase">{t("readerSettings.layout")}</p>

        {/* Line Spacing */}
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium text-text-primary">{t("readerSettings.lineSpacing")}</span>
            <span className="text-[13px] text-text-muted">{settings.lineSpacing}</span>
          </div>
          <input
            type="range"
            min={1}
            max={3}
            step={0.1}
            value={settings.lineSpacing}
            onChange={(e) => update({ lineSpacing: Number(e.target.value) })}
            className={sliderClass}
          />
        </div>

        {/* Character Spacing */}
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium text-text-primary">{t("readerSettings.charSpacing")}</span>
            <span className="text-[13px] text-text-muted">{settings.charSpacing}%</span>
          </div>
          <input
            type="range"
            min={-5}
            max={20}
            step={1}
            value={settings.charSpacing}
            onChange={(e) => update({ charSpacing: Number(e.target.value) })}
            className={sliderClass}
          />
        </div>

        {/* Word Spacing */}
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium text-text-primary">{t("readerSettings.wordSpacing")}</span>
            <span className="text-[13px] text-text-muted">{settings.wordSpacing}%</span>
          </div>
          <input
            type="range"
            min={-10}
            max={50}
            step={1}
            value={settings.wordSpacing}
            onChange={(e) => update({ wordSpacing: Number(e.target.value) })}
            className={sliderClass}
          />
        </div>

        {/* Margins */}
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium text-text-primary">{t("readerSettings.margins")}</span>
            <span className="text-[13px] text-text-muted">{settings.margins}%</span>
          </div>
          <input
            type="range"
            min={0}
            max={30}
            step={1}
            value={settings.margins}
            onChange={(e) => update({ margins: Number(e.target.value) })}
            className={sliderClass}
          />
        </div>
      </div>)}

      {capabilities.supportsWordMarkers && (
        <div className="px-4 py-3 border-t border-border-light flex flex-col gap-3">
          <p className="text-[11px] font-medium text-text-muted tracking-[0.5px] uppercase">{t("readerSettings.wordMarkers")}</p>
          {[
            ["showLookupMarkers", "readerSettings.lookupMarkers"],
            ["showNewVocabMarkers", "readerSettings.newVocabMarkers"],
            ["showLearningMarkers", "readerSettings.learningMarkers"],
            ["showMasteredMarkers", "readerSettings.masteredMarkers"],
          ].map(([key, label]) => (
            <div key={key} className="flex items-center justify-between gap-4">
              <span className="text-[13px] text-text-primary">{t(label)}</span>
              <Toggle
                label={t(label)}
                checked={settings[key as keyof Pick<ReaderSettingsState, "showLookupMarkers" | "showNewVocabMarkers" | "showLearningMarkers" | "showMasteredMarkers">]}
                onChange={(checked) => update({ [key]: checked } as Partial<ReaderSettingsState>)}
              />
            </div>
          ))}
        </div>
      )}

      {onClearLookupMarks && (
        <div className="px-4 py-3 border-t border-border-light">
          {clearLookupConfirm ? (
            <div className="flex flex-col gap-2">
              <p className="text-[12px] leading-5 text-text-muted">
                {t("readerSettings.clearLookupMarksConfirm")}
              </p>
              {clearLookupError && (
                <p role="alert" className="text-[12px] text-danger-text">
                  {t("readerSettings.clearLookupMarksFailed")}
                </p>
              )}
              <div className="flex justify-end gap-2">
                <button
                  type="button"
                  disabled={clearLookupBusy}
                  className="h-8 px-2 rounded-md text-[12px] text-text-muted hover:bg-bg-input disabled:opacity-50"
                  onClick={() => {
                    setClearLookupConfirm(false);
                    setClearLookupError(false);
                  }}
                >
                  {t("common.cancel")}
                </button>
                <button
                  type="button"
                  disabled={clearLookupBusy}
                  className="h-8 px-2 rounded-md inline-flex items-center gap-1.5 text-[12px] text-danger-text hover:bg-danger-bg disabled:opacity-50"
                  onClick={async () => {
                    setClearLookupBusy(true);
                    setClearLookupError(false);
                    try {
                      await onClearLookupMarks();
                      setClearLookupConfirm(false);
                    } catch {
                      setClearLookupError(true);
                    } finally {
                      setClearLookupBusy(false);
                    }
                  }}
                >
                  {clearLookupBusy ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
                  {t("readerSettings.clearLookupMarksAction")}
                </button>
              </div>
            </div>
          ) : (
            <button
              type="button"
              className="h-8 -mx-2 px-2 rounded-md inline-flex items-center gap-2 text-[12px] text-danger-text hover:bg-danger-bg"
              onClick={() => setClearLookupConfirm(true)}
            >
              <Trash2 size={13} />
              {t("readerSettings.clearLookupMarks")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
