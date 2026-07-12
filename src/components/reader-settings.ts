export const FONT_SIZE_MIN = 12;
export const FONT_SIZE_MAX = 48;

export const fonts = [
  { id: "system", label: "System", family: "system-ui, -apple-system, 'PingFang SC', sans-serif" },
  { id: "georgia", label: "Georgia", family: "Georgia, serif" },
  { id: "palatino", label: "Palatino", family: "Palatino, serif" },
  { id: "inter", label: "Inter", family: "Inter, sans-serif" },
  { id: "times", label: "Times New Roman", family: "'Times New Roman', serif" },
] as const;

const themes = [
  { id: "original", label: "Original", color: "bg-reader-original-bg border border-reader-original-border", pdf: true },
  { id: "paper", label: "Reading Paper", color: "bg-reader-paper-bg border border-reader-original-border", pdf: true },
  { id: "quiet", label: "Gray", color: "bg-reader-quiet-bg", pdf: true },
  { id: "dark", label: "Dark", color: "bg-reader-dark-bg border border-reader-dark-border", pdf: true },
] as const;

export type ReaderTheme = (typeof themes)[number]["id"];
export type ReaderFont = (typeof fonts)[number]["id"];

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

export function getReaderCapabilities(format?: string): ReaderCapabilities {
  switch ((format || "epub").toLowerCase()) {
    case "epub":
      return {
        supportsSelection: true,
        supportsManualAnnotations: true,
        supportsWordMarkers: true,
        supportsCfiNavigation: true,
        supportsReflowSettings: true,
        supportsSpread: false,
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
        supportsSpread: false,
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
  return fonts.find((font) => font.id === fontId)?.family ?? "Inter, system-ui, sans-serif";
}

export function getThemeStyles(themeId: ReaderTheme) {
  switch (themeId) {
    case "paper":
      return { body: "#FAF7F0", text: "#29251E" };
    case "quiet":
      return { body: "#71717b", text: "#fafafa" };
    case "dark":
      return { body: "#1b1b1f", text: "#d8d8de" };
    default:
      return { body: "#ffffff", text: "#0a0a0a" };
  }
}

export function getDefaultReaderTheme(): ReaderTheme {
  return document.documentElement.classList.contains("dark") ? "dark" : "paper";
}
