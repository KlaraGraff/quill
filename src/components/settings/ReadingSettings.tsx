import { useCallback, useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Check, Download, Trash2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import Select from "../ui/Select";
import ColorControl from "../ui/ColorControl";
import {
  customFontFamily,
  fonts,
  FONT_SIZE_MIN,
  FONT_SIZE_MAX,
  getDefaultReaderTheme,
  parseReaderCustomTheme,
  getCustomThemeStyles,
  type ReaderCustomTheme,
  type ReaderTheme,
} from "../reader-settings";
import { loadCustomFonts, type CustomFontRecord } from "../custom-fonts";
import { notifyReadingAssistanceSettingsChanged } from "../reading-assistance-events";
import { ROW_CONTROL_WIDTH, type SettingsProps } from "./types";

const READER_THEME_OPTIONS: {
  value: ReaderTheme;
  labelKey: string;
  swatchClass: string;
  checkClass: string;
}[] = [
  {
    value: "original",
    labelKey: "readerSettings.themeOriginal",
    swatchClass: "bg-reader-original-bg border border-reader-original-border",
    checkClass: "text-accent",
  },
  {
    value: "paper",
    labelKey: "readerSettings.themeSepia",
    swatchClass: "bg-reader-paper-bg",
    checkClass: "text-accent",
  },
  {
    value: "quiet",
    labelKey: "readerSettings.themeGray",
    swatchClass: "bg-reader-quiet-bg",
    checkClass: "text-white",
  },
  {
    value: "dark",
    labelKey: "readerSettings.themeDark",
    swatchClass: "bg-reader-dark-bg border border-reader-dark-border",
    checkClass: "text-white",
  },
  {
    value: "custom",
    labelKey: "readerSettings.themeCustom",
    swatchClass: "border border-reader-original-border",
    checkClass: "text-accent",
  },
];

const CUSTOM_THEME_PRESETS = ["#F4E6C7", "#DDE8D8", "#DDE7F1", "#E7DDEC", "#D8D9DC"] as const;

function NumberInput({ value, onChange, onBlur, suffix, min, max }: {
  value: number;
  onChange: (v: number) => void;
  onBlur: () => void;
  suffix?: string;
  min?: number;
  max?: number;
}) {
  return (
    <div className="flex items-center gap-1 shrink-0 w-[90px] justify-end">
      <input
        type="number"
        value={value}
        onChange={(e) => {
          const v = Number(e.target.value);
          if (min !== undefined && v < min) return;
          if (max !== undefined && v > max) return;
          onChange(v);
        }}
        onBlur={onBlur}
        onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }}
        className="w-[64px] h-8 bg-white dark:bg-bg-surface rounded-[10px] px-2 text-[13px] font-medium text-text-secondary text-center outline-none border border-border focus:border-accent transition-colors [appearance:textfield] [&::-webkit-outer-spin-button]:appearance-none [&::-webkit-inner-spin-button]:appearance-none"
      />
      <span className="text-[12px] text-text-muted w-[16px] text-left">{suffix}</span>
    </div>
  );
}

export default function ReadingSettings({ settings, loading, refresh, save, saveBulk, showSavedToast }: SettingsProps) {
  const { t } = useTranslation();
  const [readerTheme, setReaderTheme] = useState<ReaderTheme>(getDefaultReaderTheme());
  const [customTheme, setCustomTheme] = useState<ReaderCustomTheme>(() => parseReaderCustomTheme(null));
  const [fontFamily, setFontFamily] = useState("georgia");
  const [fontSize, setFontSize] = useState(26);
  const [lineSpacing, setLineSpacing] = useState(1.8);
  const [wordSpacing, setWordSpacing] = useState(0);
  const [margins, setMargins] = useState(0);
  const [customFonts, setCustomFonts] = useState<CustomFontRecord[]>([]);
  const [fontBusy, setFontBusy] = useState(false);
  const [fontError, setFontError] = useState<string | null>(null);

  const fontOptions = [
    ...fonts.filter((font) => font.group === "system").map((font) => ({ value: font.id, label: font.label, group: t("settings.layout.fontGroupSystem") })),
    ...fonts.filter((font) => font.group === "built-in").map((font) => ({ value: font.id, label: font.label, group: t("settings.layout.fontGroupBuiltIn") })),
    ...fonts.filter((font) => font.group === "custom").map((font) => ({ value: font.id, label: font.label, group: t("settings.layout.fontGroupMine") })),
  ];

  useEffect(() => {
    if (loading) return;
    setReaderTheme((settings.reader_theme as ReaderTheme) || getDefaultReaderTheme());
    setCustomTheme(parseReaderCustomTheme(settings.reader_custom_theme));
    if (settings.font_family) setFontFamily(settings.font_family);
    if (settings.font_size) setFontSize(parseInt(settings.font_size));
    if (settings.line_spacing) setLineSpacing(parseFloat(settings.line_spacing));
    if (settings.word_spacing) setWordSpacing(parseInt(settings.word_spacing));
    if (settings.margins) setMargins(parseInt(settings.margins));
  }, [settings, loading]);

  const refreshCustomFonts = useCallback(async () => {
    const records = await loadCustomFonts();
    setCustomFonts(records);
    return records;
  }, []);

  const selectedCustomFontIsMissing = (records: CustomFontRecord[]) => (
    fontFamily.startsWith("custom-")
    && !records.some((record) => record.id === fontFamily)
  );

  useEffect(() => {
    if (loading) return;
    refreshCustomFonts().catch((error) => console.error("Failed to load custom fonts:", error));
  }, [loading, refreshCustomFonts]);

  return (
    <div>
      {/* Theme */}
      <div className="flex items-center justify-between min-h-[88px] py-2">
        <div>
          <p className="text-[14px] font-medium text-text-primary tracking-[-0.15px]">{t("settings.layout.theme")}</p>
          <p className="text-[12px] text-text-muted mt-0.5">{t("settings.layout.themeHint")}</p>
        </div>
        <div className="grid grid-cols-5 gap-2 shrink-0">
          {READER_THEME_OPTIONS.map((theme) => (
            <button
              key={theme.value}
              type="button"
              onClick={() => {
                setReaderTheme(theme.value);
                save("reader_theme", theme.value);
                showSavedToast();
              }}
              className="w-[48px] flex flex-col items-center gap-1.5 cursor-pointer"
            >
              <span
                className={`size-8 rounded-full ${theme.swatchClass} flex items-center justify-center ${
                  readerTheme === theme.value ? "ring-2 ring-accent ring-offset-2 ring-offset-bg-surface" : ""
                }`}
                style={theme.value === "custom" ? { backgroundColor: getCustomThemeStyles(customTheme).body } : undefined}
              >
                {readerTheme === theme.value && <Check size={14} className={theme.checkClass} />}
              </span>
              <span className="text-[10px] font-medium text-text-muted leading-none">{t(theme.labelKey)}</span>
            </button>
          ))}
        </div>
      </div>
      {readerTheme === "custom" && (
        <div className="border-b border-border-light pb-4">
          <ColorControl
            color={customTheme.color}
            opacity={customTheme.opacity}
            minOpacity={0}
            presets={CUSTOM_THEME_PRESETS}
            colorLabel={t("settings.layout.customThemeColor")}
            pickerLabel={t("settings.layout.customThemePicker")}
            hexLabel={t("settings.layout.customThemeHex")}
            opacityLabel={t("settings.layout.customThemeOpacity")}
            onChange={(next) => {
              setCustomTheme(next);
              void saveBulk({
                reader_theme: "custom",
                reader_custom_theme: JSON.stringify(next),
              }).then(() => showSavedToast());
            }}
          />
        </div>
      )}
      {/* Font Family */}
      <div className="flex items-center justify-between h-[73px]">
        <div>
          <p className="text-[14px] font-medium text-text-primary tracking-[-0.15px]">{t("settings.layout.fontFamily")}</p>
          <p className="text-[12px] text-text-muted mt-0.5">{t("settings.layout.fontFamilyHint")}</p>
        </div>
        <Select
          className={ROW_CONTROL_WIDTH}
          value={fontFamily}
          onChange={(v) => { setFontFamily(v); save("font_family", v); showSavedToast(); }}
          options={fontOptions}
        />
      </div>
      <div className="border-t border-border-light py-3">
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-[13px] font-medium text-text-primary">{t("settings.layout.customFonts")}</p>
            <p className="mt-0.5 text-[11px] leading-[17px] text-text-muted">{t("settings.layout.customFontsHint")}</p>
          </div>
          <button
            type="button"
            disabled={fontBusy}
            onClick={async () => {
              setFontBusy(true);
              setFontError(null);
              try {
                await invoke<CustomFontRecord[]>("import_custom_fonts");
                await refreshCustomFonts();
                showSavedToast();
              } catch (error) {
                console.error("Failed to import fonts:", error);
                setFontError(t("settings.layout.fontImportFailed"));
              } finally {
                setFontBusy(false);
              }
            }}
            className="flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-border px-2.5 text-[11px] font-medium text-text-secondary hover:bg-bg-input disabled:opacity-50"
          >
            <Download size={13} />
            {t("settings.layout.importFonts")}
          </button>
        </div>
        {customFonts.length > 0 && (
          <div className="mt-3 space-y-1.5">
            {customFonts.map((font) => (
              <div key={font.id} className="flex min-h-9 items-center justify-between gap-3 rounded-md bg-bg-input px-3">
                <span className="min-w-0 truncate text-[12px] text-text-primary" style={{ fontFamily: customFontFamily(font.id) }}>
                  {font.family_name}
                </span>
                <button
                  type="button"
                  title={t("settings.layout.deleteFont")}
                  aria-label={t("settings.layout.deleteFont")}
                  disabled={fontBusy}
                  onClick={async () => {
                    setFontBusy(true);
                    setFontError(null);
                    try {
                      await invoke("delete_custom_font", { id: font.id });
                      const records = await refreshCustomFonts();
                      if (selectedCustomFontIsMissing(records)) setFontFamily("system");
                      await refresh();
                      await notifyReadingAssistanceSettingsChanged([
                        "font_family",
                        "marker_style_config",
                      ]);
                      showSavedToast();
                    } catch (error) {
                      console.error("Failed to delete font:", error);
                      // Re-read the backend after any failure. This also heals
                      // the UI when an older backend reports an error after
                      // already deleting its database row.
                      const records = await refreshCustomFonts().catch(() => null);
                      await refresh().catch(() => {});
                      if (records && selectedCustomFontIsMissing(records)) {
                        setFontFamily("system");
                        await notifyReadingAssistanceSettingsChanged([
                          "font_family",
                          "marker_style_config",
                        ]).catch(() => {});
                      }
                      setFontError(t("settings.layout.fontDeleteFailed"));
                    } finally {
                      setFontBusy(false);
                    }
                  }}
                  className="flex size-7 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-surface hover:text-danger-text disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <Trash2 size={13} />
                </button>
              </div>
            ))}
          </div>
        )}
        {fontError && (
          <p role="alert" className="mt-2 text-[11px] leading-4 text-danger-text">
            {fontError}
          </p>
        )}
      </div>
      {/* Font Size */}
      <div className="flex items-center justify-between h-[73px]">
        <div>
          <p className="text-[14px] font-medium text-text-primary tracking-[-0.15px]">{t("settings.layout.fontSize")}</p>
          <p className="text-[12px] text-text-muted mt-0.5">{t("settings.layout.fontSizeHint")}</p>
        </div>
        <NumberInput value={fontSize} onChange={setFontSize} onBlur={() => save("font_size", String(fontSize))} suffix="px" min={FONT_SIZE_MIN} max={FONT_SIZE_MAX} />
      </div>
      {/* Line Spacing */}
      <div className="flex items-center justify-between h-[73px]">
        <div>
          <p className="text-[14px] font-medium text-text-primary tracking-[-0.15px]">{t("settings.layout.lineSpacing")}</p>
          <p className="text-[12px] text-text-muted mt-0.5">{t("settings.layout.lineSpacingHint")}</p>
        </div>
        <NumberInput value={lineSpacing} onChange={setLineSpacing} onBlur={() => save("line_spacing", String(lineSpacing))} suffix="x" min={1} max={3} />
      </div>
      {/* Word Spacing */}
      <div className="flex items-center justify-between h-[73px]">
        <div>
          <p className="text-[14px] font-medium text-text-primary tracking-[-0.15px]">{t("settings.layout.wordSpacing")}</p>
          <p className="text-[12px] text-text-muted mt-0.5">{t("settings.layout.wordSpacingHint")}</p>
        </div>
        <NumberInput value={wordSpacing} onChange={setWordSpacing} onBlur={() => save("word_spacing", String(wordSpacing))} suffix="px" min={-4} max={16} />
      </div>
      {/* Margins */}
      <div className="flex items-center justify-between h-[73px]">
        <div>
          <p className="text-[14px] font-medium text-text-primary tracking-[-0.15px]">{t("settings.layout.margins")}</p>
          <p className="text-[12px] text-text-muted mt-0.5">{t("settings.layout.marginsHint")}</p>
        </div>
        <NumberInput value={margins} onChange={setMargins} onBlur={() => save("margins", String(margins))} suffix="%" min={0} max={30} />
      </div>
    </div>
  );
}
