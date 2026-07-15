import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";

export const READING_ASSISTANCE_SETTINGS_CHANGED = "reading-assistance-settings-changed";

export interface ReadingAssistanceSettingsChangedPayload {
  keys: string[];
}

export const READING_ASSISTANCE_SETTING_KEYS = [
  "double_click_quick_lookup",
  "auto_highlight_lookup_words",
  "marker_style_config",
  "learning_card_config",
] as const;

export async function notifyReadingAssistanceSettingsChanged(keys: string[]) {
  const uniqueKeys = [...new Set(keys.filter(Boolean))];
  if (uniqueKeys.length === 0) return;
  const payload = { keys: uniqueKeys } satisfies ReadingAssistanceSettingsChangedPayload;
  await emit(READING_ASSISTANCE_SETTINGS_CHANGED, payload);
}

export function listenForReadingAssistanceSettingsChanged(
  handler: (payload: ReadingAssistanceSettingsChangedPayload) => void,
): Promise<UnlistenFn> {
  return listen<ReadingAssistanceSettingsChangedPayload>(READING_ASSISTANCE_SETTINGS_CHANGED, (event) => {
    handler(event.payload);
  });
}

export function readingAssistanceSettingsChanged(
  current: Record<string, string>,
  previous: Record<string, string>,
) {
  return READING_ASSISTANCE_SETTING_KEYS.some((key) => current[key] !== previous[key]);
}
