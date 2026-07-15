import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  type PageColumns,
  type PageTurnAnimation,
  type ReaderSettingsState,
  type ReadingMode,
} from "../../components/ReaderSettings";
import {
  getDefaultReaderTheme,
  isReaderFontAvailable,
} from "../../components/reader-settings";
import {
  DEFAULT_NEXT_PAGE_BINDING,
  DEFAULT_PREVIOUS_PAGE_BINDING,
} from "../../components/page-turn-bindings";
import {
  listenForSettingsChanged,
  notifySettingsChanged,
} from "../../components/settings-events";

const readerPreferenceSettingKeys = {
  margins: "margins",
  readingMode: "reading_mode",
  pageColumns: "page_columns",
  pageTurnAnimation: "page_turn_animation",
  showBookProgress: "show_book_progress",
  showPageNumbers: "show_page_numbers",
  previousPageBinding: "previous_page_binding",
  nextPageBinding: "next_page_binding",
} as const;

function booleanSetting(value: string | undefined, fallback: boolean): boolean {
  if (value === "true") return true;
  if (value === "false") return false;
  return fallback;
}

function readingModeSetting(value: string | undefined, fallback: ReadingMode): ReadingMode {
  return value === "paginated" || value === "scrolling" ? value : fallback;
}

function pageColumnsSetting(value: string | undefined, fallback: PageColumns): PageColumns {
  return value === "1" ? 1 : value === "2" ? 2 : fallback;
}

function pageTurnAnimationSetting(
  value: string | undefined,
  fallback: PageTurnAnimation,
): PageTurnAnimation {
  return value === "none" || value === "slide" || value === "fade" || value === "cover"
    ? value
    : fallback;
}

function marginSetting(value: string | number | undefined, fallback: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.min(30, Math.max(0, parsed)) : fallback;
}

function createDefaultReaderSettings(): ReaderSettingsState {
  return {
    theme: getDefaultReaderTheme(),
    font: "palatino",
    fontSize: 26,
    brightness: 100,
    readingMode: "scrolling",
    pageColumns: 2,
    pageTurnAnimation: "slide",
    showBookProgress: false,
    showPageNumbers: false,
    previousPageBinding: DEFAULT_PREVIOUS_PAGE_BINDING,
    nextPageBinding: DEFAULT_NEXT_PAGE_BINDING,
    lineSpacing: 1.8,
    charSpacing: 0,
    wordSpacing: 0,
    margins: 0,
    showLookupMarkers: true,
    showNewVocabMarkers: true,
    showLearningMarkers: true,
    showMasteredMarkers: false,
  };
}

export function mergeStoredReaderSettings(
  previous: ReaderSettingsState,
  bookSettings: Partial<ReaderSettingsState>,
  globalSettings: Record<string, string>,
): ReaderSettingsState {
  const requestedFont = bookSettings.font
    || (globalSettings.font_family as ReaderSettingsState["font"])
    || previous.font;
  return {
    ...previous,
    theme: bookSettings.theme
      || (globalSettings.reader_theme as ReaderSettingsState["theme"])
      || previous.theme,
    brightness: bookSettings.brightness
      ?? (globalSettings.brightness ? parseInt(globalSettings.brightness) : previous.brightness),
    pageColumns: bookSettings.pageColumns
      ?? pageColumnsSetting(globalSettings.page_columns, previous.pageColumns),
    font: isReaderFontAvailable(requestedFont) ? requestedFont : "system",
    fontSize: bookSettings.fontSize
      ?? (globalSettings.font_size ? parseInt(globalSettings.font_size) : previous.fontSize),
    readingMode: bookSettings.readingMode
      || readingModeSetting(globalSettings.reading_mode, previous.readingMode),
    pageTurnAnimation: bookSettings.pageTurnAnimation
      ?? pageTurnAnimationSetting(globalSettings.page_turn_animation, previous.pageTurnAnimation),
    showBookProgress: bookSettings.showBookProgress
      ?? booleanSetting(globalSettings.show_book_progress, previous.showBookProgress),
    showPageNumbers: bookSettings.showPageNumbers
      ?? booleanSetting(globalSettings.show_page_numbers, previous.showPageNumbers),
    previousPageBinding: bookSettings.previousPageBinding
      || globalSettings.previous_page_binding
      || previous.previousPageBinding,
    nextPageBinding: bookSettings.nextPageBinding
      || globalSettings.next_page_binding
      || previous.nextPageBinding,
    lineSpacing: bookSettings.lineSpacing
      ?? (globalSettings.line_spacing ? parseFloat(globalSettings.line_spacing) : previous.lineSpacing),
    charSpacing: bookSettings.charSpacing
      ?? (globalSettings.char_spacing ? parseInt(globalSettings.char_spacing) : previous.charSpacing),
    wordSpacing: bookSettings.wordSpacing
      ?? (globalSettings.word_spacing ? parseInt(globalSettings.word_spacing) : previous.wordSpacing),
    // Global-first keeps the Settings page and reader toolbar synchronized.
    margins: marginSetting(globalSettings.margins ?? bookSettings.margins, previous.margins),
    showLookupMarkers: bookSettings.showLookupMarkers ?? previous.showLookupMarkers,
    showNewVocabMarkers: bookSettings.showNewVocabMarkers ?? previous.showNewVocabMarkers,
    showLearningMarkers: bookSettings.showLearningMarkers ?? previous.showLearningMarkers,
    showMasteredMarkers: bookSettings.showMasteredMarkers ?? previous.showMasteredMarkers,
  };
}

interface ReaderSettingsController {
  readerSettings: ReaderSettingsState;
  setReaderSettings: Dispatch<SetStateAction<ReaderSettingsState>>;
  readerSettingsRef: MutableRefObject<ReaderSettingsState>;
  settingsLoadedBookRef: MutableRefObject<string | null>;
  handleReaderSettingsChange(next: ReaderSettingsState): void;
}

export function useReaderSettingsSync(bookId: string | undefined): ReaderSettingsController {
  const [readerSettings, setReaderSettings] = useState<ReaderSettingsState>(createDefaultReaderSettings);
  const readerSettingsRef = useRef(readerSettings);
  const settingsLoadedBookRef = useRef<string | null>(null);
  const saveTimerRef = useRef<number | null>(null);
  const pendingPreferencesRef = useRef<Record<string, string>>({});

  useEffect(() => {
    readerSettingsRef.current = readerSettings;
  }, [readerSettings]);

  const flushReaderPreferences = useCallback(async () => {
    saveTimerRef.current = null;
    const values = pendingPreferencesRef.current;
    pendingPreferencesRef.current = {};
    if (Object.keys(values).length === 0) return;
    try {
      await invoke("set_settings_bulk", { settings: values });
      await notifySettingsChanged(values).catch(() => {});
    } catch {
      pendingPreferencesRef.current = {
        ...values,
        ...pendingPreferencesRef.current,
      };
    }
  }, []);

  const scheduleReaderPreferenceSave = useCallback((values: Record<string, string>) => {
    if (Object.keys(values).length === 0) return;
    pendingPreferencesRef.current = {
      ...pendingPreferencesRef.current,
      ...values,
    };
    if (saveTimerRef.current !== null) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = window.setTimeout(() => {
      void flushReaderPreferences();
    }, 400);
  }, [flushReaderPreferences]);

  useEffect(() => () => {
    if (saveTimerRef.current !== null) window.clearTimeout(saveTimerRef.current);
    void flushReaderPreferences();
  }, [flushReaderPreferences]);

  const handleReaderSettingsChange = useCallback((next: ReaderSettingsState) => {
    const previous = readerSettingsRef.current;
    readerSettingsRef.current = next;
    setReaderSettings(next);
    const changed: Record<string, string> = {};
    if (previous.margins !== next.margins) changed[readerPreferenceSettingKeys.margins] = String(next.margins);
    if (previous.readingMode !== next.readingMode) changed[readerPreferenceSettingKeys.readingMode] = next.readingMode;
    if (previous.pageColumns !== next.pageColumns) changed[readerPreferenceSettingKeys.pageColumns] = String(next.pageColumns);
    if (previous.pageTurnAnimation !== next.pageTurnAnimation) changed[readerPreferenceSettingKeys.pageTurnAnimation] = next.pageTurnAnimation;
    if (previous.showBookProgress !== next.showBookProgress) changed[readerPreferenceSettingKeys.showBookProgress] = String(next.showBookProgress);
    if (previous.showPageNumbers !== next.showPageNumbers) changed[readerPreferenceSettingKeys.showPageNumbers] = String(next.showPageNumbers);
    if (previous.previousPageBinding !== next.previousPageBinding) changed[readerPreferenceSettingKeys.previousPageBinding] = next.previousPageBinding;
    if (previous.nextPageBinding !== next.nextPageBinding) changed[readerPreferenceSettingKeys.nextPageBinding] = next.nextPageBinding;
    scheduleReaderPreferenceSave(changed);
  }, [scheduleReaderPreferenceSave]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listenForSettingsChanged((values) => {
      if (disposed) return;
      setReaderSettings((current) => {
        const next = {
          ...current,
          margins: marginSetting(values[readerPreferenceSettingKeys.margins], current.margins),
          readingMode: readingModeSetting(
            values[readerPreferenceSettingKeys.readingMode],
            current.readingMode,
          ),
          pageColumns: pageColumnsSetting(
            values[readerPreferenceSettingKeys.pageColumns],
            current.pageColumns,
          ),
          pageTurnAnimation: pageTurnAnimationSetting(
            values[readerPreferenceSettingKeys.pageTurnAnimation],
            current.pageTurnAnimation,
          ),
          showBookProgress: booleanSetting(
            values[readerPreferenceSettingKeys.showBookProgress],
            current.showBookProgress,
          ),
          showPageNumbers: booleanSetting(
            values[readerPreferenceSettingKeys.showPageNumbers],
            current.showPageNumbers,
          ),
          previousPageBinding: values[readerPreferenceSettingKeys.previousPageBinding]
            || current.previousPageBinding,
          nextPageBinding: values[readerPreferenceSettingKeys.nextPageBinding]
            || current.nextPageBinding,
        };
        readerSettingsRef.current = next;
        return next;
      });
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (settingsLoadedBookRef.current !== bookId) return;
    localStorage.setItem(`reader-settings-${bookId}`, JSON.stringify(readerSettings));
  }, [bookId, readerSettings]);

  return {
    readerSettings,
    setReaderSettings,
    readerSettingsRef,
    settingsLoadedBookRef,
    handleReaderSettingsChange,
  };
}
