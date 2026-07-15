import { invoke } from "@tauri-apps/api/core";
import { normalizeInteractionText } from "./reader-interaction";

interface WordFormsRow {
  normalized_word: string;
  forms: string[];
}

export async function expandWordForms(words: string[], enabled: boolean) {
  const base = words.map(normalizeInteractionText).filter(Boolean);
  if (!enabled || base.length === 0) return base;
  const rows = await invoke<WordFormsRow[]>("get_word_forms", { words: base });
  const expanded = new Set(base);
  for (const row of rows) {
    expanded.add(normalizeInteractionText(row.normalized_word));
    for (const form of row.forms) expanded.add(normalizeInteractionText(form));
  }
  return [...expanded].filter(Boolean);
}
