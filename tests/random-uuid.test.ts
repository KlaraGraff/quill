import assert from "node:assert/strict";
import test from "node:test";

import { createUuid } from "../src/utils/randomUuid.ts";

test("createUuid uses the native generator when available", () => {
  const source = {
    randomUUID: () => "native-id",
    getRandomValues: (bytes: Uint8Array) => bytes,
  } as unknown as Crypto;
  assert.equal(createUuid(source), "native-id");
});

test("createUuid generates a v4 UUID when randomUUID is unavailable", () => {
  const source = {
    getRandomValues: (bytes: Uint8Array) => {
      bytes.set(Array.from({ length: bytes.length }, (_, index) => index));
      return bytes;
    },
  } as unknown as Crypto;
  assert.equal(createUuid(source), "00010203-0405-4607-8809-0a0b0c0d0e0f");
});
