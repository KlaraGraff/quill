import type { CSSProperties } from "react";
import type { ReaderSettingsState } from "../../components/ReaderSettings";
import {
  getEffectivePageColumns,
  getFontFamily,
  getThemeStyles,
} from "../../components/reader-settings";
import { prefersReducedMotion } from "../../components/page-turn-transition";

interface LayoutView {
  renderer: {
    setAttribute(name: string, value: string): void;
    getAttribute(name: string): string | null;
    toggleAttribute(name: string, force?: boolean): boolean;
  };
}

export type PdfOverlay = { layers: CSSProperties[] } | null;

function readerSelectionColor(theme: string): string {
  switch (theme) {
    case "paper": return "rgba(163, 106, 49, 0.28)";
    case "quiet": return "rgba(216, 180, 254, 0.34)";
    case "dark": return "rgba(167, 139, 250, 0.38)";
    default: return "rgba(124, 58, 237, 0.24)";
  }
}

export function getPdfOverlays(theme: string): PdfOverlay {
  switch (theme) {
    case "paper": return { layers: [{
      backgroundColor: getThemeStyles("paper").body,
      mixBlendMode: "multiply",
    }] };
    case "quiet": return { layers: [
      { backgroundColor: "#ffffff", mixBlendMode: "difference" },
      { backgroundColor: getThemeStyles("quiet").body, mixBlendMode: "screen" },
    ] };
    case "dark": return { layers: [
      { backgroundColor: "#ffffff", mixBlendMode: "difference" },
      { backgroundColor: getThemeStyles("dark").body, mixBlendMode: "screen" },
    ] };
    default: return null;
  }
}

export function getReaderThemeVars(theme: string): Record<string, string> | undefined {
  switch (theme) {
    case "original": return {
      "--color-bg-page": "#f4f4f5",
      "--color-bg-surface": "#ffffff",
      "--color-bg-muted": "#fafafa",
      "--color-bg-input": "#f3f3f5",
      "--color-text-primary": "#18181b",
      "--color-text-body": "#0a0a0a",
      "--color-text-secondary": "#52525c",
      "--color-text-muted": "#71717b",
      "--color-text-placeholder": "#a1a1aa",
      "--color-border": "#e4e4e7",
      "--color-border-light": "#f4f4f5",
      "--color-accent": "#7c3aed",
      "--color-accent-text": "#7c3aed",
      "--color-accent-bg": "#f3e8ff",
    };
    case "paper": return {
      "--color-bg-page": "#F4F0E7",
      "--color-bg-surface": "#FAF7F0",
      "--color-bg-muted": "#F7F3EB",
      "--color-bg-input": "#EFE9DD",
      "--color-text-primary": "#29251E",
      "--color-text-body": "#29251E",
      "--color-text-secondary": "#5F584D",
      "--color-text-muted": "#827969",
      "--color-text-placeholder": "#9A907F",
      "--color-border": "#DDD5C8",
      "--color-border-light": "#EEE8DD",
      "--color-accent": "#A36A31",
      "--color-accent-text": "#8A5728",
      "--color-accent-bg": "#F0E3D1",
    };
    case "quiet": return {
      "--color-bg-page": "#5A5A63",
      "--color-bg-surface": "#71717b",
      "--color-bg-muted": "#68686F",
      "--color-bg-input": "#5A5A63",
      "--color-text-primary": "#fafafa",
      "--color-text-body": "#fafafa",
      "--color-text-secondary": "#d4d4d8",
      "--color-text-muted": "#d4d4d8",
      "--color-text-placeholder": "#a1a1aa",
      "--color-border": "#9999a1",
      "--color-border-light": "#5A5A63",
      "--color-accent": "#D8B4FE",
      "--color-accent-text": "#F3E8FF",
      "--color-accent-bg": "#5A4D6E",
    };
    case "dark": return {
      "--color-bg-page": "#151518",
      "--color-bg-surface": "#18191d",
      "--color-bg-muted": "#1f2023",
      "--color-bg-input": "#25262c",
      "--color-text-primary": "#f4f4f5",
      "--color-text-body": "#e7e7ea",
      "--color-text-secondary": "#c9c9d1",
      "--color-text-muted": "#9a9aa4",
      "--color-text-placeholder": "#85858f",
      "--color-border": "#34343d",
      "--color-border-light": "#2a2b31",
      "--color-accent": "#8B5CF6",
      "--color-accent-text": "#A78BFA",
      "--color-accent-bg": "#302647",
    };
    default: return undefined;
  }
}

export function getReaderCSS(settings: ReaderSettingsState): string {
  const themeColors = getThemeStyles(settings.theme);
  const fontFamily = getFontFamily(settings.font);
  const letterSpacing = settings.charSpacing === 0 ? "normal" : `${settings.charSpacing * 0.01}em`;
  const wordSpacing = settings.wordSpacing === 0 ? "normal" : `${settings.wordSpacing * 0.01}em`;
  const chapterBreakCss = settings.readingMode === "paginated" ? `
    [data-quill-chapter-start] {
      break-before: column !important;
      page-break-before: always !important;
    }
  ` : "";
  return `
    body {
      background-color: ${themeColors.body} !important;
      color: ${themeColors.text} !important;
      font-family: ${fontFamily} !important;
      font-size: ${settings.fontSize}px !important;
      line-height: ${settings.lineSpacing} !important;
      letter-spacing: ${letterSpacing} !important;
      word-spacing: ${wordSpacing} !important;
    }
    p, span, div, li, td, th, h1, h2, h3, h4, h5, h6 {
      color: ${themeColors.text} !important;
      font-family: ${fontFamily} !important;
      line-height: ${settings.lineSpacing} !important;
    }
    ::selection {
      background: ${readerSelectionColor(settings.theme)} !important;
      color: inherit !important;
    }
    ${chapterBreakCss}
    ::-webkit-scrollbar { width: 8px; height: 8px; }
    ::-webkit-scrollbar-track { background: transparent; }
    ::-webkit-scrollbar-thumb { background: ${themeColors.text}33; border-radius: 9999px; }
    ::-webkit-scrollbar-thumb:hover { background: ${themeColors.text}55; }
    img, svg, video {
      max-width: 100% !important;
      height: auto !important;
      object-fit: contain !important;
      box-sizing: border-box !important;
    }
    figure {
      max-width: 100% !important;
      overflow: hidden !important;
    }
  `;
}

export function applyReflowLayout(
  view: LayoutView,
  settings: ReaderSettingsState,
  viewportWidth: number,
  viewportHeight: number,
): void {
  const isPaginated = settings.readingMode === "paginated";
  const width = Math.max(1, viewportWidth);
  const effectiveColumns = getEffectivePageColumns(settings, width, viewportHeight);
  const columnWidth = width / effectiveColumns;
  view.renderer.setAttribute("flow", isPaginated ? "paginated" : "scrolled");
  view.renderer.setAttribute("gap", `${settings.margins}%`);
  view.renderer.setAttribute("max-column-count", String(effectiveColumns));
  view.renderer.setAttribute("max-inline-size", `${columnWidth}px`);
  // Reflowable books retain Foliate's native slide so direct trackpad gestures
  // animate too. The shared transition layer detects this paginator and does
  // not add a second animation; fixed-layout PDF uses the container fallback.
  view.renderer.toggleAttribute(
    "animated",
    isPaginated
      && settings.pageTurnAnimation === "slide"
      && !prefersReducedMotion(),
  );
}

export function applyPdfLayout(
  view: LayoutView,
  settings: ReaderSettingsState,
  viewportWidth: number,
  viewportHeight: number,
): number {
  const effectiveColumns = getEffectivePageColumns(settings, viewportWidth, viewportHeight);
  const columns = String(effectiveColumns);
  const spread = effectiveColumns === 2 ? "auto" : "none";
  if (view.renderer.getAttribute("max-column-count") !== columns) {
    view.renderer.setAttribute("max-column-count", columns);
  }
  if (view.renderer.getAttribute("spread") !== spread) {
    view.renderer.setAttribute("spread", spread);
  }
  return effectiveColumns;
}
