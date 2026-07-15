import assert from "node:assert/strict";
import test from "node:test";

import {
  selectChapterLevelItems,
  type FoliateTocItem,
} from "../src/pages/reader/chapter-pagination.ts";

test("uses multiple top-level TOC items as chapter starts", () => {
  const toc: FoliateTocItem[] = [{ href: "one" }, { href: "two" }];
  assert.deepEqual(selectChapterLevelItems(toc, new Set(toc)), toc);
});

test("unwraps one book-level TOC item with multiple chapter children", () => {
  const chapters: FoliateTocItem[] = [
    { href: "chapter-1", subitems: [{ href: "section-1" }] },
    { href: "chapter-2" },
  ];
  const root: FoliateTocItem = { href: "book", subitems: chapters };
  const valid = new Set<FoliateTocItem>([
    root,
    ...chapters,
    chapters[0].subitems?.[0] as FoliateTocItem,
  ]);
  assert.deepEqual(selectChapterLevelItems([root], valid), chapters);
});

test("ignores invalid and deeper section targets", () => {
  const section: FoliateTocItem = { href: "section" };
  const chapter: FoliateTocItem = { href: "chapter", subitems: [section] };
  const invalidRoot: FoliateTocItem = { subitems: [chapter] };
  assert.deepEqual(selectChapterLevelItems([invalidRoot], new Set([chapter, section])), [chapter]);
});

test("selects chapters recursively inside multiple labelled parts but not sections", () => {
  const sectionOne: FoliateTocItem = { label: "Section 1", href: "part-1-section-1" };
  const chapterOne: FoliateTocItem = {
    label: "Chapter 1",
    href: "part-1-chapter-1",
    subitems: [sectionOne],
  };
  const chapterTwo: FoliateTocItem = { label: "The Road", href: "part-1-chapter-2" };
  const partOne: FoliateTocItem = {
    label: "Part I",
    href: "part-1",
    subitems: [chapterOne, chapterTwo],
  };
  const chapterThree: FoliateTocItem = { label: "Chapter 3", href: "part-2-chapter-3" };
  const partTwo: FoliateTocItem = {
    label: "Part II",
    href: "part-2",
    subitems: [chapterThree],
  };
  const valid = new Set([partOne, chapterOne, sectionOne, chapterTwo, partTwo, chapterThree]);

  assert.deepEqual(
    selectChapterLevelItems([partOne, partTwo], valid),
    [partOne, chapterOne, chapterTwo, partTwo, chapterThree],
  );
});

test("keeps labelled chapter roots while excluding their nested sections", () => {
  const section: FoliateTocItem = { label: "SECTION 2", href: "section-2" };
  const first: FoliateTocItem = {
    label: "CHAPTER ONE",
    href: "chapter-1",
    subitems: [section],
  };
  const second: FoliateTocItem = { label: "CHAPTER TWO", href: "chapter-2" };

  assert.deepEqual(
    selectChapterLevelItems([first, second], new Set([first, section, second])),
    [first, second],
  );
});

test("uses a labelled structural wrapper even when only its child targets resolve", () => {
  const first: FoliateTocItem = { label: "Chapter 1", href: "chapter-1" };
  const second: FoliateTocItem = { label: "The Journey", href: "chapter-2" };
  const unresolvedPart: FoliateTocItem = {
    label: "Part One",
    subitems: [first, second],
  };

  assert.deepEqual(
    selectChapterLevelItems([unresolvedPart], new Set([first, second])),
    [first, second],
  );
});
