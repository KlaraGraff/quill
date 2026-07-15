import { getFontFamily } from "./reader-settings";

export const MARKER_STYLE_SETTING_KEY = "marker_style_config";

export type MarkerFontChoice = "inherit" | "reader" | string;

export interface MarkerVisualStyleV1 {
  color: string;
  opacity: number;
  background: boolean;
  underline: boolean;
  bold: boolean;
  font: MarkerFontChoice;
}

export interface MarkerStyleConfigV1 {
  version: 1;
  wordMatchScope: "current" | "book" | "forms";
  manual: MarkerVisualStyleV1;
  automaticFollowsManual: boolean;
  automatic: MarkerVisualStyleV1;
}

export const MARKER_COLOR_PRESETS = [
  "#E9B949",
  "#4FAE91",
  "#5B8FD9",
  "#CF6F8A",
  "#8A8F98",
] as const;

const DEFAULT_MANUAL: MarkerVisualStyleV1 = {
  color: "#E9B949",
  opacity: 32,
  background: true,
  underline: false,
  bold: false,
  font: "inherit",
};

const DEFAULT_AUTOMATIC: MarkerVisualStyleV1 = {
  color: "#8D7C65",
  opacity: 18,
  background: true,
  underline: true,
  bold: false,
  font: "inherit",
};

export function createDefaultMarkerStyleConfig(): MarkerStyleConfigV1 {
  return {
    version: 1,
    wordMatchScope: "book",
    manual: { ...DEFAULT_MANUAL },
    automaticFollowsManual: true,
    automatic: { ...DEFAULT_AUTOMATIC },
  };
}

function normalizeColor(value: unknown, fallback: string) {
  return typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value) ? value.toUpperCase() : fallback;
}

function normalizeVisualStyle(value: unknown, fallback: MarkerVisualStyleV1): MarkerVisualStyleV1 {
  const source = value && typeof value === "object" ? value as Partial<MarkerVisualStyleV1> : {};
  const background = source.background ?? fallback.background;
  const underline = source.underline ?? fallback.underline;
  const bold = source.bold ?? fallback.bold;
  // At least one treatment must remain active so a saved marker cannot become
  // invisible. Font choice is deliberately not counted as a marker treatment.
  const hasTreatment = background || underline || bold;
  return {
    color: normalizeColor(source.color, fallback.color),
    opacity: Math.min(100, Math.max(5, Number.isFinite(source.opacity) ? Number(source.opacity) : fallback.opacity)),
    background: hasTreatment ? background : true,
    underline,
    bold,
    font: typeof source.font === "string" && source.font.trim() ? source.font : fallback.font,
  };
}

export function parseMarkerStyleConfig(value: unknown): MarkerStyleConfigV1 {
  let source: unknown = value;
  if (typeof value === "string") {
    try {
      source = JSON.parse(value);
    } catch {
      source = null;
    }
  }
  const parsed = source && typeof source === "object" ? source as Partial<MarkerStyleConfigV1> & { markMatchingWords?: boolean } : {};
  const defaults = createDefaultMarkerStyleConfig();
  return {
    version: 1,
    wordMatchScope: parsed.wordMatchScope === "current" || parsed.wordMatchScope === "book" || parsed.wordMatchScope === "forms"
      ? parsed.wordMatchScope
      : parsed.markMatchingWords === false ? "current" : "book",
    manual: normalizeVisualStyle(parsed.manual, defaults.manual),
    automaticFollowsManual: parsed.automaticFollowsManual ?? defaults.automaticFollowsManual,
    automatic: normalizeVisualStyle(parsed.automatic, defaults.automatic),
  };
}

export function serializeMarkerStyleConfig(config: MarkerStyleConfigV1) {
  return JSON.stringify(parseMarkerStyleConfig(config));
}

export function effectiveAutomaticMarkerStyle(config: MarkerStyleConfigV1) {
  return config.automaticFollowsManual ? config.manual : config.automatic;
}

export function markerFontFamily(font: MarkerFontChoice, readerFont?: string) {
  if (font === "inherit") return undefined;
  if (font === "reader") return readerFont;
  return getFontFamily(font);
}

export function markerStyleCss(style: MarkerVisualStyleV1, fontFamily?: string) {
  const alpha = Math.round((style.opacity / 100) * 255).toString(16).padStart(2, "0");
  return {
    backgroundColor: style.background ? `${style.color}${alpha}` : "transparent",
    textDecoration: style.underline ? "underline" : "none",
    textDecorationColor: style.color,
    textDecorationThickness: style.underline ? "1.5px" : undefined,
    textUnderlineOffset: style.underline ? "0.14em" : undefined,
    fontWeight: style.bold ? 700 : undefined,
    fontFamily: fontFamily || undefined,
  } as const;
}

export function markerHighlightCss(style: MarkerVisualStyleV1, fontFamily?: string) {
  const alpha = Math.round((style.opacity / 100) * 255).toString(16).padStart(2, "0");
  return [
    style.background ? `background-color: ${style.color}${alpha};` : "background-color: transparent;",
    style.underline
      ? `text-decoration: underline; text-decoration-color: ${style.color}; text-decoration-thickness: 1.5px; text-underline-offset: 0.14em;`
      : "text-decoration: none;",
    style.bold ? "font-weight: 700;" : "",
    fontFamily ? `font-family: ${fontFamily};` : "",
  ].filter(Boolean).join(" ");
}

// Foliate renders stored CFI annotations in an SVG overlay. Background and
// underline are reliable there; font and weight are only supported by the
// direct DOM text reader and CSS Highlight-based whole-word markers.
export function markerOverlayStyle(style: MarkerVisualStyleV1): MarkerVisualStyleV1 {
  return { ...style, bold: false, font: "inherit" };
}
