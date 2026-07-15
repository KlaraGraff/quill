import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  MARKER_COLOR_PRESETS,
  effectiveAutomaticMarkerStyle,
  markerStyleCss,
  type MarkerStyleConfigV1,
  type MarkerVisualStyleV1,
} from "../marker-style";
import { fonts } from "../reader-settings";
import { installCustomFontFaces, type CustomFontRecord } from "../custom-fonts";
import Select from "../ui/Select";
import Toggle from "../ui/Toggle";
import { ROW_CONTROL_WIDTH } from "./types";

interface MarkerStyleSettingsProps {
  value: MarkerStyleConfigV1;
  onChange: (value: MarkerStyleConfigV1) => void;
}

function fontFamilyForMarker(font: string) {
  if (font === "inherit" || font === "reader") return undefined;
  return fonts.find((item) => item.id === font)?.family;
}

function TreatmentToggle({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={`h-8 rounded-md border px-3 text-[11px] font-medium transition-colors ${
        active
          ? "border-accent bg-accent-bg text-accent-text"
          : "border-border bg-bg-surface text-text-secondary hover:bg-bg-input"
      }`}
    >
      {children}
    </button>
  );
}

function StyleEditor({
  id,
  title,
  value,
  onChange,
}: {
  id: "manual" | "automatic";
  title: string;
  value: MarkerVisualStyleV1;
  onChange: (value: MarkerVisualStyleV1) => void;
}) {
  const { t } = useTranslation();
  const [colorDraft, setColorDraft] = useState(value.color);
  useEffect(() => {
    setColorDraft(value.color);
  }, [value.color]);

  const update = <K extends keyof MarkerVisualStyleV1>(key: K, next: MarkerVisualStyleV1[K]) => {
    const candidate = { ...value, [key]: next };
    if (!candidate.background && !candidate.underline && !candidate.bold) return;
    onChange(candidate);
  };
  const commitColorDraft = () => {
    const trimmed = colorDraft.trim();
    const normalized = (trimmed.startsWith("#") ? trimmed : `#${trimmed}`).toUpperCase();
    if (/^#[0-9A-F]{6}$/.test(normalized)) {
      setColorDraft(normalized);
      update("color", normalized);
    } else {
      setColorDraft(value.color);
    }
  };
  const fontOptions = [
    { value: "inherit", label: t("settings.tools.markers.followOriginal") },
    { value: "reader", label: t("settings.tools.markers.followReaderFont") },
    ...fonts.map((font) => ({ value: font.id, label: font.label })),
  ];

  return (
    <section className="border-t border-border-light py-4 first:border-t-0 first:pt-0">
      <h4 className="mb-3 text-[12px] font-semibold text-text-primary">{title}</h4>
      <div className="space-y-3">
        <div>
          <p className="mb-2 text-[11px] text-text-muted">{t("settings.tools.markers.color")}</p>
          <div className="flex flex-wrap items-center gap-2">
            {MARKER_COLOR_PRESETS.map((color) => (
              <button
                key={color}
                type="button"
                aria-label={color}
                title={color}
                onClick={() => update("color", color)}
                className={`flex size-7 items-center justify-center rounded-full border border-black/10 ${value.color === color ? "ring-2 ring-accent ring-offset-2 ring-offset-bg-surface" : ""}`}
                style={{ backgroundColor: color }}
              >
                {value.color === color && <Check size={13} className="text-white drop-shadow" />}
              </button>
            ))}
            <label className="relative size-7 shrink-0 overflow-hidden rounded-full border border-border" title={t("settings.tools.markers.colorPicker")}>
              <input
                type="color"
                value={value.color}
                onChange={(event) => update("color", event.target.value.toUpperCase())}
                className="absolute -inset-2 size-12 cursor-pointer border-0 bg-transparent p-0"
              />
            </label>
            <input
              value={colorDraft}
              maxLength={7}
              aria-label={t("settings.tools.markers.hexColor")}
              onChange={(event) => setColorDraft(event.target.value.toUpperCase())}
              onBlur={commitColorDraft}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                commitColorDraft();
              }}
              className="h-8 w-[88px] rounded-md border border-border bg-bg-input px-2 font-mono text-[11px] uppercase text-text-primary outline-none focus:border-accent"
            />
          </div>
        </div>

        <div className="flex items-center gap-3">
          <label className="w-[72px] shrink-0 text-[11px] text-text-muted" htmlFor={`${id}-marker-opacity`}>
            {t("settings.tools.markers.opacity")}
          </label>
          <input
            id={`${id}-marker-opacity`}
            type="range"
            min={5}
            max={100}
            step={1}
            value={value.opacity}
            onChange={(event) => update("opacity", Number(event.target.value))}
            className="h-1 flex-1 cursor-pointer accent-accent"
          />
          <span className="w-10 text-right text-[11px] tabular-nums text-text-secondary">{value.opacity}%</span>
        </div>

        <div>
          <p className="mb-2 text-[11px] text-text-muted">{t("settings.tools.markers.treatments")}</p>
          <div className="flex flex-wrap gap-2">
            <TreatmentToggle active={value.background} onClick={() => update("background", !value.background)}>
              {t("settings.tools.markers.background")}
            </TreatmentToggle>
            <TreatmentToggle active={value.underline} onClick={() => update("underline", !value.underline)}>
              {t("settings.tools.markers.underline")}
            </TreatmentToggle>
            <TreatmentToggle active={value.bold} onClick={() => update("bold", !value.bold)}>
              {t("settings.tools.markers.bold")}
            </TreatmentToggle>
          </div>
        </div>

        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-[11px] font-medium text-text-primary">{t("settings.tools.markers.font")}</p>
            <p className="text-[10px] leading-4 text-text-muted">{t("settings.tools.markers.fontHint")}</p>
          </div>
          <Select className={ROW_CONTROL_WIDTH} value={value.font} onChange={(font) => update("font", font)} options={fontOptions} />
        </div>
      </div>
    </section>
  );
}

export default function MarkerStyleSettings({ value, onChange }: MarkerStyleSettingsProps) {
  const { t } = useTranslation();
  const [customFonts, setCustomFonts] = useState<CustomFontRecord[]>([]);

  useEffect(() => {
    invoke<CustomFontRecord[]>("list_custom_fonts").then((records) => {
      setCustomFonts(records);
      installCustomFontFaces(records);
    }).catch(() => {});
  }, []);

  const manualCss = markerStyleCss(value.manual, fontFamilyForMarker(value.manual.font));
  const automatic = effectiveAutomaticMarkerStyle(value);
  const automaticCss = markerStyleCss(automatic, fontFamilyForMarker(automatic.font));

  return (
    <div className="mx-auto w-full max-w-[620px]">
      <div className="mb-4 grid grid-cols-2 gap-2 rounded-md border border-border-light p-3">
        {["light", "dark"].map((theme) => (
          <div key={theme} className={`min-w-0 rounded-md px-3 py-3 text-[13px] leading-6 ${theme === "dark" ? "bg-[#1B1B1F] text-[#E7E7EA]" : "bg-[#FAF7F0] text-[#29251E]"}`}>
            <span>{t("settings.tools.markers.previewBefore")} </span>
            <span style={manualCss}>{t("settings.tools.markers.previewManual")}</span>
            <span> {t("settings.tools.markers.previewMiddle")} </span>
            <span style={automaticCss}>{t("settings.tools.markers.previewAutomatic")}</span>
            <span> {t("settings.tools.markers.previewAfter")}</span>
          </div>
        ))}
      </div>

      <div className="mb-4 flex min-h-[52px] items-center justify-between gap-4 border-b border-border-light pb-3">
        <div>
          <p className="text-[13px] font-medium text-text-primary">{t("settings.tools.markers.wordScope")}</p>
          <p className="text-[11px] leading-[17px] text-text-muted">{t("settings.tools.markers.wordScopeHint")}</p>
        </div>
        <Select
          className={ROW_CONTROL_WIDTH}
          value={value.markMatchingWords ? "book" : "current"}
          onChange={(scope) => onChange({ ...value, markMatchingWords: scope === "book" })}
          options={[
            { value: "current", label: t("settings.tools.markers.currentOnly") },
            { value: "book", label: t("settings.tools.markers.sameWordsInBook") },
          ]}
        />
      </div>

      <StyleEditor id="manual" title={t("settings.tools.markers.manualStyle")} value={value.manual} onChange={(manual) => onChange({ ...value, manual })} />

      <div className="flex min-h-[52px] items-center justify-between gap-4 border-t border-border-light py-3">
        <div>
          <p className="text-[13px] font-medium text-text-primary">{t("settings.tools.markers.automaticFollowsManual")}</p>
          <p className="text-[11px] leading-[17px] text-text-muted">{t("settings.tools.markers.automaticFollowsManualHint")}</p>
        </div>
        <Toggle
          label={t("settings.tools.markers.automaticFollowsManual")}
          checked={value.automaticFollowsManual}
          onChange={(automaticFollowsManual) => onChange({ ...value, automaticFollowsManual })}
        />
      </div>

      {!value.automaticFollowsManual && (
        <StyleEditor id="automatic" title={t("settings.tools.markers.automaticStyle")} value={value.automatic} onChange={(automatic) => onChange({ ...value, automatic })} />
      )}

      {customFonts.length === 0 && (
        <p className="border-t border-border-light py-3 text-[10px] leading-4 text-text-muted">
          {t("settings.tools.markers.customFontHint")}
        </p>
      )}
    </div>
  );
}
