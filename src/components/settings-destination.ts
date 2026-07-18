export type SettingsSection =
  | "general"
  | "appearance"
  | "reading"
  | "ai"
  | "tools"
  | "librarySync"
  | "mcp"
  | "about";

export type SettingsView = "ocr";

export type SettingsDestination =
  | SettingsSection
  | { section: SettingsSection; view: SettingsView };

const SECTIONS = new Set<SettingsSection>([
  "general",
  "appearance",
  "reading",
  "ai",
  "tools",
  "librarySync",
  "mcp",
  "about",
]);

export function normalizeSettingsDestination(value: unknown): SettingsDestination {
  if (value === "lookup" || value === "translation") return "tools";
  if (typeof value === "string" && SECTIONS.has(value as SettingsSection)) {
    return value as SettingsSection;
  }
  if (value && typeof value === "object") {
    const candidate = value as { section?: unknown; view?: unknown };
    if (candidate.section === "tools" && candidate.view === "ocr") {
      return { section: "tools", view: "ocr" };
    }
    if (typeof candidate.section === "string" && SECTIONS.has(candidate.section as SettingsSection)) {
      return candidate.section as SettingsSection;
    }
  }
  return "general";
}

export function settingsDestinationSection(destination: SettingsDestination): SettingsSection {
  return typeof destination === "string" ? destination : destination.section;
}

export function settingsDestinationView(destination: SettingsDestination): SettingsView | undefined {
  return typeof destination === "string" ? undefined : destination.view;
}
