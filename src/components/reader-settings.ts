import type { PageColumns, ReaderSettingsState } from "./ReaderSettings";

export const FONT_SIZE_MIN = 12;
export const FONT_SIZE_MAX = 48;

export interface ReaderFontOption {
  id: string;
  label: string;
  family: string;
  group: "system" | "built-in" | "custom";
  filePath?: string;
}

export const fonts: ReaderFontOption[] = [
  { id: "system", label: "System", family: "system-ui, -apple-system, 'PingFang SC', sans-serif", group: "system" },
  { id: "georgia", label: "Georgia", family: "Georgia, serif", group: "system" },
  { id: "palatino", label: "Palatino", family: "Palatino, serif", group: "system" },
  { id: "times", label: "Times New Roman", family: "'Times New Roman', serif", group: "system" },
  { id: "inter", label: "Inter", family: "Inter, sans-serif", group: "built-in" },
];

const themes = [
  { id: "original", label: "Original", color: "bg-reader-original-bg border border-reader-original-border", pdf: true },
  { id: "paper", label: "Reading Paper", color: "bg-reader-paper-bg border border-reader-original-border", pdf: true },
  { id: "quiet", label: "Gray", color: "bg-reader-quiet-bg", pdf: true },
  { id: "dark", label: "Dark", color: "bg-reader-dark-bg border border-reader-dark-border", pdf: true },
  { id: "custom", label: "Custom", color: "border border-reader-original-border", pdf: true },
] as const;

export type ReaderTheme = (typeof themes)[number]["id"];
export interface ReaderCustomTheme { color: string; opacity: number }
export type ReaderFont = string;

export interface ReaderCapabilities {
  // Selection and stored annotation support are deliberately separate from
  // automatic word markers. PDF text layers can support the former without
  // making a promise that we can reliably mark every vocabulary occurrence.
  supportsSelection: boolean;
  supportsManualAnnotations: boolean;
  supportsWordMarkers: boolean;
  supportsCfiNavigation: boolean;
  supportsReflowSettings: boolean;
  supportsSpread: boolean;
  supportsContinuousScroll: boolean;
  supportsZoom: boolean;
}

export function getEffectivePageColumns(
  settings: Pick<ReaderSettingsState, "readingMode" | "pageColumns">,
  viewportWidth: number,
  viewportHeight: number,
): PageColumns {
  if (settings.readingMode !== "paginated" || settings.pageColumns !== 2) return 1;
  return Math.max(1, viewportWidth) > Math.max(1, viewportHeight) ? 2 : 1;
}

export function getReaderCapabilities(format?: string): ReaderCapabilities {
  switch ((format || "epub").toLowerCase()) {
    case "epub":
      return {
        supportsSelection: true,
        supportsManualAnnotations: true,
        supportsWordMarkers: true,
        supportsCfiNavigation: true,
        supportsReflowSettings: true,
        supportsSpread: true,
        supportsContinuousScroll: true,
        supportsZoom: false,
      };
    case "text":
      return {
        supportsSelection: true,
        supportsManualAnnotations: true,
        supportsWordMarkers: true,
        supportsCfiNavigation: true,
        supportsReflowSettings: true,
        supportsSpread: true,
        supportsContinuousScroll: true,
        supportsZoom: false,
      };
    case "pdf":
      return {
        supportsSelection: true,
        supportsManualAnnotations: true,
        supportsWordMarkers: false,
        supportsCfiNavigation: true,
        supportsReflowSettings: false,
        supportsSpread: true,
        supportsContinuousScroll: true,
        supportsZoom: true,
      };
    case "mobi":
    case "azw":
    case "azw3":
    case "fb2":
    case "fbz":
      return {
        supportsSelection: false,
        supportsManualAnnotations: false,
        supportsWordMarkers: false,
        supportsCfiNavigation: false,
        supportsReflowSettings: true,
        supportsSpread: true,
        supportsContinuousScroll: true,
        supportsZoom: false,
      };
    case "cbz":
      return {
        supportsSelection: false,
        supportsManualAnnotations: false,
        supportsWordMarkers: false,
        supportsCfiNavigation: false,
        supportsReflowSettings: false,
        supportsSpread: false,
        supportsContinuousScroll: false,
        supportsZoom: false,
      };
    default:
      return {
        supportsSelection: false,
        supportsManualAnnotations: false,
        supportsWordMarkers: false,
        supportsCfiNavigation: false,
        supportsReflowSettings: false,
        supportsSpread: false,
        supportsContinuousScroll: false,
        supportsZoom: false,
      };
  }
}

export function getReaderThemes() {
  return themes;
}

export function getFontFamily(fontId: ReaderFont): string {
  if (fontId.startsWith("custom-")) return `${customFontFamily(fontId)}, serif`;
  return fonts.find((font) => font.id === fontId)?.family ?? "Inter, system-ui, sans-serif";
}

export function isReaderFontAvailable(fontId: ReaderFont): boolean {
  return fonts.some((font) => font.id === fontId);
}

export function customFontFamily(id: string) {
  return `"QuillCustom-${id.replace(/[^a-zA-Z0-9_-]/g, "")}"`;
}

export function setCustomReaderFonts(customFonts: Array<{ id: string; family_name: string; file_path: string }>) {
  const next = fonts.filter((font) => font.group !== "custom");
  for (const font of customFonts) {
    next.push({
      id: font.id,
      label: font.family_name,
      family: `${customFontFamily(font.id)}, serif`,
      group: "custom",
      filePath: font.file_path,
    });
  }
  fonts.splice(0, fonts.length, ...next);
}

export const DEFAULT_READER_CUSTOM_THEME: ReaderCustomTheme = { color: "#DDE8D8", opacity: 70 };

export function parseReaderCustomTheme(value: unknown): ReaderCustomTheme {
  let source = value;
  if (typeof source === "string") {
    try { source = JSON.parse(source); } catch { source = null; }
  }
  const record = source && typeof source === "object" ? source as Partial<ReaderCustomTheme> : {};
  return {
    color: typeof record.color === "string" && /^#[0-9a-f]{6}$/i.test(record.color)
      ? record.color.toUpperCase()
      : DEFAULT_READER_CUSTOM_THEME.color,
    opacity: Number.isFinite(record.opacity)
      ? Math.min(100, Math.max(0, Number(record.opacity)))
      : DEFAULT_READER_CUSTOM_THEME.opacity,
  };
}

function rgb(color: string) {
  return [1, 3, 5].map((start) => parseInt(color.slice(start, start + 2), 16));
}

function hex(channels: number[]) {
  return `#${channels.map((channel) => Math.round(channel).toString(16).padStart(2, "0")).join("")}`.toUpperCase();
}

function luminance(color: string) {
  const channels = rgb(color).map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
}

function contrast(left: string, right: string) {
  const values = [luminance(left), luminance(right)].sort((a, b) => b - a);
  return (values[0] + 0.05) / (values[1] + 0.05);
}

function mix(foreground: string, background: string, opacity: number) {
  const fg = rgb(foreground);
  const bg = rgb(background);
  const alpha = opacity / 100;
  return hex(fg.map((channel, index) => channel * alpha + bg[index] * (1 - alpha)));
}

export function getCustomThemeStyles(customTheme: ReaderCustomTheme) {
  const normalized = parseReaderCustomTheme(customTheme);
  const body = mix(normalized.color, "#FFFFFF", normalized.opacity);
  const lightBackground = luminance(body) >= 0.42;
  let text = lightBackground ? "#2A2620" : "#E7E7EA";
  const target = lightBackground ? "#000000" : "#FFFFFF";
  for (let step = 1; contrast(body, text) < 4.5 && step <= 10; step += 1) {
    text = mix(target, text, step * 10);
  }
  return { body, text };
}

export function getThemeStyles(themeId: ReaderTheme, customTheme = DEFAULT_READER_CUSTOM_THEME) {
  switch (themeId) {
    case "paper":
      return { body: "#FAF7F0", text: "#29251E" };
    case "quiet":
      return { body: "#71717b", text: "#fafafa" };
    case "dark":
      return { body: "#1b1b1f", text: "#d8d8de" };
    case "custom":
      return getCustomThemeStyles(customTheme);
    default:
      return { body: "#ffffff", text: "#0a0a0a" };
  }
}

export function getDefaultReaderTheme(): ReaderTheme {
  return document.documentElement.classList.contains("dark") ? "dark" : "paper";
}
