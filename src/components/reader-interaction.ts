export type InteractionKind = "word" | "phrase" | "passage";

export interface SerializableRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface ReaderInteraction {
  trigger: "word-click" | "selection-contextmenu";
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
    .trim()
    .replace(/^[^\p{L}\p{N}']+|[^\p{L}\p{N}']+$/gu, "")
    .toLocaleLowerCase();
}

export function classifySelection(value: string, locale?: string): InteractionKind {
  const text = value.trim();
  if (!text) return "passage";
  const words = lexicalSegments(text, locale);
  if (words.length === 1 && words[0].segment === text) return "word";
  if (words.length <= 5 && !/[.!?。！？；;:\n\r]/u.test(text)) return "phrase";
  return "passage";
}

function lexicalSegments(text: string, locale?: string): Array<{ segment: string; index: number }> {
  const Segmenter = (Intl as typeof Intl & {
    Segmenter?: new (
      locale?: string,
      options?: { granularity: "word" },
    ) => { segment(value: string): Iterable<{ segment: string; index: number; isWordLike?: boolean }> };
  }).Segmenter;
  if (Segmenter) {
    const segmenter = new Segmenter(locale, { granularity: "word" });
    return Array.from(segmenter.segment(text))
      .filter((part) => part.isWordLike)
      .map((part) => ({ segment: part.segment, index: part.index }));
  }
  return Array.from(text.matchAll(/[\p{L}\p{M}\p{N}]+(?:['’][\p{L}\p{M}\p{N}]+)*/gu))
    .map((match) => ({ segment: match[0], index: match.index ?? 0 }));
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
  const node = caret.startContainer;
  const text = node.textContent ?? "";
  const offset = Math.min(caret.startOffset, Math.max(0, text.length - 1));
  const segment = lexicalSegments(text, locale).find(({ segment, index }) => (
    offset >= index && offset < index + segment.length
  ));
  if (!segment) return null;
  const range = doc.createRange();
  range.setStart(node, segment.index);
  range.setEnd(node, segment.index + segment.segment.length);
  return range;
}

export function selectedRange(doc: Document): Range | null {
  const selection = doc.getSelection?.();
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  return range.toString().trim() ? range.cloneRange() : null;
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
  const rect = rects[0] ?? range.getBoundingClientRect();
  const frame = range.startContainer.ownerDocument?.defaultView?.frameElement as HTMLElement | null;
  const frameRect = frame?.getBoundingClientRect();
  const left = rect.left + (frameRect?.left ?? 0);
  const top = rect.top + (frameRect?.top ?? 0);
  return {
    left,
    top,
    right: rect.right + (frameRect?.left ?? 0),
    bottom: rect.bottom + (frameRect?.top ?? 0),
    width: rect.width,
    height: rect.height,
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
    for (const segment of lexicalSegments(text, doc.documentElement.lang || undefined)) {
      if (!words.has(normalizeInteractionText(segment.segment))) continue;
      const range = doc.createRange();
      range.setStart(node, segment.index);
      range.setEnd(node, segment.index + segment.segment.length);
      ranges.push(range);
      if (ranges.length >= 20_000) break;
    }
    node = walker.nextNode();
  }

  registry.set(name, new view.Highlight(...ranges));
  if (!doc.getElementById("quill-word-mark-style")) {
    const style = doc.createElement("style");
    style.id = "quill-word-mark-style";
    style.textContent = `::highlight(${name}) { background: rgba(163, 106, 49, 0.12); text-decoration: underline; text-decoration-color: rgba(141, 124, 101, 0.85); text-decoration-thickness: 1.5px; text-underline-offset: 0.14em; }`;
    (doc.head ?? doc.documentElement).appendChild(style);
  }
  return true;
}
