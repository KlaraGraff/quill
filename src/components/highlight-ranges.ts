import { parseTextLocation, textLocation } from "./text-book-location";

export interface CfiModule {
  parse(cfi: string): unknown;
  collapse(cfi: string | unknown, toEnd?: boolean): unknown;
  compare(left: string | unknown, right: string | unknown): number;
}

let cfiModulePromise: Promise<CfiModule> | null = null;
const CFI_MODULE_URL = "/foliate-js/epubcfi.js";

function loadCfiModule() {
  cfiModulePromise ??= import(/* @vite-ignore */ CFI_MODULE_URL) as Promise<CfiModule>;
  return cfiModulePromise;
}

export interface StoredHighlightLocation {
  id: string;
  cfi_range: string;
  color: string;
  note: string | null;
  text_content: string | null;
  created_at?: number;
}

export interface HighlightAddition {
  cfiRange: string;
  color: string;
  note: string | null;
  textContent: string | null;
  createdAt: number | null;
}

export interface HighlightMutationPlan {
  fullyHighlighted: boolean;
  removeIds: string[];
  additions: HighlightAddition[];
}

export function isManualSelectionFullyHighlighted(
  selectionLocation: string,
  highlights: StoredHighlightLocation[],
  source: "text" | "foliate",
) {
  if (source === "text") {
    return Promise.resolve(
      planTextHighlightMutation(selectionLocation, highlights)?.fullyHighlighted ?? false,
    );
  }
  return planCfiHighlightMutation(selectionLocation, highlights)
    .then((plan) => plan?.fullyHighlighted ?? false);
}

interface PositionedHighlight<Position> extends StoredHighlightLocation {
  start: Position;
  end: Position;
}

interface PlannedRange<Position> {
  start: Position;
  end: Position;
  color: string;
  note: string | null;
  textContent: string | null;
  createdAt: number | null;
}

type Compare<Position> = (left: Position, right: Position) => number;

function maximum<Position>(left: Position, right: Position, compare: Compare<Position>) {
  return compare(left, right) >= 0 ? left : right;
}

function minimum<Position>(left: Position, right: Position, compare: Compare<Position>) {
  return compare(left, right) <= 0 ? left : right;
}

function overlaps<Position>(
  left: { start: Position; end: Position },
  right: { start: Position; end: Position },
  compare: Compare<Position>,
) {
  return compare(left.start, right.end) < 0 && compare(left.end, right.start) > 0;
}

function touchesOrOverlaps<Position>(
  left: { start: Position; end: Position },
  right: { start: Position; end: Position },
  compare: Compare<Position>,
) {
  return compare(left.start, right.end) <= 0 && compare(left.end, right.start) >= 0;
}

export function isRangeFullyHighlighted<Position>(
  selection: { start: Position; end: Position },
  highlights: Array<{ start: Position; end: Position }>,
  compare: Compare<Position>,
) {
  if (compare(selection.start, selection.end) >= 0) return false;
  const intersecting = highlights
    .filter((highlight) => overlaps(selection, highlight, compare))
    .sort((left, right) => compare(left.start, right.start) || compare(left.end, right.end));
  let cursor = selection.start;
  for (const highlight of intersecting) {
    if (compare(highlight.start, cursor) > 0) return false;
    cursor = maximum(cursor, highlight.end, compare);
    if (compare(cursor, selection.end) >= 0) return true;
  }
  return false;
}

function planRanges<Position>(
  selection: { start: Position; end: Position },
  highlights: PositionedHighlight<Position>[],
  compare: Compare<Position>,
  targetColor: string,
  selectedText: string,
) {
  const fullyHighlighted = isRangeFullyHighlighted(selection, highlights, compare);
  const removeIds = new Set<string>();
  const additions: PlannedRange<Position>[] = [];

  if (fullyHighlighted) {
    for (const highlight of highlights) {
      if (!overlaps(selection, highlight, compare)) continue;
      removeIds.add(highlight.id);
      if (compare(highlight.start, selection.start) < 0) {
        additions.push({
          start: highlight.start,
          end: minimum(highlight.end, selection.start, compare),
          color: highlight.color,
          note: highlight.note,
          textContent: null,
          createdAt: highlight.created_at ?? null,
        });
      }
      if (compare(highlight.end, selection.end) > 0) {
        additions.push({
          start: maximum(highlight.start, selection.end, compare),
          end: highlight.end,
          color: highlight.color,
          note: highlight.note,
          textContent: null,
          createdAt: highlight.created_at ?? null,
        });
      }
    }
    return { fullyHighlighted, removeIds: [...removeIds], additions };
  }

  let mergedStart = selection.start;
  let mergedEnd = selection.end;
  let changed = true;
  while (changed) {
    changed = false;
    for (const highlight of highlights) {
      if (highlight.color !== targetColor || removeIds.has(highlight.id)) continue;
      // Highlight ranges are only valid within one EPUB/PDF section. Foliate
      // CFI comparison can order separate sections, but they must not be
      // serialized back into one cross-document range.
      if (typeof mergedStart === "string"
        && typeof highlight.start === "string"
        && cfiSectionKey(mergedStart) !== cfiSectionKey(highlight.start)) continue;
      if (!touchesOrOverlaps(
        { start: mergedStart, end: mergedEnd },
        highlight,
        compare,
      )) continue;
      removeIds.add(highlight.id);
      const nextStart = minimum(mergedStart, highlight.start, compare);
      const nextEnd = maximum(mergedEnd, highlight.end, compare);
      changed = compare(nextStart, mergedStart) !== 0 || compare(nextEnd, mergedEnd) !== 0;
      mergedStart = nextStart;
      mergedEnd = nextEnd;
    }
  }

  // A new manual highlight owns its selected range. Trim any differently
  // coloured ranges underneath it, while preserving their unaffected sides.
  // Same-colour ranges were consumed by the fixed-point merge above.
  for (const highlight of highlights) {
    if (removeIds.has(highlight.id)
      || !overlaps({ start: mergedStart, end: mergedEnd }, highlight, compare)) continue;
    removeIds.add(highlight.id);
    if (compare(highlight.start, mergedStart) < 0) {
      additions.push({
        start: highlight.start,
        end: minimum(highlight.end, mergedStart, compare),
        color: highlight.color,
        note: highlight.note,
        textContent: null,
        createdAt: highlight.created_at ?? null,
      });
    }
    if (compare(highlight.end, mergedEnd) > 0) {
      additions.push({
        start: maximum(highlight.start, mergedEnd, compare),
        end: highlight.end,
        color: highlight.color,
        note: highlight.note,
        textContent: null,
        createdAt: highlight.created_at ?? null,
      });
    }
  }

  additions.push({
    start: mergedStart,
    end: mergedEnd,
    color: targetColor,
    note: null,
    textContent: removeIds.size === 0 ? selectedText : null,
    createdAt: null,
  });
  return { fullyHighlighted, removeIds: [...removeIds], additions };
}

function createLocationPlan<Position>(
  selection: { start: Position; end: Position },
  highlights: PositionedHighlight<Position>[],
  compare: Compare<Position>,
  serialize: (start: Position, end: Position) => string,
  targetColor: string,
  selectedText: string,
): HighlightMutationPlan {
  const plan = planRanges(selection, highlights, compare, targetColor, selectedText);
  return {
    fullyHighlighted: plan.fullyHighlighted,
    removeIds: plan.removeIds,
    additions: plan.additions
      .filter((addition) => compare(addition.start, addition.end) < 0)
      .map((addition) => ({
        cfiRange: serialize(addition.start, addition.end),
        color: addition.color,
        note: addition.note,
        textContent: addition.textContent,
        createdAt: addition.createdAt,
      })),
  };
}

function textInterval(location: string) {
  const parsed = parseTextLocation(location);
  if (!parsed || parsed.version !== 2 || parsed.end <= parsed.start) return null;
  return { start: parsed.start, end: parsed.end };
}

export function planTextHighlightMutation(
  selectionLocation: string,
  highlights: StoredHighlightLocation[],
  targetColor = "yellow",
  selectedText = "",
): HighlightMutationPlan | null {
  const selection = textInterval(selectionLocation);
  if (!selection) return null;
  const positioned = highlights.flatMap((highlight) => {
    const interval = textInterval(highlight.cfi_range);
    return interval ? [{ ...highlight, ...interval }] : [];
  });
  return createLocationPlan(
    selection,
    positioned,
    (left, right) => left - right,
    textLocation,
    targetColor,
    selectedText,
  );
}

interface CfiPart {
  index: number;
  id?: string;
  offset?: number;
  temporal?: number;
  spatial?: number[];
  text?: string[];
  side?: string;
}

type CfiPath = CfiPart[][];

function escapeCfi(value: string) {
  return value.replace(/[\^[\](),;=]/g, "^$&");
}

function cfiPartToString(part: CfiPart) {
  const side = part.side ? `;s=${part.side}` : "";
  return `/${part.index}`
    + (part.id ? `[${escapeCfi(part.id)}${side}]` : "")
    + (part.offset != null && part.index % 2 ? `:${part.offset}` : "")
    + (part.temporal ? `~${part.temporal}` : "")
    + (part.spatial ? `@${part.spatial.join(":")}` : "")
    + (part.text || (!part.id && part.side)
      ? `[${part.text?.map(escapeCfi).join(",") ?? ""}${side}]`
      : "");
}

function cfiPathToString(path: CfiPath) {
  return path.map((parts) => parts.map(cfiPartToString).join("")).join("!");
}

function cfiSectionKey(location: string) {
  const separator = location.indexOf("!");
  return separator >= 0 ? location.slice(0, separator) : location;
}

function buildCfiRange(start: string, end: string, cfi: CfiModule) {
  const from = cfi.parse(start) as CfiPath;
  const to = cfi.parse(end) as CfiPath;
  const localFrom = from[from.length - 1] ?? [];
  const localTo = to[to.length - 1] ?? [];
  const parent: CfiPart[] = [];
  const localStart: CfiPart[] = [];
  const localEnd: CfiPart[] = [];
  let commonParent = true;
  const length = Math.max(localFrom.length, localTo.length);
  for (let index = 0; index < length; index += 1) {
    const left = localFrom[index];
    const right = localTo[index];
    commonParent = commonParent
      && left?.index === right?.index
      && !left?.offset
      && !right?.offset;
    if (commonParent && left) parent.push(left);
    else {
      if (left) localStart.push(left);
      if (right) localEnd.push(right);
    }
  }
  const parentPath = from.slice(0, -1).concat([parent]);
  return `epubcfi(${cfiPathToString(parentPath)},${cfiPathToString([localStart])},${cfiPathToString([localEnd])})`;
}

function cfiInterval(location: string, cfi: CfiModule) {
  try {
    const start = cfi.collapse(location) as string;
    const end = cfi.collapse(location, true) as string;
    if (cfi.compare(start, end) >= 0) return null;
    return { start, end };
  } catch {
    return null;
  }
}

export async function planCfiHighlightMutation(
  selectionLocation: string,
  highlights: StoredHighlightLocation[],
  targetColor = "yellow",
  selectedText = "",
  cfiModule?: CfiModule,
): Promise<HighlightMutationPlan | null> {
  const cfi = cfiModule ?? await loadCfiModule();
  const selection = cfiInterval(selectionLocation, cfi);
  if (!selection) return null;
  const positioned = highlights.flatMap((highlight) => {
    const interval = cfiInterval(highlight.cfi_range, cfi);
    return interval ? [{ ...highlight, ...interval }] : [];
  });
  return createLocationPlan(
    selection,
    positioned,
    (left, right) => cfi.compare(left, right),
    (start, end) => buildCfiRange(start, end, cfi),
    targetColor,
    selectedText,
  );
}
