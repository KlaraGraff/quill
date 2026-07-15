export const DEFAULT_PREVIOUS_PAGE_BINDING = "key:ArrowLeft";
export const DEFAULT_NEXT_PAGE_BINDING = "key:ArrowRight";
export const READER_BINDINGS_SETTING_KEY = "reader_bindings";

export type BuiltInReaderActionId = "lookup" | "translate" | "collect" | "highlight" | "copy" | "ask_ai" | "explain";
export type ReaderActionId = BuiltInReaderActionId | `custom_${string}`;
export interface ReaderActionBinding { actionId: ReaderActionId; trigger: string }
export interface ReaderBindingsConfig { version: 1; bindings: ReaderActionBinding[] }

const MODIFIER_KEYS = new Set(["Alt", "Control", "Meta", "Shift"]);
const RESERVED_BINDINGS = new Set([
  "key:Meta+C", "key:Meta+V", "key:Meta+X", "key:Meta+A", "key:Meta+W", "key:Meta+Q",
  "key:Control+C", "key:Control+V", "key:Control+X", "key:Control+A",
]);

function normalizedKey(key: string): string {
  if (key === " ") return "Space";
  return key.length === 1 ? key.toUpperCase() : key;
}

export function bindingFromKeyboardEvent(event: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(event.key)) return null;
  const modifiers = [
    event.metaKey ? "Meta" : null,
    event.ctrlKey ? "Control" : null,
    event.altKey ? "Alt" : null,
    event.shiftKey ? "Shift" : null,
  ].filter(Boolean);
  return `key:${[...modifiers, normalizedKey(event.key)].join("+")}`;
}

export function bindingFromMouseEvent(event: MouseEvent): string | null {
  if (event.button === 0) return null;
  return `mouse:${event.button}`;
}

export function keyboardEventMatchesBinding(event: KeyboardEvent, binding: string): boolean {
  return bindingFromKeyboardEvent(event) === binding;
}

export function mouseEventMatchesBinding(event: MouseEvent, binding: string): boolean {
  return bindingFromMouseEvent(event) === binding;
}

export function isReservedReaderBinding(binding: string) {
  return RESERVED_BINDINGS.has(binding);
}

export function formatReaderBinding(binding: string, locale = "en"): string {
  if (binding === "mouse:double") return locale.startsWith("zh") ? "双击" : "Double click";
  if (binding.startsWith("mouse:")) {
    const button = Number(binding.slice("mouse:".length));
    const labels: Record<number, string> = locale.startsWith("zh")
      ? { 1: "鼠标中键", 2: "鼠标右键", 3: "鼠标后退键", 4: "鼠标前进键" }
      : { 1: "Middle click", 2: "Right click", 3: "Mouse back", 4: "Mouse forward" };
    return labels[button] ?? (locale.startsWith("zh") ? `鼠标键 ${button + 1}` : `Mouse ${button + 1}`);
  }
  const value = binding.startsWith("key:") ? binding.slice("key:".length) : binding;
  return value
    .replace(/Meta/g, "Cmd")
    .replace(/Control/g, "Ctrl")
    .replace(/Alt/g, locale.startsWith("zh") ? "Option" : "Alt")
    .replace(/ArrowLeft/g, "Left")
    .replace(/ArrowRight/g, "Right")
    .replace(/ArrowUp/g, "Up")
    .replace(/ArrowDown/g, "Down")
    .replace(/Space/g, locale.startsWith("zh") ? "空格" : "Space");
}

export const formatPageTurnBinding = formatReaderBinding;

export function parseReaderBindings(value: unknown): ReaderBindingsConfig {
  let source = value;
  if (typeof source === "string") {
    try { source = JSON.parse(source); } catch { source = null; }
  }
  const record = source && typeof source === "object" ? source as Partial<ReaderBindingsConfig> : {};
  const seenActions = new Set<string>();
  const seenTriggers = new Set<string>();
  const bindings = Array.isArray(record.bindings) ? record.bindings.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const actionId = (item as ReaderActionBinding).actionId;
    const trigger = (item as ReaderActionBinding).trigger;
    if (typeof actionId !== "string" || typeof trigger !== "string"
      || (!trigger.startsWith("key:") && trigger !== "mouse:double")
      || seenActions.has(actionId) || seenTriggers.has(trigger)) return [];
    seenActions.add(actionId);
    seenTriggers.add(trigger);
    return [{ actionId, trigger } as ReaderActionBinding];
  }) : [];
  return { version: 1, bindings };
}
