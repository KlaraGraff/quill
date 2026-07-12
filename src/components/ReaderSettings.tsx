import { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Sun, Check, ScrollText, BookOpen, File, Files } from "lucide-react";
import Toggle from "./ui/Toggle";
import Select from "./ui/Select";
import {
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  fonts,
  getReaderThemes,
  type ReaderCapabilities,
  type ReaderFont,
  type ReaderTheme,
} from "./reader-settings";

const sliderClass =
  "w-full h-1 cursor-pointer appearance-none rounded-full bg-border [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-3.5 [&::-webkit-slider-thumb]:h-3.5 [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-bg-surface [&::-webkit-slider-thumb]:border [&::-webkit-slider-thumb]:border-border [&::-webkit-slider-thumb]:shadow-sm";

export type ReadingMode = "scrolling" | "paginated";
export type PageColumns = 1 | 2;

export interface ReaderSettingsState {
  theme: ReaderTheme;
  font: ReaderFont;
  fontSize: number; // px
  brightness: number; // 0-100
  readingMode: ReadingMode;
  pageColumns: PageColumns; // 1 = single page, 2 = two pages side by side
  lineSpacing: number; // multiplier, e.g. 1.5
  charSpacing: number; // percentage, 0 = normal
  wordSpacing: number; // percentage, 0 = normal
  margins: number; // pixels, 0 = none
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
}

export default function ReaderSettings({ open, onClose, anchorRef, settings, onSettingsChange, capabilities }: ReaderSettingsProps) {
  const { t } = useTranslation();
  const popoverRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ top: 0, right: 0 });

  useEffect(() => {
    if (open && anchorRef.current) {
      const rect = anchorRef.current.getBoundingClientRect();
      setPosition({
        top: rect.bottom + 4,
        right: window.innerWidth - rect.right,
      });
    }
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

  const update = (partial: Partial<ReaderSettingsState>) => {
    onSettingsChange({ ...settings, ...partial });
  };

  const themeLabels: Record<string, string> = {
    original: t("readerSettings.themeOriginal"),
    paper: t("readerSettings.themeSepia"),
    quiet: t("readerSettings.themeGray"),
    dark: t("readerSettings.themeDark"),
  };

  if (!open) return null;

  return (
    <div
      ref={popoverRef}
      className="fixed z-50 w-[280px] bg-bg-surface border border-border rounded-xl shadow-popover flex flex-col"
      style={{ top: position.top, right: position.right }}
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
      {capabilities.supportsSpread && (<div className="px-4 py-3 border-b border-border-light">
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
      </div>)}

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
            <span className="text-[13px] text-text-muted">{settings.margins}px</span>
          </div>
          <input
            type="range"
            min={0}
            max={120}
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
                checked={settings[key as keyof Pick<ReaderSettingsState, "showLookupMarkers" | "showNewVocabMarkers" | "showLearningMarkers" | "showMasteredMarkers">]}
                onChange={(checked) => update({ [key]: checked } as Partial<ReaderSettingsState>)}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
