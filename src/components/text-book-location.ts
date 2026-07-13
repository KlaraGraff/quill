export type TextBookBlockKind = "heading" | "paragraph";

export interface TextBookSourceSpan {
  rendered_start: number;
  source_start: number;
  length: number;
}

export interface TextBookBlock {
  kind: TextBookBlockKind;
  text: string;
  source_start: number;
  source_end: number;
  source_spans: TextBookSourceSpan[];
  depth?: number;
}

export interface TextBookChunk {
  blocks: TextBookBlock[];
}

export interface TextBookTocEntry {
  title: string;
  depth: number;
  source_offset: number;
}

export interface TextBookDocument {
  version: number;
  source_sha256: string | null;
  coordinate_space: "normalized_utf16";
  chunks: TextBookChunk[];
  toc: TextBookTocEntry[];
  legacy_locations: number[][];
}

export interface AbsoluteTextLocation {
  version: 2;
  start: number;
  end: number;
}

export interface LegacyTextLocation {
  version: 1;
  startChapter: number;
  startParagraph: number;
  startOffset: number;
  endChapter: number;
  endParagraph: number;
  endOffset: number;
}

export type ParsedTextLocation = AbsoluteTextLocation | LegacyTextLocation;

export function textLocation(start: number, end = start): string {
  return `textloc:v2:${start}:${end}`;
}

export function parseTextLocation(value: string | null | undefined): ParsedTextLocation | null {
  if (!value?.startsWith("textloc:")) return null;
  if (value.startsWith("textloc:v2:")) {
    const parts = value.slice("textloc:v2:".length).split(":").map(Number);
    if (parts.length !== 2 || parts.some((part) => !Number.isSafeInteger(part) || part < 0)) return null;
    return { version: 2, start: parts[0], end: parts[1] };
  }

  const parts = value.slice("textloc:".length).split(":").map(Number);
  if (parts.length !== 6 || parts.some((part) => !Number.isSafeInteger(part) || part < 0)) return null;
  return {
    version: 1,
    startChapter: parts[0],
    startParagraph: parts[1],
    startOffset: parts[2],
    endChapter: parts[3],
    endParagraph: parts[4],
    endOffset: parts[5],
  };
}

export function resolveTextLocation(
  value: string | null | undefined,
  document: TextBookDocument,
): AbsoluteTextLocation | null {
  const parsed = parseTextLocation(value);
  if (!parsed) return null;
  if (parsed.version === 2) return parsed;

  const startBase = document.legacy_locations[parsed.startChapter]?.[parsed.startParagraph];
  const endBase = document.legacy_locations[parsed.endChapter]?.[parsed.endParagraph];
  if (!Number.isSafeInteger(startBase) || !Number.isSafeInteger(endBase)) return null;
  const start = startBase + parsed.startOffset;
  const end = endBase + parsed.endOffset;
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end)) return null;
  return { version: 2, start, end };
}
