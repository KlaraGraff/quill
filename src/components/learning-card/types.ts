export type LearningCardKind = "word" | "phrase" | "passage";
export type ContentDensity = "compact" | "standard" | "detailed";
export type ModuleDensity = "inherit" | ContentDensity;
export type CardWidthMode = "auto" | "compact" | "wide";

export type LearningModuleId =
  | "context_meaning"
  | "word_info"
  | "target_translation"
  | "common_senses"
  | "collocations"
  | "morphology"
  | "grammar_role"
  | "grammar_analysis"
  | "synonyms"
  | "usage"
  | "key_terms"
  | "idioms"
  | "references"
  | "reusable_patterns"
  | "tone"
  | "memory_aid"
  | "source_excerpt";

export type SelectionMenuKind = "phrase" | "passage";
export type SelectionMenuActionId =
  | "define"
  | "explain"
  | "ask_ai"
  | "collect"
  | "highlight"
  | "translate"
  | "copy";

export interface CardModuleConfig {
  id: LearningModuleId;
  enabled: boolean;
  defaultExpanded: boolean;
  density: ModuleDensity;
}

export interface CardKindConfig {
  defaultDensity: ContentDensity;
  widthMode: CardWidthMode;
  exampleCount: number;
  keyTermCount: number;
  modules: CardModuleConfig[];
}

export interface SelectionMenuItemConfig {
  id: SelectionMenuActionId;
  enabled: boolean;
}

export interface CardDesignConfigV1 {
  version: 1;
  cards: Record<LearningCardKind, CardKindConfig>;
  selectionMenus: Record<SelectionMenuKind, SelectionMenuItemConfig[]>;
}

export interface LearningModuleDefinition {
  id: LearningModuleId;
  labelKey: string;
  descriptionKey: string;
  required: boolean;
}

export interface SelectionMenuActionDefinition {
  id: SelectionMenuActionId;
  labelKey: string;
}

export interface LearningExample {
  source: string;
  target?: string;
}

export interface LearningContentItem {
  title: string;
  text?: string;
  meta?: string[];
  examples?: LearningExample[];
}

export interface LearningModuleContent {
  heading?: string;
  summary?: string;
  meta?: string[];
  details?: string[];
  items?: LearningContentItem[];
  quote?: string;
}

export interface LearningCardResult {
  version: number;
  kind: LearningCardKind;
  sourceText: string;
  modules: Partial<Record<LearningModuleId, LearningModuleContent>>;
}

export interface LearningCardNote {
  id: string;
  content: string;
  updatedAt?: number;
  scope?: "book" | "global";
}

export type LearningCardActionId = "collect" | "ask_ai" | "note" | "copy";

export interface LearningCardActionState {
  collected?: boolean;
  copied?: boolean;
  disabled?: boolean;
}
