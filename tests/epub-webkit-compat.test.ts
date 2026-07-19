import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync } from "node:zlib";

import {
  groupByMap,
  groupByObject,
} from "../public/foliate-js/epub.js";

const NativeCompressionStream = globalThis.CompressionStream;
const NativeDecompressionStream = globalThis.DecompressionStream;

const crc32 = (bytes: Uint8Array) => {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
};

const makeDeflatedZip = (name: string, contents: string) => {
  const encoder = new TextEncoder();
  const filename = encoder.encode(name);
  const data = encoder.encode(contents);
  const compressed = new Uint8Array(deflateRawSync(data));
  const checksum = crc32(data);
  const localHeaderSize = 30 + filename.length;
  const centralHeaderSize = 46 + filename.length;
  const bytes = new Uint8Array(localHeaderSize + compressed.length + centralHeaderSize + 22);
  const view = new DataView(bytes.buffer);

  view.setUint32(0, 0x04034b50, true);
  view.setUint16(4, 20, true);
  view.setUint16(8, 8, true);
  view.setUint32(14, checksum, true);
  view.setUint32(18, compressed.length, true);
  view.setUint32(22, data.length, true);
  view.setUint16(26, filename.length, true);
  bytes.set(filename, 30);
  bytes.set(compressed, localHeaderSize);

  const centralOffset = localHeaderSize + compressed.length;
  view.setUint32(centralOffset, 0x02014b50, true);
  view.setUint16(centralOffset + 4, 20, true);
  view.setUint16(centralOffset + 6, 20, true);
  view.setUint16(centralOffset + 10, 8, true);
  view.setUint32(centralOffset + 16, checksum, true);
  view.setUint32(centralOffset + 20, compressed.length, true);
  view.setUint32(centralOffset + 24, data.length, true);
  view.setUint16(centralOffset + 28, filename.length, true);
  bytes.set(filename, centralOffset + 46);

  const endOffset = centralOffset + centralHeaderSize;
  view.setUint32(endOffset, 0x06054b50, true);
  view.setUint16(endOffset + 8, 1, true);
  view.setUint16(endOffset + 10, 1, true);
  view.setUint32(endOffset + 12, centralHeaderSize, true);
  view.setUint32(endOffset + 16, centralOffset, true);
  return bytes;
};

const withCompressionStreams = async (
  scenario: string,
  CompressionStream: typeof globalThis.CompressionStream | undefined,
  DecompressionStream: typeof globalThis.DecompressionStream | undefined,
) => {
  const compressionDescriptor = Object.getOwnPropertyDescriptor(globalThis, "CompressionStream");
  const decompressionDescriptor = Object.getOwnPropertyDescriptor(globalThis, "DecompressionStream");
  Object.defineProperty(globalThis, "CompressionStream", {
    configurable: true,
    writable: true,
    value: CompressionStream,
  });
  Object.defineProperty(globalThis, "DecompressionStream", {
    configurable: true,
    writable: true,
    value: DecompressionStream,
  });
  try {
    const url = new URL("../public/foliate-js/vendor/zip.js", import.meta.url);
    url.searchParams.set("scenario", scenario);
    return await import(url.href);
  } finally {
    if (compressionDescriptor) {
      Object.defineProperty(globalThis, "CompressionStream", compressionDescriptor);
    } else {
      delete (globalThis as { CompressionStream?: unknown }).CompressionStream;
    }
    if (decompressionDescriptor) {
      Object.defineProperty(globalThis, "DecompressionStream", decompressionDescriptor);
    } else {
      delete (globalThis as { DecompressionStream?: unknown }).DecompressionStream;
    }
  }
};

const readEntry = async (zipModule: typeof import("../public/foliate-js/vendor/zip.js")) => {
  const reader = new zipModule.ZipReader(new zipModule.BlobReader(
    new Blob([makeDeflatedZip("chapter.txt", "Lantern on Monterey")]),
  ));
  try {
    const entries = await reader.getEntries();
    assert.equal(entries.length, 1);
    assert.equal(entries[0].compressionMethod, 8);
    return await entries[0].getData(new zipModule.TextWriter());
  } finally {
    await reader.close();
  }
};

test("groupByObject converts property keys and preserves group order", () => {
  const symbol = Symbol("metadata");
  const items = [
    { key: "title", value: 1 },
    { key: null, value: 2 },
    { key: "title", value: 3 },
    { key: symbol, value: 4 },
  ];
  const indices: number[] = [];
  const groups = groupByObject(items, (item, index) => {
    indices.push(index);
    return item.key;
  });

  assert.equal(Object.getPrototypeOf(groups), null);
  assert.deepEqual(indices, [0, 1, 2, 3]);
  assert.deepEqual(groups.title, [items[0], items[2]]);
  assert.deepEqual(groups.null, [items[1]]);
  assert.deepEqual(groups[symbol], [items[3]]);
});

test("groupByMap keeps key identity and preserves group order", () => {
  const objectKey = {};
  const items = [
    { key: null, value: 1 },
    { key: "null", value: 2 },
    { key: objectKey, value: 3 },
    { key: objectKey, value: 4 },
  ];
  const groups = groupByMap(items, item => item.key);

  assert.deepEqual([...groups.keys()], [null, "null", objectKey]);
  assert.deepEqual(groups.get(null), [items[0]]);
  assert.deepEqual(groups.get("null"), [items[1]]);
  assert.deepEqual(groups.get(objectKey), [items[2], items[3]]);
});

test("method 8 ZIP extraction selects native or streaming fallback by capability", async (t) => {
  await t.test("uses native deflate-raw when supported", async () => {
    assert.equal(typeof NativeDecompressionStream, "function");
    let constructions = 0;
    class TrackingDecompressionStream {
      constructor(format: CompressionFormat) {
        constructions += 1;
        return new NativeDecompressionStream(format);
      }
    }
    const zipModule = await withCompressionStreams(
      "native",
      NativeCompressionStream,
      TrackingDecompressionStream as typeof globalThis.DecompressionStream,
    );

    assert.equal(await readEntry(zipModule), "Lantern on Monterey");
    assert.equal(constructions, 1);
  });

  await t.test("falls back when native rejects deflate-raw", async () => {
    let constructions = 0;
    class RejectingDecompressionStream {
      constructor(format: CompressionFormat) {
        constructions += 1;
        throw new TypeError(`Unsupported format: ${format}`);
      }
    }
    const zipModule = await withCompressionStreams(
      "rejects-deflate-raw",
      NativeCompressionStream,
      RejectingDecompressionStream as typeof globalThis.DecompressionStream,
    );

    assert.equal(await readEntry(zipModule), "Lantern on Monterey");
    assert.equal(constructions, 1);
  });

  await t.test("falls back when Compression Streams are missing", async () => {
    const zipModule = await withCompressionStreams("missing", undefined, undefined);
    assert.equal(await readEntry(zipModule), "Lantern on Monterey");
  });
});
