import assert from "node:assert/strict";
import test from "node:test";

import {
  normalizeInteractionText,
  segmentInteractionWords,
} from "../src/components/reader-interaction.ts";

const words = (value: string, locale = "en") => (
  segmentInteractionWords(value, locale).map(({ segment }) => segment)
);

test("keeps apostrophes and hyphens inside interaction words", () => {
  assert.deepEqual(words("don't teacher's well-known"), ["don't", "teacher's", "well-known"]);
});

test("normalizes decomposed accents to NFC", () => {
  assert.equal(normalizeInteractionText("  Cafe\u0301!  "), "cafe\u0301".normalize("NFC"));
});

test("segments CJK without dropping characters", () => {
  assert.equal(words("\u4f60\u597d\u4e16\u754c", "zh").join(""), "\u4f60\u597d\u4e16\u754c");
});

test("rejects punctuation and whitespace", () => {
  assert.deepEqual(words("  ... -- !  "), []);
  assert.equal(normalizeInteractionText("  ... -- !  "), "");
});
