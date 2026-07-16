export type InteractionKind = "word" | "phrase" | "passage";

export interface SerializableRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface ReaderSelectionSnapshot {
  range: Range;
}

export interface ReaderInteraction {
  trigger: "word-menu" | "word-quick-lookup" | "selection-menu";
  kind: InteractionKind;
  text: string;
  normalizedText: string;
  context: string;
  location: string;
  anchorRect: SerializableRect;
  source: "foliate" | "text";
  format: "epub" | "pdf" | "text";
  locale?: string;
}

const BLOCK_TAGS = new Set([
  "P", "DIV", "LI", "BLOCKQUOTE", "TD", "TH", "H1", "H2", "H3", "H4",
  "H5", "H6", "SECTION", "ARTICLE", "ASIDE", "FIGCAPTION", "DT", "DD",
]);

export function normalizeInteractionText(value: string): string {
  return value
    .normalize("NFC")
    .trim()
    .replace(/^[^\p{L}\p{M}\p{N}]+|[^\p{L}\p{M}\p{N}]+$/gu, "")
    .toLocaleLowerCase();
}

export function classifySelection(value: string, locale?: string): InteractionKind {
  const text = value.trim();
  if (!text) return "passage";
  const words = segmentInteractionWords(text, locale);
  if (words.length === 1 && words[0].segment === text) return "word";
  if (words.length <= 5 && !/[.!?。！？；;:\n\r]/u.test(text)) return "phrase";
  return "passage";
}

const WORD_CONNECTOR = /^['’\-\u2010\u2011]$/u;

export interface InteractionWordSegment {
  segment: string;
  index: number;
}

export function segmentInteractionWords(text: string, locale?: string): InteractionWordSegment[] {
  const Segmenter = (Intl as typeof Intl & {
    Segmenter?: new (
      locale?: string,
      options?: { granularity: "word" },
    ) => { segment(value: string): Iterable<{ segment: string; index: number; isWordLike?: boolean }> };
  }).Segmenter;
  if (Segmenter) {
    const segmenter = new Segmenter(locale, { granularity: "word" });
    const parts = Array.from(segmenter.segment(text));
    const words: Array<{ segment: string; index: number }> = [];
    for (let index = 0; index < parts.length; index += 1) {
      const part = parts[index];
      if (!part.isWordLike) continue;
      const start = part.index;
      let end = start + part.segment.length;
      while (
        index + 2 < parts.length
        && parts[index + 1].index === end
        && WORD_CONNECTOR.test(parts[index + 1].segment)
        && parts[index + 2].isWordLike
        && parts[index + 2].index === end + parts[index + 1].segment.length
      ) {
        end = parts[index + 2].index + parts[index + 2].segment.length;
        index += 2;
      }
      words.push({ segment: text.slice(start, end), index: start });
    }
    return words;
  }
  return Array.from(text.matchAll(/[\p{L}\p{M}\p{N}]+(?:['’\-\u2010\u2011][\p{L}\p{M}\p{N}]+)*/gu))
    .map((match) => ({ segment: match[0], index: match.index ?? 0 }));
}

interface FlatTextEntry {
  node: Text;
  flatStart: number;
  flatEnd: number;
}

interface FlatTextRun {
  root: Element;
  text: string;
  entries: FlatTextEntry[];
}

interface DomPoint {
  node: Node;
  offset: number;
}

const INTERACTION_EXCLUSION_SELECTOR =
  "script,style,noscript,textarea,input,[contenteditable='true']";

function closestTextRunRoot(node: Node): Element | null {
  let element = node.nodeType === Node.ELEMENT_NODE
    ? node as Element
    : node.parentElement;
  while (element && !BLOCK_TAGS.has(element.tagName)) element = element.parentElement;
  return element ?? node.ownerDocument?.body ?? node.ownerDocument?.documentElement ?? null;
}

function flattenTextRun(root: Element): FlatTextRun {
  const entries: FlatTextEntry[] = [];
  let text = "";
  const walker = root.ownerDocument.createTreeWalker(
    root,
    NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT,
  );
  let current = walker.nextNode();
  while (current) {
    if (
      current.nodeType === Node.ELEMENT_NODE
      && (current as Element).tagName === "BR"
      && closestTextRunRoot(current) === root
      && !(current as Element).closest(INTERACTION_EXCLUSION_SELECTOR)
    ) {
      text += "\n";
      current = walker.nextNode();
      continue;
    }
    if (current.nodeType !== Node.TEXT_NODE) {
      current = walker.nextNode();
      continue;
    }
    const node = current as Text;
    const value = node.data;
    if (
      value
      && !node.parentElement?.closest(INTERACTION_EXCLUSION_SELECTOR)
      && closestTextRunRoot(node) === root
    ) {
      const flatStart = text.length;
      text += value;
      entries.push({ node, flatStart, flatEnd: text.length });
    }
    current = walker.nextNode();
  }
  return { root, text, entries };
}

function compareDomPoints(first: DomPoint, second: DomPoint): number {
  if (first.node === second.node) return Math.sign(first.offset - second.offset);
  const doc = first.node.ownerDocument;
  if (!doc || doc !== second.node.ownerDocument) return 0;
  const firstRange = doc.createRange();
  const secondRange = doc.createRange();
  try {
    firstRange.setStart(first.node, first.offset);
    firstRange.collapse(true);
    secondRange.setStart(second.node, second.offset);
    secondRange.collapse(true);
    return firstRange.compareBoundaryPoints(Range.START_TO_START, secondRange);
  } catch {
    return 0;
  }
}

function domPointToFlatOffset(run: FlatTextRun, node: Node, offset: number): number | null {
  const direct = run.entries.find((entry) => entry.node === node);
  if (direct) {
    return direct.flatStart + Math.min(Math.max(0, offset), direct.node.length);
  }
  if (!run.root.contains(node) && node !== run.root) return null;
  const boundary = { node, offset };
  for (const entry of run.entries) {
    if (compareDomPoints(boundary, { node: entry.node, offset: 0 }) <= 0) {
      return entry.flatStart;
    }
    if (compareDomPoints(boundary, { node: entry.node, offset: entry.node.length }) <= 0) {
      return entry.flatEnd;
    }
  }
  return run.entries.length > 0 ? run.text.length : null;
}

function domPointAtFlatOffset(
  run: FlatTextRun,
  offset: number,
  edge: "start" | "end",
): DomPoint | null {
  const clamped = Math.min(Math.max(0, offset), run.text.length);
  const entry = edge === "start"
    ? run.entries.find((candidate, index) => (
      clamped < candidate.flatEnd
      || (clamped === candidate.flatStart && index === 0)
    )) ?? run.entries[run.entries.length - 1]
    : run.entries.find((candidate) => clamped <= candidate.flatEnd)
      ?? run.entries[run.entries.length - 1];
  if (!entry) return null;
  return {
    node: entry.node,
    offset: Math.min(entry.node.length, Math.max(0, clamped - entry.flatStart)),
  };
}

function rangeForFlatSegment(run: FlatTextRun, segment: InteractionWordSegment): Range {
  const range = run.root.ownerDocument.createRange();
  const start = domPointAtFlatOffset(run, segment.index, "start");
  const end = domPointAtFlatOffset(run, segment.index + segment.segment.length, "end");
  if (!start || !end) {
    range.selectNodeContents(run.root);
    range.collapse(true);
    return range;
  }
  range.setStart(start.node, start.offset);
  range.setEnd(end.node, end.offset);
  return range;
}

function pointIntersectsRange(range: Range, x: number, y: number): boolean {
  return Array.from(range.getClientRects()).some((rect) => (
    rect.width > 0
    && rect.height > 0
    && x >= rect.left - 0.5
    && x <= rect.right + 0.5
    && y >= rect.top - 0.5
    && y <= rect.bottom + 0.5
  ));
}

function caretRangeAtPoint(doc: Document, x: number, y: number): Range | null {
  const caretDocument = doc as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  const position = caretDocument.caretPositionFromPoint?.(x, y);
  if (position) {
    const range = doc.createRange();
    range.setStart(position.offsetNode, position.offset);
    range.collapse(true);
    return range;
  }
  return caretDocument.caretRangeFromPoint?.(x, y) ?? null;
}

export function wordRangeAtPoint(
  doc: Document,
  x: number,
  y: number,
  locale?: string,
): Range | null {
  const caret = caretRangeAtPoint(doc, x, y);
  if (!caret || caret.startContainer.nodeType !== Node.TEXT_NODE) return null;
  const root = closestTextRunRoot(caret.startContainer);
  if (!root) return null;
  const run = flattenTextRun(root);
  const offset = domPointToFlatOffset(run, caret.startContainer, caret.startOffset);
  if (offset === null) return null;

  const segments = segmentInteractionWords(run.text, locale);
  const direct = segments.find(({ segment, index }) => (
    offset >= index && offset < index + segment.length
  ));
  if (direct) {
    const range = rangeForFlatSegment(run, direct);
    if (offset !== direct.index || pointIntersectsRange(range, x, y)) return range;
    const previous = segments.find(({ segment, index }) => index + segment.length === offset);
    if (!previous) return null;
    const previousRange = rangeForFlatSegment(run, previous);
    return pointIntersectsRange(previousRange, x, y) ? previousRange : null;
  }

  // Caret APIs return insertion positions. Clicking the right half of the
  // final glyph can therefore land exactly at the word end; accept that word
  // only when the pointer is still geometrically inside its rendered range.
  const previous = segments.find(({ segment, index }) => index + segment.length === offset);
  if (!previous) return null;
  const previousRange = rangeForFlatSegment(run, previous);
  return pointIntersectsRange(previousRange, x, y) ? previousRange : null;
}

export function selectedRange(doc: Document): Range | null {
  const selection = doc.getSelection?.();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  return range.toString().trim() ? range.cloneRange() : null;
}

export function snapshotSelectionRange(range: Range | null): ReaderSelectionSnapshot | null {
  if (!range) return null;
  return {
    range: range.cloneRange(),
  };
}

export function rangeFromSelectionSnapshotAtPoint(
  snapshot: ReaderSelectionSnapshot | null,
  x: number,
  y: number,
): Range | null {
  const containsPoint = Array.from(snapshot?.range.getClientRects() ?? []).some((rect) => (
    x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
  ));
  return containsPoint ? snapshot?.range.cloneRange() ?? null : null;
}

export function readerMenuActivationIndex(
  key: string,
  currentIndex: number,
  itemCount: number,
  modified = false,
): number | null {
  if (modified || currentIndex >= 0 || itemCount <= 0) return null;
  return key === "Enter" || key === " " ? 0 : null;
}

export function readerMenuFocusIndex(
  key: string,
  currentIndex: number,
  itemCount: number,
  shiftKey = false,
  modified = false,
): number | null {
  if (itemCount <= 0) return null;
  if (modified || (shiftKey && key !== "Tab")) return null;
  if (key === "Home") return 0;
  if (key === "End") return itemCount - 1;
  if (key === "Tab" && currentIndex < 0) return shiftKey ? itemCount - 1 : 0;
  if (key !== "ArrowDown" && key !== "ArrowUp") return null;
  if (currentIndex < 0) return key === "ArrowDown" ? 0 : itemCount - 1;
  return (currentIndex + (key === "ArrowDown" ? 1 : -1) + itemCount) % itemCount;
}

export const READER_CONTEXT_MENU_KEY_EVENT = "quill-reader-context-menu-key";

export interface ReaderContextMenuKeyDetail {
  key: string;
  shiftKey: boolean;
  modified: boolean;
  handled: boolean;
}

export function forwardReaderContextMenuKey(event: KeyboardEvent): boolean {
  const detail: ReaderContextMenuKeyDetail = {
    key: event.key,
    shiftKey: event.shiftKey,
    modified: event.altKey || event.ctrlKey || event.metaKey,
    handled: false,
  };
  window.dispatchEvent(new CustomEvent<ReaderContextMenuKeyDetail>(
    READER_CONTEXT_MENU_KEY_EVENT,
    { detail },
  ));
  return detail.handled;
}

export function replaceDocumentSelection(doc: Document, range: Range): void {
  const selection = doc.getSelection?.();
  if (!selection) return;
  selection.removeAllRanges();
  selection.addRange(range.cloneRange());
}

export function expandRangeToWordBoundaries(range: Range, locale?: string): Range | null {
  if (range.collapsed || !/[\p{L}\p{M}\p{N}]/u.test(range.toString())) return null;
  const doc = range.startContainer.ownerDocument;
  if (!doc || doc !== range.endContainer.ownerDocument) return null;
  const startRoot = closestTextRunRoot(range.startContainer);
  const endRoot = closestTextRunRoot(range.endContainer);
  if (!startRoot || !endRoot) return null;
  const startRun = flattenTextRun(startRoot);
  const endRun = startRoot === endRoot ? startRun : flattenTextRun(endRoot);
  const startOffset = domPointToFlatOffset(startRun, range.startContainer, range.startOffset);
  const endOffset = domPointToFlatOffset(endRun, range.endContainer, range.endOffset);
  if (startOffset === null || endOffset === null) return null;

  const startSegment = segmentInteractionWords(startRun.text, locale).find(({ segment, index }) => (
    startOffset >= index && startOffset < index + segment.length
  ));
  const endSegment = segmentInteractionWords(endRun.text, locale).find(({ segment, index }) => (
    endOffset > index && endOffset <= index + segment.length
  ));
  const expanded = range.cloneRange();
  if (startSegment) {
    const point = domPointAtFlatOffset(startRun, startSegment.index, "start");
    if (point) expanded.setStart(point.node, point.offset);
  }
  if (endSegment) {
    const point = domPointAtFlatOffset(
      endRun,
      endSegment.index + endSegment.segment.length,
      "end",
    );
    if (point) expanded.setEnd(point.node, point.offset);
  }
  return expanded;
}

export function contextForRange(range: Range, fallback: string): string {
  let node: Node | null = range.commonAncestorContainer;
  if (node.nodeType !== Node.ELEMENT_NODE) node = node.parentElement;
  while (node && node.nodeType === Node.ELEMENT_NODE && !BLOCK_TAGS.has((node as Element).tagName)) {
    node = node.parentNode;
  }
  const context = (node as Element | null)?.textContent?.trim() || fallback.trim();
  if (context.length <= 800) return context;
  const selected = range.toString().trim();
  const selectedIndex = context.indexOf(selected);
  if (selectedIndex < 0) return context.slice(0, 800);
  const start = Math.max(0, selectedIndex - 300);
  return context.slice(start, Math.min(context.length, start + 800));
}

export function serializableRect(rect: DOMRect | DOMRectReadOnly): SerializableRect {
  return {
    left: rect.left,
    top: rect.top,
    right: rect.right,
    bottom: rect.bottom,
    width: rect.width,
    height: rect.height,
  };
}

export function viewportRectForRange(range: Range): SerializableRect {
  const rects = Array.from(range.getClientRects()).filter((rect) => rect.width > 0 && rect.height > 0);
  const fallback = range.getBoundingClientRect();
  const rect = rects.length > 0 ? {
    left: Math.min(...rects.map((value) => value.left)),
    top: Math.min(...rects.map((value) => value.top)),
    right: Math.max(...rects.map((value) => value.right)),
    bottom: Math.max(...rects.map((value) => value.bottom)),
  } : fallback;
  const frame = range.startContainer.ownerDocument?.defaultView?.frameElement as HTMLElement | null;
  const frameRect = frame?.getBoundingClientRect();
  const left = rect.left + (frameRect?.left ?? 0);
  const top = rect.top + (frameRect?.top ?? 0);
  return {
    left,
    top,
    right: rect.right + (frameRect?.left ?? 0),
    bottom: rect.bottom + (frameRect?.top ?? 0),
    width: rect.right - rect.left,
    height: rect.bottom - rect.top,
  };
}

export function isInteractiveReaderTarget(target: EventTarget | null): boolean {
  const node = target as Node | null;
  const element = node?.nodeType === 1 ? node as Element : node?.parentElement;
  return Boolean(element?.closest("a,button,input,textarea,select,option,[contenteditable='true'],[role='button']"));
}

type HighlightRegistry = {
  set(name: string, highlight: unknown): void;
  delete(name: string): boolean;
};

export function applyWordMarkHighlights(
  doc: Document,
  normalizedWords: Iterable<string>,
  name = "quill-word-marks",
  root?: Node,
  includeRange?: (word: string, range: Range) => boolean,
  css?: string,
): boolean {
  const words = new Set(Array.from(normalizedWords, (word) => normalizeInteractionText(word)).filter(Boolean));
  const view = doc.defaultView as (Window & typeof globalThis & {
    CSS?: typeof CSS & { highlights?: HighlightRegistry };
    Highlight?: new (...ranges: Range[]) => unknown;
  }) | null;
  const registry = view?.CSS?.highlights;
  if (!registry || !view?.Highlight) return false;
  if (words.size === 0) {
    registry.delete(name);
    return true;
  }

  const ranges: Range[] = [];
  const walker = doc.createTreeWalker(root ?? doc.body ?? doc.documentElement, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const parent = node.parentElement;
      if (!node.textContent?.trim() || parent?.closest("script,style,noscript,textarea,input,[contenteditable='true']")) {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  let node = walker.nextNode();
  while (node && ranges.length < 20_000) {
    const text = node.textContent ?? "";
    for (const segment of segmentInteractionWords(text, doc.documentElement.lang || undefined)) {
      if (!words.has(normalizeInteractionText(segment.segment))) continue;
      const range = doc.createRange();
      range.setStart(node, segment.index);
      range.setEnd(node, segment.index + segment.segment.length);
      const normalized = normalizeInteractionText(segment.segment);
      if (includeRange && !includeRange(normalized, range)) continue;
      ranges.push(range);
      if (ranges.length >= 20_000) break;
    }
    node = walker.nextNode();
  }

  registry.set(name, new view.Highlight(...ranges));
  const styleId = `quill-word-mark-style-${name}`;
  let style = doc.getElementById(styleId) as HTMLStyleElement | null;
  if (!style) {
    style = doc.createElement("style");
    style.id = styleId;
    (doc.head ?? doc.documentElement).appendChild(style);
  }
  style.textContent = `::highlight(${name}) { ${css ?? "background: rgba(163, 106, 49, 0.12); text-decoration: underline; text-decoration-color: rgba(141, 124, 101, 0.85); text-decoration-thickness: 1.5px; text-underline-offset: 0.14em;"} }`;
  return true;
}
