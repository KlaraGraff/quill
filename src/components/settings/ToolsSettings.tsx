import { useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Highlighter, LayoutPanelTop, Languages, MousePointer2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  LEARNING_CARD_CONFIG_SETTING_KEY,
  createDefaultCardDesignConfig,
  parseCardDesignConfig,
  serializeCardDesignConfig,
  type CardDesignConfigV1,
  type LearningCardKind,
  type SelectionMenuKind,
} from "../learning-card";
import Select from "../ui/Select";
import Toggle from "../ui/Toggle";
import CardDesignSettings from "./CardDesignSettings";
import CardPreview from "./CardPreview";
import DensityHelpDialog from "./DensityHelpDialog";
import { LANGUAGE_OPTIONS } from "./languageOptions";
import SelectionMenuSettings from "./SelectionMenuSettings";
import type { SettingsProps } from "./types";

type ToolsView = "languages" | "cards" | "menu" | "markers";
type LanguageSection = "lookup" | "explain" | "translate";

function AccordionHeader({
  title,
  subtitle,
  open,
  onClick,
}: {
  title: string;
  subtitle: string;
  open: boolean;
  onClick: () => void;
}) {
  const Icon = open ? ChevronDown : ChevronRight;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-expanded={open}
      className="flex min-h-[55px] w-full items-center justify-between gap-3 py-2.5 text-left"
    >
      <div className="min-w-0 flex-1">
        <p className="text-[12px] font-semibold uppercase tracking-[0.3px] text-text-muted">{title}</p>
        <p className="break-words text-[11px] leading-[18px] text-text-placeholder">{subtitle}</p>
      </div>
      <Icon size={16} className="shrink-0 text-text-placeholder" />
    </button>
  );
}

function AccordionBody({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      aria-hidden={!open}
      className={`grid w-full min-w-0 transition-[grid-template-rows,opacity,visibility] duration-200 ${open ? "visible grid-rows-[1fr] opacity-100" : "invisible grid-rows-[0fr] opacity-0"}`}
    >
      <div className="min-w-0 overflow-hidden">{children}</div>
    </div>
  );
}

function SettingsRow({ title, subtitle, children }: { title: string; subtitle: string; children: ReactNode }) {
  return (
    <div className="flex min-h-[52px] w-full items-center justify-between gap-4 px-1 py-1.5">
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium text-text-primary">{title}</p>
        <p className="break-words text-[11px] leading-[17px] text-text-placeholder">{subtitle}</p>
      </div>
      {children}
    </div>
  );
}

function setWordTranslationModule(config: CardDesignConfigV1, enabled: boolean): CardDesignConfigV1 {
  return {
    ...config,
    cards: {
      ...config.cards,
      word: {
        ...config.cards.word,
        modules: config.cards.word.modules.map((module) =>
          module.id === "target_translation" ? { ...module, enabled } : module,
        ),
      },
    },
  };
}

function wordTranslationEnabled(config: CardDesignConfigV1) {
  return config.cards.word.modules.find((module) => module.id === "target_translation")?.enabled ?? true;
}

export default function ToolsSettings({ settings, loading, save, saveBulk, showSavedToast }: SettingsProps) {
  const { t } = useTranslation();
  const [view, setView] = useState<ToolsView>("languages");
  const [cardKind, setCardKind] = useState<LearningCardKind>("word");
  const [menuKind, setMenuKind] = useState<SelectionMenuKind>("phrase");
  const [densityHelpOpen, setDensityHelpOpen] = useState(false);
  const [config, setConfig] = useState<CardDesignConfigV1>(createDefaultCardDesignConfig);
  const [lookupLanguage, setLookupLanguage] = useState("selection");
  const [showTranslation, setShowTranslation] = useState(true);
  const [lookupTranslationLanguage, setLookupTranslationLanguage] = useState("");
  const [explainLanguage, setExplainLanguage] = useState("lookup");
  const [translationLanguage, setTranslationLanguage] = useState("");
  const [autoHighlightLookupWords, setAutoHighlightLookupWords] = useState(true);
  const [openSections, setOpenSections] = useState<Record<LanguageSection, boolean>>({
    lookup: true,
    explain: false,
    translate: false,
  });
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const hydratedRef = useRef(false);

  useEffect(() => {
    if (loading || hydratedRef.current) return;
    let parsed = parseCardDesignConfig(settings[LEARNING_CARD_CONFIG_SETTING_KEY]);
    if (!settings[LEARNING_CARD_CONFIG_SETTING_KEY] && settings.show_translation !== undefined) {
      parsed = setWordTranslationModule(parsed, settings.show_translation === "true");
    }
    setConfig(parsed);
    setLookupLanguage(settings.lookup_language || "selection");
    setLookupTranslationLanguage(settings.lookup_translation_language || settings.language || "en");
    setShowTranslation(wordTranslationEnabled(parsed));
    setExplainLanguage(settings.explain_language || "lookup");
    setTranslationLanguage(settings.translation_language || settings.language || "en");
    setAutoHighlightLookupWords(settings.auto_highlight_lookup_words !== "false");
    hydratedRef.current = true;
  }, [settings, loading]);

  if (loading) return null;

  const queueSave = (entries: Record<string, string>) => {
    saveQueue.current = saveQueue.current
      .catch(() => {})
      .then(() => saveBulk(entries))
      .then(() => showSavedToast())
      .catch((error) => console.error("Failed to save learning tool settings:", error));
  };
  const persistConfig = (next: CardDesignConfigV1) => {
    const normalized = parseCardDesignConfig(next);
    const translationEnabled = wordTranslationEnabled(normalized);
    setConfig(normalized);
    setShowTranslation(translationEnabled);
    queueSave({
      [LEARNING_CARD_CONFIG_SETTING_KEY]: serializeCardDesignConfig(normalized),
      show_translation: String(translationEnabled),
    });
  };
  const persistLegacy = (key: string, value: string) => {
    save(key, value).then(() => showSavedToast()).catch((error) => {
      console.error(`Failed to save ${key}:`, error);
    });
  };
  const updateCard = (kind: LearningCardKind, card: CardDesignConfigV1["cards"][LearningCardKind]) => {
    persistConfig({ ...config, cards: { ...config.cards, [kind]: card } });
  };
  const explanationLanguage = cardKind === "passage"
    ? explainLanguage === "lookup" ? lookupLanguage : explainLanguage
    : lookupLanguage;
  const resolvedExplanationLanguage = explanationLanguage === "selection"
    ? "en"
    : explanationLanguage;
  const targetLanguage = cardKind === "passage" ? translationLanguage : lookupTranslationLanguage;
  const views: { id: ToolsView; icon: typeof Languages; label: string }[] = [
    { id: "languages", icon: Languages, label: t("settings.tools.views.languages") },
    { id: "cards", icon: LayoutPanelTop, label: t("settings.tools.views.cards") },
    { id: "menu", icon: MousePointer2, label: t("settings.tools.views.menu") },
    {
      id: "markers",
      icon: Highlighter,
      label: t("settings.tools.views.markers", { defaultValue: "正文标记" }),
    },
  ];

  return (
    <div className="w-full min-w-0 pb-10">
      <div role="tablist" className="mb-4 flex min-w-0 gap-1 border-b border-border-light">
        {views.map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={view === item.id}
              onClick={() => setView(item.id)}
              className={`flex h-10 min-w-0 items-center gap-1.5 border-b-2 px-3 text-[12px] font-medium ${view === item.id ? "border-accent text-accent-text" : "border-transparent text-text-muted hover:text-text-primary"}`}
            >
              <Icon size={14} className="shrink-0" />
              <span className="truncate">{item.label}</span>
            </button>
          );
        })}
      </div>

      {view === "languages" && (
        <div className="mx-auto w-full max-w-[620px]">
          {(["lookup", "explain", "translate"] as LanguageSection[]).map((section, index) => {
            const open = openSections[section];
            return (
              <div key={section} className={index > 0 ? "border-t border-border-light" : ""}>
                <AccordionHeader
                  title={t(`settings.tools.${section}`)}
                  subtitle={t(`settings.tools.${section}Sub`)}
                  open={open}
                  onClick={() => setOpenSections((current) => ({ ...current, [section]: !current[section] }))}
                />
                <AccordionBody open={open}>
                  {section === "lookup" && (
                    <div className="pb-3">
                      <SettingsRow title={t("settings.tools.lookupLanguage")} subtitle={t("settings.tools.lookupLanguageHint")}>
                        <Select
                          className="w-[175px] shrink-0"
                          value={lookupLanguage}
                          onChange={(language) => {
                            setLookupLanguage(language);
                            persistLegacy("lookup_language", language);
                          }}
                          options={[{ value: "selection", label: t("settings.tools.sameAsSelection") }, ...LANGUAGE_OPTIONS]}
                        />
                      </SettingsRow>
                      <SettingsRow title={t("settings.tools.briefTranslation")} subtitle={t("settings.tools.briefTranslationHint")}>
                        <Toggle
                          checked={showTranslation}
                          onChange={(enabled) => persistConfig(setWordTranslationModule(config, enabled))}
                        />
                      </SettingsRow>
                      <SettingsRow title={t("settings.tools.glossTarget")} subtitle={t("settings.tools.glossTargetHint")}>
                        <Select
                          className="w-[130px] shrink-0"
                          value={lookupTranslationLanguage}
                          placeholder={t("settings.languageUnset")}
                          onChange={(language) => {
                            setLookupTranslationLanguage(language);
                            persistLegacy("lookup_translation_language", language);
                          }}
                          options={LANGUAGE_OPTIONS}
                        />
                      </SettingsRow>
                    </div>
                  )}
                  {section === "explain" && (
                    <div className="pb-3">
                      <SettingsRow title={t("settings.tools.explainLanguage")} subtitle={t("settings.tools.explainLanguageHint")}>
                        <Select
                          className="w-[150px] shrink-0"
                          value={explainLanguage}
                          onChange={(language) => {
                            setExplainLanguage(language);
                            persistLegacy("explain_language", language);
                          }}
                          options={[{ value: "lookup", label: t("settings.tools.sameAsLookup") }, ...LANGUAGE_OPTIONS]}
                        />
                      </SettingsRow>
                    </div>
                  )}
                  {section === "translate" && (
                    <div className="pb-3">
                      <SettingsRow title={t("settings.tools.translateTo")} subtitle={t("settings.tools.translateToHint")}>
                        <Select
                          className="w-[130px] shrink-0"
                          value={translationLanguage}
                          placeholder={t("settings.languageUnset")}
                          onChange={(language) => {
                            setTranslationLanguage(language);
                            persistLegacy("translation_language", language);
                          }}
                          options={LANGUAGE_OPTIONS}
                        />
                      </SettingsRow>
                    </div>
                  )}
                </AccordionBody>
              </div>
            );
          })}
        </div>
      )}

      {view === "cards" && (
        <div>
          <div className="mb-4 flex gap-1 border-b border-border-light" role="tablist">
            {(["word", "phrase", "passage"] as LearningCardKind[]).map((kind) => (
              <button
                key={kind}
                type="button"
                role="tab"
                aria-selected={cardKind === kind}
                onClick={() => setCardKind(kind)}
                className={`h-9 border-b-2 px-3 text-[12px] font-medium ${cardKind === kind ? "border-accent text-accent-text" : "border-transparent text-text-muted"}`}
              >
                {t(`settings.tools.cardKind.${kind}`)}
              </button>
            ))}
          </div>
          <div className="grid min-w-0 gap-6 lg:grid-cols-[minmax(300px,380px)_minmax(0,1fr)]">
            <CardDesignSettings
              kind={cardKind}
              value={config.cards[cardKind]}
              onChange={(card) => updateCard(cardKind, card)}
              onOpenDensityHelp={() => setDensityHelpOpen(true)}
            />
            <CardPreview
              kind={cardKind}
              config={config}
              explanationLanguage={resolvedExplanationLanguage}
              targetLanguage={targetLanguage}
              learnerLevel={settings.cefr_level || "B1"}
              explanationMode={settings.explanation_mode || "adaptive_bilingual"}
            />
          </div>
        </div>
      )}

      {view === "menu" && (
        <div>
          <div className="mb-4 flex gap-1 border-b border-border-light" role="tablist">
            {(["phrase", "passage"] as SelectionMenuKind[]).map((kind) => (
              <button
                key={kind}
                type="button"
                role="tab"
                aria-selected={menuKind === kind}
                onClick={() => setMenuKind(kind)}
                className={`h-9 border-b-2 px-3 text-[12px] font-medium ${menuKind === kind ? "border-accent text-accent-text" : "border-transparent text-text-muted"}`}
              >
                {t(`settings.tools.cardKind.${kind}`)}
              </button>
            ))}
          </div>
          <div className="grid min-w-0 gap-6 lg:grid-cols-[minmax(300px,380px)_minmax(0,1fr)]">
            <SelectionMenuSettings
              kind={menuKind}
              value={config.selectionMenus[menuKind]}
              onChange={(menu) => persistConfig({
                ...config,
                selectionMenus: { ...config.selectionMenus, [menuKind]: menu },
              })}
            />
            <CardPreview
              kind={menuKind}
              config={config}
              explanationLanguage={menuKind === "passage" ? (explainLanguage === "lookup" ? resolvedExplanationLanguage : explainLanguage) : resolvedExplanationLanguage}
              targetLanguage={menuKind === "passage" ? translationLanguage : lookupTranslationLanguage}
              learnerLevel={settings.cefr_level || "B1"}
              explanationMode={settings.explanation_mode || "adaptive_bilingual"}
              showMenu
            />
          </div>
        </div>
      )}

      {view === "markers" && (
        <div className="mx-auto w-full max-w-[620px]">
          <SettingsRow
            title={t("settings.tools.autoHighlightLookupWords", { defaultValue: "查词后自动标记" })}
            subtitle={t("settings.tools.autoHighlightLookupWordsHint", {
              defaultValue: "单击查询单词后，在本书中标记所有相同拼写；手动高亮不受影响。",
            })}
          >
            <Toggle
              checked={autoHighlightLookupWords}
              onChange={(enabled) => {
                setAutoHighlightLookupWords(enabled);
                persistLegacy("auto_highlight_lookup_words", String(enabled));
              }}
            />
          </SettingsRow>
          <p className="border-t border-border-light px-1 py-3 text-[11px] leading-[18px] text-text-muted">
            {t("settings.tools.wordMarkExactHint", {
              defaultValue: "当前版本忽略大小写并只匹配完整的相同拼写。关闭后不会删除已有标记，可在正文菜单中取消某个单词的全书标记。",
            })}
          </p>
        </div>
      )}

      {densityHelpOpen && <DensityHelpDialog initialKind={cardKind} onClose={() => setDensityHelpOpen(false)} />}
    </div>
  );
}
