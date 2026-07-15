export const CHAPTER_START_ATTRIBUTE = "data-quill-chapter-start";

export interface FoliateTocItem {
  label?: unknown;
  href?: unknown;
  subitems?: readonly FoliateTocItem[] | null;
}

interface FoliateResolvedTarget {
  index: number;
  anchor?: unknown;
}

export interface FoliateChapterBook {
  toc?: readonly FoliateTocItem[] | null;
  resolveHref?: (href: string) => unknown | Promise<unknown>;
}

interface TocEntry {
  item: FoliateTocItem;
  depth: number;
  order: number;
}

interface ResolvedTocEntry extends TocEntry {
  href: string;
  target: FoliateResolvedTarget;
}

type TocUnitKind = "structural" | "chapter" | "section" | "unknown";

const ELEMENT_NODE = 1;
const TEXT_NODE = 3;
const DOCUMENT_NODE = 9;
const SHOW_ELEMENT_AND_TEXT = 0x5;
const START_TO_START = 0;

const IGNORED_CONTENT_TAGS = new Set([
  "HEAD",
  "LINK",
  "META",
  "NOSCRIPT",
  "SCRIPT",
  "STYLE",
  "TEMPLATE",
  "TITLE",
]);

const MEDIA_CONTENT_TAGS = new Set([
  "AUDIO",
  "CANVAS",
  "EMBED",
  "HR",
  "IFRAME",
  "IMG",
  "MATH",
  "OBJECT",
  "SVG",
  "VIDEO",
]);

const BLOCK_TAGS = new Set([
  "ADDRESS",
  "ARTICLE",
  "ASIDE",
  "BLOCKQUOTE",
  "DD",
  "DETAILS",
  "DIALOG",
  "DIV",
  "DL",
  "DT",
  "FIELDSET",
  "FIGCAPTION",
  "FIGURE",
  "FOOTER",
  "FORM",
  "H1",
  "H2",
  "H3",
  "H4",
  "H5",
  "H6",
  "HEADER",
  "HGROUP",
  "HR",
  "LI",
  "MAIN",
  "NAV",
  "OL",
  "P",
  "PRE",
  "SECTION",
  "TABLE",
  "TBODY",
  "TD",
  "TFOOT",
  "TH",
  "THEAD",
  "TR",
  "UL",
]);

function normalizedTagName(element: Element): string {
  return (element.localName || element.tagName).toUpperCase();
}

function flattenToc(
  items: readonly FoliateTocItem[],
  depth = 0,
  result: TocEntry[] = [],
  seen: Set<FoliateTocItem> = new Set(),
): TocEntry[] {
  for (const item of items) {
    if (!item || typeof item !== "object" || seen.has(item)) continue;
    seen.add(item);
    result.push({ item, depth, order: result.length });
    if (Array.isArray(item.subitems)) {
      flattenToc(item.subitems, depth + 1, result, seen);
    }
  }
  return result;
}

function tocLabel(item: FoliateTocItem): string {
  return typeof item.label === "string" ? item.label.trim() : "";
}

function tocUnitKind(item: FoliateTocItem): TocUnitKind {
  const label = tocLabel(item)
    .normalize("NFKC")
    .replace(/[\s.:–—-]+/gu, " ")
    .trim();
  if (!label) return "unknown";

  if (/^(?:section|subsection)\b/iu.test(label) || /^第[\d零〇一二三四五六七八九十百千万两壹贰叁肆伍陆柒捌玖拾佰仟]+节/u.test(label)) {
    return "section";
  }
  if (/^(?:volume|book|part|division|act)\b/iu.test(label) || /^第[\d零〇一二三四五六七八九十百千万两壹贰叁肆伍陆柒捌玖拾佰仟]+[卷部篇集]/u.test(label)) {
    return "structural";
  }
  if (/^(?:chapter|scene|prologue|epilogue)\b/iu.test(label) || /^第[\d零〇一二三四五六七八九十百千万两壹贰叁肆伍陆柒捌玖拾佰仟]+[章回]/u.test(label)) {
    return "chapter";
  }
  return "unknown";
}

/** Selects semantic reading units without promoting nested sections. */
export function selectChapterLevelItems(
  toc: readonly FoliateTocItem[],
  validItems: ReadonlySet<FoliateTocItem>,
): FoliateTocItem[] {
  const entries = flattenToc(toc);
  const validEntries = entries.filter(({ item }) => validItems.has(item));
  if (validEntries.length === 0) return [];

  const selected = new Set<FoliateTocItem>();
  const add = (item: FoliateTocItem) => {
    if (validItems.has(item) && tocUnitKind(item) !== "section") selected.add(item);
  };

  // Labels are the strongest signal. Include structural reading units and
  // chapters at any nesting depth, while explicitly excluding sections.
  for (const { item } of validEntries) {
    const kind = tocUnitKind(item);
    if (kind === "structural" || kind === "chapter") add(item);
  }

  // Literary chapter titles often do not contain "Chapter". Direct children
  // of a labelled Book/Part/Act are reading units unless their own label
  // explicitly identifies a section.
  const addStructuralChildren = (item: FoliateTocItem) => {
    for (const child of item.subitems ?? []) {
      const kind = tocUnitKind(child);
      if (kind === "section") continue;
      add(child);
      if (kind === "structural") addStructuralChildren(child);
    }
  };
  for (const { item } of entries) {
    if (tocUnitKind(item) === "structural") addStructuralChildren(item);
  }

  // A single valid or invalid book-title wrapper with several children is a
  // common EPUB shape. Preserve the existing conservative unwrap heuristic.
  if (toc.length === 1) {
    const directChildren = (toc[0].subitems ?? [])
      .filter((item) => validItems.has(item) && tocUnitKind(item) !== "section");
    if (directChildren.length >= 2) directChildren.forEach(add);
  }

  if (selected.size > 0) {
    return entries
      .filter(({ item }) => selected.has(item))
      .sort((left, right) => left.order - right.order)
      .map(({ item }) => item);
  }

  const shallowestDepth = Math.min(...validEntries.map(({ depth }) => depth));
  return validEntries
    .filter(({ depth, item }) => depth === shallowestDepth && tocUnitKind(item) !== "section")
    .sort((left, right) => left.order - right.order)
    .map(({ item }) => item);
}

function hrefForItem(item: FoliateTocItem): string | null {
  return typeof item.href === "string"
    && item.href.length > 0
    && item.href !== "null"
    ? item.href
    : null;
}

function asResolvedTarget(value: unknown): FoliateResolvedTarget | null {
  if (!value || typeof value !== "object") return null;
  const index = (value as { index?: unknown }).index;
  if (typeof index !== "number" || !Number.isInteger(index) || index < 0) return null;
  return {
    index,
    anchor: (value as { anchor?: unknown }).anchor,
  };
}

function isNode(value: unknown): value is Node {
  return Boolean(
    value
    && typeof value === "object"
    && typeof (value as { nodeType?: unknown }).nodeType === "number",
  );
}

function isRange(value: unknown): value is Range {
  return Boolean(
    value
    && typeof value === "object"
    && isNode((value as { startContainer?: unknown }).startContainer)
    && typeof (value as { startOffset?: unknown }).startOffset === "number",
  );
}

function resolveAnchor(anchor: unknown, doc: Document): unknown {
  if (typeof anchor !== "function") return anchor ?? 0;
  try {
    return (anchor as (document: Document) => unknown)(doc);
  } catch {
    return null;
  }
}

function rangeForAnchor(doc: Document, anchor: unknown): Range | null {
  if (typeof anchor === "number") {
    if (anchor !== 0) return null;
    const root = doc.body ?? doc.documentElement;
    if (!root) return null;
    const range = doc.createRange();
    range.setStart(root, 0);
    range.collapse(true);
    return range;
  }

  if (isRange(anchor)) {
    if (anchor.startContainer.ownerDocument !== doc) return null;
    const range = doc.createRange();
    try {
      range.setStart(anchor.startContainer, anchor.startOffset);
      range.collapse(true);
      return range;
    } catch {
      return null;
    }
  }

  if (!isNode(anchor)) return null;
  const ownerDocument = anchor.nodeType === DOCUMENT_NODE
    ? anchor as Document
    : anchor.ownerDocument;
  if (ownerDocument !== doc) return null;

  const range = doc.createRange();
  try {
    if (anchor.nodeType === DOCUMENT_NODE) {
      const root = doc.body ?? doc.documentElement;
      if (!root) return null;
      range.setStart(root, 0);
    } else if (anchor.parentNode) {
      range.setStartBefore(anchor);
    } else {
      range.setStart(anchor, 0);
    }
    range.collapse(true);
    return range;
  } catch {
    return null;
  }
}

function elementAtAnchor(doc: Document, anchor: unknown): Element | null {
  if (isNode(anchor)) {
    if (anchor.nodeType === ELEMENT_NODE) return anchor as Element;
    return anchor.parentElement;
  }
  if (!isRange(anchor) || anchor.startContainer.ownerDocument !== doc) return null;

  const container = anchor.startContainer;
  if (container.nodeType === ELEMENT_NODE) {
    const child = container.childNodes.item(anchor.startOffset);
    if (child?.nodeType === ELEMENT_NODE) return child as Element;
    return container as Element;
  }
  return container.parentElement;
}

function isIgnoredContent(node: Node, root: Element): boolean {
  let element = node.nodeType === ELEMENT_NODE ? node as Element : node.parentElement;
  while (element && element !== root) {
    if (
      IGNORED_CONTENT_TAGS.has(normalizedTagName(element))
      || element.hasAttribute("hidden")
      || element.getAttribute("aria-hidden") === "true"
    ) {
      return true;
    }
    element = element.parentElement;
  }
  return false;
}

function meaningfulRange(doc: Document, startAt?: Range): Range | null {
  const root = doc.body ?? doc.documentElement;
  if (!root) return null;
  const iterator = doc.createNodeIterator(root, SHOW_ELEMENT_AND_TEXT);
  let node: Node | null;

  while ((node = iterator.nextNode())) {
    if (isIgnoredContent(node, root)) continue;
    const range = doc.createRange();

    if (node.nodeType === TEXT_NODE) {
      const text = node.nodeValue ?? "";
      const offset = text.search(/\S/u);
      if (offset < 0) continue;
      range.setStart(node, offset);
      range.collapse(true);
    } else if (
      node.nodeType === ELEMENT_NODE
      && MEDIA_CONTENT_TAGS.has(normalizedTagName(node as Element))
    ) {
      try {
        range.setStartBefore(node);
        range.collapse(true);
      } catch {
        continue;
      }
    } else {
      continue;
    }

    if (!startAt || range.compareBoundaryPoints(START_TO_START, startAt) >= 0) {
      return range;
    }
  }
  return null;
}

function closestBlock(element: Element | null, doc: Document): Element | null {
  const root = doc.body ?? doc.documentElement;
  let current = element;
  while (current && current !== root && current !== doc.documentElement) {
    if (BLOCK_TAGS.has(normalizedTagName(current))) return current;
    current = current.parentElement;
  }
  return null;
}

function blockForAnchor(doc: Document, anchor: unknown, anchorRange: Range): Element | null {
  const directElement = elementAtAnchor(doc, anchor);
  const directBlock = closestBlock(directElement, doc);
  const isEmptyInlineAnchor = directElement
    && !BLOCK_TAGS.has(normalizedTagName(directElement))
    && !(directElement.textContent ?? "").trim();
  if (directBlock && !isEmptyInlineAnchor) return directBlock;

  // EPUB 2 books often place an empty named anchor immediately before the
  // actual heading. In that case, mark the following content block rather
  // than the inline anchor, because CSS fragmentation ignores inline boxes.
  const nextContent = meaningfulRange(doc, anchorRange);
  return closestBlock(nextContent?.startContainer.parentElement ?? null, doc) ?? directBlock;
}

function hasMeaningfulContentBefore(doc: Document, element: Element): boolean {
  const firstContent = meaningfulRange(doc);
  if (!firstContent) return false;

  const elementStart = doc.createRange();
  try {
    elementStart.setStartBefore(element);
    elementStart.collapse(true);
  } catch {
    return false;
  }
  return firstContent.compareBoundaryPoints(START_TO_START, elementStart) < 0;
}

function reliableFallbackHeadings(doc: Document): Element[] {
  const root = doc.body ?? doc.documentElement;
  if (!root) return [];
  const candidates = Array.from(doc.querySelectorAll(
    "body > h1, section > h1, section > header > h1",
  ));
  return candidates.filter((element) => (
    !isIgnoredContent(element, root)
    && tocUnitKind({ label: element.textContent ?? "" }) !== "section"
    && hasMeaningfulContentBefore(doc, element)
  ));
}

async function resolveTocEntries(book: FoliateChapterBook): Promise<ResolvedTocEntry[]> {
  if (!Array.isArray(book.toc) || typeof book.resolveHref !== "function") return [];
  const entries = flattenToc(book.toc);
  const resolved = await Promise.all(entries.map(async (entry) => {
    const href = hrefForItem(entry.item);
    if (!href) return null;
    try {
      const target = asResolvedTarget(await book.resolveHref?.(href));
      return target ? { ...entry, href, target } : null;
    } catch {
      return null;
    }
  }));
  return resolved.filter((entry): entry is ResolvedTocEntry => entry !== null);
}

/**
 * Builds an immutable TOC-derived marker function. Call the returned function
 * synchronously from Foliate's `load` event before paginator measurement.
 */
export async function createChapterPaginationMarker(
  book: FoliateChapterBook,
): Promise<(doc: Document, index: number) => number> {
  const resolvedEntries = await resolveTocEntries(book);
  const validItems = new Set(resolvedEntries.map(({ item }) => item));
  const selectedItems = new Set(selectChapterLevelItems(book.toc ?? [], validItems));
  const targetsByIndex = new Map<number, unknown[]>();
  const duplicateTargets = new Set<string>();

  for (const { item, href, target } of resolvedEntries) {
    if (!selectedItems.has(item)) continue;
    const duplicateKey = `${target.index}\u0000${href}`;
    if (duplicateTargets.has(duplicateKey)) continue;
    duplicateTargets.add(duplicateKey);
    const targets = targetsByIndex.get(target.index) ?? [];
    targets.push(target.anchor);
    targetsByIndex.set(target.index, targets);
  }

  return (doc: Document, index: number): number => {
    for (const element of doc.querySelectorAll(`[${CHAPTER_START_ATTRIBUTE}]`)) {
      element.removeAttribute(CHAPTER_START_ATTRIBUTE);
    }

    const marked = new Set<Element>();
    for (const unresolvedAnchor of targetsByIndex.get(index) ?? []) {
      const anchor = resolveAnchor(unresolvedAnchor, doc);
      const anchorRange = rangeForAnchor(doc, anchor);
      if (!anchorRange) continue;
      const block = blockForAnchor(doc, anchor, anchorRange);
      if (!block || marked.has(block) || !hasMeaningfulContentBefore(doc, block)) continue;
      block.setAttribute(CHAPTER_START_ATTRIBUTE, "");
      marked.add(block);
    }

    // Supplement incomplete or missing TOCs with only high-confidence h1
    // structures. Generic h2/h3 headings are deliberately excluded.
    for (const heading of reliableFallbackHeadings(doc)) {
      const alreadyCovered = Array.from(marked).some((element) => (
        element === heading || element.contains(heading) || heading.contains(element)
      ));
      if (alreadyCovered) continue;
      heading.setAttribute(CHAPTER_START_ATTRIBUTE, "");
      marked.add(heading);
    }
    return marked.size;
  };
}
