import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight } from "lucide-react";
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
import ColorControl from "../ui/ColorControl";
import { ROW_CONTROL_WIDTH } from "./types";
import WordFormsManager from "./WordFormsManager";

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
  title,
  value,
  onChange,
}: {
  title: string;
  value: MarkerVisualStyleV1;
  onChange: (value: MarkerVisualStyleV1) => void;
}) {
  const { t } = useTranslation();
  const update = <K extends keyof MarkerVisualStyleV1>(key: K, next: MarkerVisualStyleV1[K]) => {
    const candidate = { ...value, [key]: next };
    if (!candidate.background && !candidate.underline && !candidate.bold) return;
    onChange(candidate);
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
        <ColorControl
          color={value.color}
          opacity={value.opacity}
          presets={MARKER_COLOR_PRESETS}
          colorLabel={t("settings.tools.markers.color")}
          pickerLabel={t("settings.tools.markers.colorPicker")}
          hexLabel={t("settings.tools.markers.hexColor")}
          opacityLabel={t("settings.tools.markers.opacity")}
          onChange={(next) => onChange({ ...value, ...next })}
        />

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
  const [wordFormsOpen, setWordFormsOpen] = useState(true);

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
        <div className="flex min-w-0 items-start gap-1.5">
          {value.wordMatchScope === "forms" && (
            <button
              type="button"
              aria-expanded={wordFormsOpen}
              onClick={() => setWordFormsOpen((open) => !open)}
              className="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"
            >
              {wordFormsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            </button>
          )}
          <div>
          <p className="text-[13px] font-medium text-text-primary">{t("settings.tools.markers.wordScope")}</p>
          <p className="text-[11px] leading-[17px] text-text-muted">{t("settings.tools.markers.wordScopeHint")}</p>
          </div>
        </div>
        <Select
          className={ROW_CONTROL_WIDTH}
          value={value.wordMatchScope}
          onChange={(scope) => {
            if (scope === "forms") setWordFormsOpen(true);
            onChange({ ...value, wordMatchScope: scope as MarkerStyleConfigV1["wordMatchScope"] });
          }}
          options={[
            { value: "current", label: t("settings.tools.markers.currentOnly") },
            { value: "book", label: t("settings.tools.markers.sameWordsInBook") },
            { value: "forms", label: t("settings.tools.markers.sameWordForms") },
          ]}
        />
      </div>

      {value.wordMatchScope === "forms" && wordFormsOpen && <WordFormsManager />}

      <StyleEditor title={t("settings.tools.markers.manualStyle")} value={value.manual} onChange={(manual) => onChange({ ...value, manual })} />

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
        <StyleEditor title={t("settings.tools.markers.automaticStyle")} value={value.automatic} onChange={(automatic) => onChange({ ...value, automatic })} />
      )}

      {customFonts.length === 0 && (
        <p className="border-t border-border-light py-3 text-[10px] leading-4 text-text-muted">
          {t("settings.tools.markers.customFontHint")}
        </p>
      )}
    </div>
  );
}
