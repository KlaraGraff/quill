import type {
  CardDesignConfigV1,
  CardKindConfig,
  CardModuleConfig,
  CardWidthMode,
  ContentDensity,
  LearningCardKind,
  LearningModuleDefinition,
  LearningModuleId,
  ModuleDensity,
  SelectionMenuActionDefinition,
  SelectionMenuActionId,
  SelectionMenuItemConfig,
  SelectionMenuKind,
} from "./types";

export const LEARNING_CARD_CONFIG_SETTING_KEY = "learning_card_config";

export const CARD_KIND_ORDER: LearningCardKind[] = ["word", "phrase", "passage"];
export const SELECTION_MENU_KIND_ORDER: SelectionMenuKind[] = ["phrase", "passage"];
export const DENSITY_ORDER: ContentDensity[] = ["compact", "standard", "detailed"];

export const CARD_TARGET_WIDTHS: Record<LearningCardKind, Record<ContentDensity, number>> = {
  word: { compact: 380, standard: 480, detailed: 600 },
  phrase: { compact: 420, standard: 520, detailed: 620 },
  passage: { compact: 460, standard: 560, detailed: 680 },
};

const definition = (
  id: LearningModuleId,
  required = false,
): LearningModuleDefinition => ({
  id,
  required,
  labelKey: `settings.tools.modules.${id}`,
  descriptionKey: `settings.tools.modules.${id}Hint`,
});

export const MODULE_DEFINITIONS: Record<LearningCardKind, LearningModuleDefinition[]> = {
  word: [
    definition("context_meaning", true),
    definition("word_info", true),
    definition("target_translation"),
    definition("common_senses"),
    definition("collocations"),
    definition("morphology"),
    definition("grammar_role"),
    definition("synonyms"),
    definition("usage"),
    definition("memory_aid"),
    definition("source_excerpt"),
  ],
  phrase: [
    definition("context_meaning", true),
    definition("target_translation"),
    definition("common_senses"),
    definition("collocations"),
    definition("grammar_analysis"),
    definition("idioms"),
    definition("usage"),
    definition("source_excerpt"),
  ],
  passage: [
    definition("context_meaning", true),
    definition("target_translation"),
    definition("grammar_analysis"),
    definition("key_terms"),
    definition("idioms"),
    definition("references"),
    definition("reusable_patterns"),
    definition("tone"),
    definition("source_excerpt"),
  ],
};

const menuAction = (id: SelectionMenuActionId): SelectionMenuActionDefinition => ({
  id,
  labelKey: `settings.tools.menuActions.${id}`,
});

export const MENU_ACTION_DEFINITIONS: Record<SelectionMenuKind, SelectionMenuActionDefinition[]> = {
  phrase: [
    menuAction("define"),
    menuAction("ask_ai"),
    menuAction("collect"),
    menuAction("highlight"),
    menuAction("copy"),
    menuAction("translate"),
  ],
  passage: [
    menuAction("explain"),
    menuAction("ask_ai"),
    menuAction("collect"),
    menuAction("highlight"),
    menuAction("copy"),
    menuAction("translate"),
  ],
};

const moduleConfig = (
  id: LearningModuleId,
  enabled: boolean,
  defaultExpanded = true,
  density: ModuleDensity = "inherit",
): CardModuleConfig => ({ id, enabled, defaultExpanded, density });

const defaultCard = (
  kind: LearningCardKind,
  enabled: LearningModuleId[],
  collapsed: LearningModuleId[] = [],
): CardKindConfig => ({
  defaultDensity: "standard",
  widthMode: "auto",
  exampleCount: 1,
  keyTermCount: kind === "passage" ? 3 : 3,
  modules: MODULE_DEFINITIONS[kind].map(({ id, required }) =>
    moduleConfig(id, required || enabled.includes(id), !collapsed.includes(id)),
  ),
});

export function createDefaultCardDesignConfig(): CardDesignConfigV1 {
  return {
    version: 1,
    cards: {
      word: defaultCard(
        "word",
        ["target_translation", "common_senses", "collocations", "morphology", "grammar_role"],
        ["morphology", "grammar_role"],
      ),
      phrase: defaultCard(
        "phrase",
        ["target_translation", "common_senses", "collocations", "grammar_analysis", "idioms"],
        ["grammar_analysis", "idioms"],
      ),
      passage: defaultCard(
        "passage",
        ["target_translation", "grammar_analysis", "key_terms", "idioms", "references"],
        ["grammar_analysis", "key_terms", "idioms", "references"],
      ),
    },
    selectionMenus: {
      phrase: MENU_ACTION_DEFINITIONS.phrase.map(({ id }) => ({ id, enabled: id !== "translate" })),
      passage: MENU_ACTION_DEFINITIONS.passage.map(({ id }) => ({ id, enabled: id !== "translate" })),
    },
  };
}

export const DEFAULT_CARD_DESIGN_CONFIG = createDefaultCardDesignConfig();

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const isDensity = (value: unknown): value is ContentDensity =>
  value === "compact" || value === "standard" || value === "detailed";

const isModuleDensity = (value: unknown): value is ModuleDensity =>
  value === "inherit" || isDensity(value);

const isWidthMode = (value: unknown): value is CardWidthMode =>
  value === "auto" || value === "compact" || value === "wide";

const clampInteger = (value: unknown, fallback: number, min: number, max: number) => {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.round(value)));
};

function parseModules(
  kind: LearningCardKind,
  value: unknown,
  defaults: CardModuleConfig[],
): CardModuleConfig[] {
  if (!Array.isArray(value)) return defaults.map((item) => ({ ...item }));
  const allowed = new Map(MODULE_DEFINITIONS[kind].map((item) => [item.id, item]));
  const defaultById = new Map(defaults.map((item) => [item.id, item]));
  const seen = new Set<LearningModuleId>();
  const parsed: CardModuleConfig[] = [];

  for (const item of value) {
    if (!isObject(item) || typeof item.id !== "string") continue;
    const moduleDefinition = allowed.get(item.id as LearningModuleId);
    const fallback = defaultById.get(item.id as LearningModuleId);
    if (!moduleDefinition || !fallback || seen.has(moduleDefinition.id)) continue;
    seen.add(moduleDefinition.id);
    parsed.push({
      id: moduleDefinition.id,
      enabled: moduleDefinition.required ? true : typeof item.enabled === "boolean" ? item.enabled : fallback.enabled,
      defaultExpanded: typeof item.defaultExpanded === "boolean"
        ? item.defaultExpanded
        : fallback.defaultExpanded,
      density: isModuleDensity(item.density) ? item.density : fallback.density,
    });
  }

  for (const fallback of defaults) {
    if (!seen.has(fallback.id)) parsed.push({ ...fallback });
  }
  return parsed;
}

function parseCard(
  kind: LearningCardKind,
  value: unknown,
  fallback: CardKindConfig,
): CardKindConfig {
  if (!isObject(value)) return { ...fallback, modules: fallback.modules.map((item) => ({ ...item })) };
  return {
    defaultDensity: isDensity(value.defaultDensity) ? value.defaultDensity : fallback.defaultDensity,
    widthMode: isWidthMode(value.widthMode) ? value.widthMode : fallback.widthMode,
    exampleCount: clampInteger(value.exampleCount, fallback.exampleCount, 0, 3),
    keyTermCount: clampInteger(value.keyTermCount, fallback.keyTermCount, 1, 8),
    modules: parseModules(kind, value.modules, fallback.modules),
  };
}

function parseMenu(
  kind: SelectionMenuKind,
  value: unknown,
  defaults: SelectionMenuItemConfig[],
): SelectionMenuItemConfig[] {
  if (!Array.isArray(value)) return defaults.map((item) => ({ ...item }));
  const allowed = new Set(MENU_ACTION_DEFINITIONS[kind].map((item) => item.id));
  const defaultById = new Map(defaults.map((item) => [item.id, item]));
  const seen = new Set<SelectionMenuActionId>();
  const parsed: SelectionMenuItemConfig[] = [];

  for (const item of value) {
    if (!isObject(item) || typeof item.id !== "string") continue;
    const id = item.id as SelectionMenuActionId;
    const fallback = defaultById.get(id);
    if (!allowed.has(id) || !fallback || seen.has(id)) continue;
    seen.add(id);
    parsed.push({ id, enabled: typeof item.enabled === "boolean" ? item.enabled : fallback.enabled });
  }
  for (const fallback of defaults) {
    if (!seen.has(fallback.id)) parsed.push({ ...fallback });
  }
  return parsed;
}

export function parseCardDesignConfig(value: unknown): CardDesignConfigV1 {
  const defaults = createDefaultCardDesignConfig();
  let candidate = value;
  if (typeof candidate === "string") {
    try {
      candidate = JSON.parse(candidate);
    } catch {
      return defaults;
    }
  }
  if (!isObject(candidate) || candidate.version !== 1) return defaults;
  const cards = isObject(candidate.cards) ? candidate.cards : {};
  const selectionMenus = isObject(candidate.selectionMenus) ? candidate.selectionMenus : {};
  return {
    version: 1,
    cards: {
      word: parseCard("word", cards.word, defaults.cards.word),
      phrase: parseCard("phrase", cards.phrase, defaults.cards.phrase),
      passage: parseCard("passage", cards.passage, defaults.cards.passage),
    },
    selectionMenus: {
      phrase: parseMenu("phrase", selectionMenus.phrase, defaults.selectionMenus.phrase),
      passage: parseMenu("passage", selectionMenus.passage, defaults.selectionMenus.passage),
    },
  };
}

export function serializeCardDesignConfig(config: CardDesignConfigV1): string {
  return JSON.stringify(parseCardDesignConfig(config));
}

export function getEffectiveDensity(
  module: CardModuleConfig,
  card: CardKindConfig,
): ContentDensity {
  return module.density === "inherit" ? card.defaultDensity : module.density;
}

export function getCardLayoutDensity(card: CardKindConfig): ContentDensity {
  let index = DENSITY_ORDER.indexOf(card.defaultDensity);
  for (const module of card.modules) {
    if (!module.enabled) continue;
    index = Math.max(index, DENSITY_ORDER.indexOf(getEffectiveDensity(module, card)));
  }
  return DENSITY_ORDER[Math.max(0, index)];
}

export function getLearningCardTargetWidth(
  kind: LearningCardKind,
  card: CardKindConfig,
): number {
  if (card.widthMode === "compact") return CARD_TARGET_WIDTHS[kind].compact;
  if (card.widthMode === "wide") return CARD_TARGET_WIDTHS[kind].detailed;
  return CARD_TARGET_WIDTHS[kind][getCardLayoutDensity(card)];
}

export function getResponsiveLearningCardWidth(
  kind: LearningCardKind,
  card: CardKindConfig,
  availableWidth: number,
  viewportMargin = 12,
): number {
  const safeAvailableWidth = Number.isFinite(availableWidth) ? Math.max(0, availableWidth) : 0;
  return Math.max(0, Math.min(
    getLearningCardTargetWidth(kind, card),
    safeAvailableWidth - viewportMargin * 2,
  ));
}

export function reorderArray<T>(items: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || from >= items.length || to < 0 || to >= items.length) return items;
  const next = [...items];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}
