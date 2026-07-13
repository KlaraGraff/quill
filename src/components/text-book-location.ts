export interface TextBookChapter {
  title: string;
  paragraphs: string[];
}

export interface TextBookDocument {
  version: number;
  source_sha256: string | null;
  chapters: TextBookChapter[];
}

export interface TextLocation {
  startChapter: number;
  startParagraph: number;
  startOffset: number;
  endChapter: number;
  endParagraph: number;
  endOffset: number;
}

export function textLocation(
  startChapter: number,
  startParagraph: number,
  startOffset: number,
  endChapter = startChapter,
  endParagraph = startParagraph,
  endOffset = startOffset,
): string {
  return `textloc:${startChapter}:${startParagraph}:${startOffset}:${endChapter}:${endParagraph}:${endOffset}`;
}

export function parseTextLocation(value: string | null | undefined): TextLocation | null {
  if (!value?.startsWith("textloc:")) return null;
  const parts = value.slice("textloc:".length).split(":").map(Number);
  if (parts.length !== 6 || parts.some((part) => !Number.isSafeInteger(part) || part < 0)) return null;
  return {
    startChapter: parts[0],
    startParagraph: parts[1],
    startOffset: parts[2],
    endChapter: parts[3],
    endParagraph: parts[4],
    endOffset: parts[5],
  };
}
