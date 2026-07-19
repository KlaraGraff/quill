import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { promisify } from "node:util";

import {
  selectPdfJsVariant,
  snapshotPdfCapabilities,
} from "../public/foliate-js/pdf-compat.js";
import { toReaderOpenError } from "../src/pages/reader/reader-open-error.ts";

const execFileAsync = promisify(execFile);
const fixtureUrl = (name: string) =>
  new URL(`./fixtures/reader-compat/${name}`, import.meta.url);

const restoreProperty = (
  target: object,
  key: PropertyKey,
  descriptor: PropertyDescriptor | undefined,
) => {
  if (descriptor) Object.defineProperty(target, key, descriptor);
  else delete (target as Record<PropertyKey, unknown>)[key];
};

const importZipWithoutCompressionStreams = async () => {
  const compressionDescriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "CompressionStream",
  );
  const decompressionDescriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "DecompressionStream",
  );

  Object.defineProperty(globalThis, "CompressionStream", {
    configurable: true,
    writable: true,
    value: undefined,
  });
  Object.defineProperty(globalThis, "DecompressionStream", {
    configurable: true,
    writable: true,
    value: undefined,
  });

  try {
    const url = new URL("../public/foliate-js/vendor/zip.js", import.meta.url);
    url.searchParams.set("reader-compat", "missing-compression-streams");
    return await import(url.href);
  } finally {
    restoreProperty(globalThis, "CompressionStream", compressionDescriptor);
    restoreProperty(globalThis, "DecompressionStream", decompressionDescriptor);
  }
};

test("deflated EPUB fixture extracts through the ZIP fallback", async () => {
  const zipModule = await importZipWithoutCompressionStreams();
  const bytes = await readFile(fixtureUrl("minimal-deflated.epub"));
  const archive = new zipModule.ZipReader(
    new zipModule.BlobReader(new Blob([bytes])),
  );

  try {
    const entries = await archive.getEntries();
    assert.deepEqual(entries.map(entry => entry.filename), [
      "mimetype",
      "META-INF/container.xml",
      "EPUB/package.opf",
      "EPUB/toc.ncx",
      "EPUB/nav.xhtml",
      "EPUB/chapter.xhtml",
      "EPUB/image.svg",
    ]);
    assert.equal(entries[0].compressionMethod, 0);
    assert.ok(entries.slice(1).every(entry => entry.compressionMethod === 8));

    const payloads = new Map(await Promise.all(entries.map(async entry => [
      entry.filename,
      await entry.getData(new zipModule.TextWriter()),
    ] as const)));

    assert.equal(payloads.get("mimetype"), "application/epub+zip");
    assert.match(
      payloads.get("META-INF/container.xml") ?? "",
      /full-path="EPUB\/package\.opf"/,
    );
    assert.match(
      payloads.get("EPUB/package.opf") ?? "",
      /Lantern Reader Compatibility Fixture/,
    );
    assert.match(payloads.get("EPUB/package.opf") ?? "", /properties="nav"/);
    assert.match(payloads.get("EPUB/package.opf") ?? "", /idref="chapter"/);
    assert.match(payloads.get("EPUB/toc.ncx") ?? "", /chapter\.xhtml/);
    assert.match(payloads.get("EPUB/nav.xhtml") ?? "", /chapter\.xhtml/);
    assert.match(
      payloads.get("EPUB/chapter.xhtml") ?? "",
      /pure JavaScript fallback/,
    );
    assert.match(payloads.get("EPUB/chapter.xhtml") ?? "", /src="image\.svg"/);
    assert.match(payloads.get("EPUB/image.svg") ?? "", /<svg\b/);
  } finally {
    await archive.close();
  }
});

test("PDF capability snapshot and selector cover the Safari 15 boundary", () => {
  const modernGlobal = {
    URL: { parse() {} },
    AbortSignal: { any() {} },
    Promise: { withResolvers() {}, try() {} },
    structuredClone() {},
    Uint8Array: {
      fromBase64() {},
      prototype: { toBase64() {}, toHex() {} },
    },
    Set: { prototype: { intersection() {} } },
  };
  const modern = snapshotPdfCapabilities(modernGlobal);

  assert.deepEqual(modern, {
    urlParse: true,
    abortSignalAny: true,
    promiseWithResolvers: true,
    promiseTry: true,
    structuredClone: true,
    uint8ArrayFromBase64: true,
    uint8ArrayToBase64: true,
    uint8ArrayToHex: true,
    setIntersection: true,
  });
  assert.equal(selectPdfJsVariant(modern), "modern");

  for (const missing of Object.keys(modern) as Array<keyof typeof modern>) {
    assert.equal(
      selectPdfJsVariant({ ...modern, [missing]: false }),
      "legacy",
      `${missing} must keep the modern build isolated`,
    );
  }

  assert.deepEqual(snapshotPdfCapabilities({}), {
    urlParse: false,
    abortSignalAny: false,
    promiseWithResolvers: false,
    promiseTry: false,
    structuredClone: false,
    uint8ArrayFromBase64: false,
    uint8ArrayToBase64: false,
    uint8ArrayToHex: false,
    setIntersection: false,
  });
});

const probePdfVariant = async (requestedVariant: "modern" | "legacy") => {
  const pdfCompatUrl = new URL(
    "../public/foliate-js/pdf-compat.js",
    import.meta.url,
  ).href;
  const pdfFixtureUrl = fixtureUrl("minimal-text.pdf").href;
  const script = `
    import { readFile } from 'node:fs/promises';

    globalThis.DOMMatrix = class DOMMatrix {};
    globalThis.ImageData = class ImageData {};
    globalThis.Path2D = class Path2D {};

    if (process.env.W4_PDF_VARIANT === 'modern') {
      if (typeof URL.parse !== 'function') {
        URL.parse = (url, base) => {
          try {
            return new URL(url, base);
          } catch {
            return null;
          }
        };
      }
      if (typeof AbortSignal.any !== 'function') {
        AbortSignal.any = signals => {
          const controller = new AbortController();
          for (const signal of signals) {
            if (signal.aborted) {
              controller.abort(signal.reason);
              break;
            }
            signal.addEventListener(
              'abort',
              () => controller.abort(signal.reason),
              { once: true },
            );
          }
          return controller.signal;
        };
      }
      if (typeof Promise.withResolvers !== 'function') {
        Promise.withResolvers = () => {
          let resolve;
          let reject;
          const promise = new Promise((res, rej) => {
            resolve = res;
            reject = rej;
          });
          return { promise, resolve, reject };
        };
      }
      if (typeof Promise.try !== 'function') {
        Promise.try = (callback, ...args) =>
          new Promise(resolve => resolve(callback(...args)));
      }
      if (typeof globalThis.structuredClone !== 'function') {
        globalThis.structuredClone = value => value;
      }
      if (typeof Uint8Array.fromBase64 !== 'function') {
        Uint8Array.fromBase64 = value =>
          new Uint8Array(Buffer.from(value, 'base64'));
      }
      if (typeof Uint8Array.prototype.toBase64 !== 'function') {
        Uint8Array.prototype.toBase64 = function () {
          return Buffer.from(this).toString('base64');
        };
      }
      if (typeof Uint8Array.prototype.toHex !== 'function') {
        Uint8Array.prototype.toHex = function () {
          return Buffer.from(this).toString('hex');
        };
      }
      if (typeof Set.prototype.intersection !== 'function') {
        Set.prototype.intersection = function (other) {
          return new Set([...this].filter(value => other.has(value)));
        };
      }
    } else {
      Object.defineProperty(Uint8Array, 'fromBase64', {
        configurable: true,
        writable: true,
        value: undefined,
      });
    }

    const { loadPdfJs } = await import(${JSON.stringify(pdfCompatUrl)});
    const { pdfjsLib, workerUrl, variant } = await loadPdfJs();
    pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;

    const data = new Uint8Array(await readFile(new URL(${JSON.stringify(pdfFixtureUrl)})));
    const loadingTask = pdfjsLib.getDocument({
      data,
      isEvalSupported: false,
      useWorkerFetch: false,
    });
    const pdf = await loadingTask.promise;
    const page = await pdf.getPage(1);
    const textContent = await page.getTextContent();
    const text = textContent.items.map(item => item.str ?? '').join(' ').trim();

    process.stdout.write('W4_RESULT=' + JSON.stringify({
      variant,
      workerUrl,
      version: pdfjsLib.version,
      pages: pdf.numPages,
      text,
    }) + '\\n');
    await pdf.destroy();
  `;
  const { stdout } = await execFileAsync(
    process.execPath,
    ["--input-type=module", "--eval", script],
    {
      env: { ...process.env, W4_PDF_VARIANT: requestedVariant },
      maxBuffer: 4 * 1024 * 1024,
    },
  );
  const resultLine = stdout
    .split(/\r?\n/)
    .find(line => line.startsWith("W4_RESULT="));
  assert.ok(resultLine, `PDF probe did not report a result:\n${stdout}`);
  return JSON.parse(resultLine.slice("W4_RESULT=".length)) as {
    variant: "modern" | "legacy";
    workerUrl: string;
    version: string;
    pages: number;
    text: string;
  };
};

test("PDF loader pairs each main build with its worker and reads the fixture", async () => {
  const modern = await probePdfVariant("modern");
  const legacy = await probePdfVariant("legacy");

  assert.equal(modern.variant, "modern");
  assert.match(modern.workerUrl, /\/vendor\/pdfjs\/pdf\.worker\.mjs$/);
  assert.doesNotMatch(modern.workerUrl, /\/legacy\//);

  assert.equal(legacy.variant, "legacy");
  assert.match(
    legacy.workerUrl,
    /\/vendor\/pdfjs\/legacy\/pdf\.worker\.mjs$/,
  );

  assert.equal(modern.version, legacy.version);
  assert.equal(modern.pages, 1);
  assert.equal(legacy.pages, 1);
  assert.equal(modern.text, "Lantern reader compatibility");
  assert.equal(legacy.text, modern.text);
});

test("compatibility loader failures remain generic Reader errors", () => {
  const failures = [
    new TypeError("false is not a constructor (evaluating 'new i(o,r)')"),
    new SyntaxError("Unexpected token '{'"),
    new TypeError("Promise.withResolvers is not a function"),
    new TypeError("Failed to fetch dynamically imported module"),
  ];

  for (const failure of failures) {
    assert.deepEqual(toReaderOpenError(failure, "pdf"), {
      kind: "generic",
      detail: failure.message,
    });
  }

  const invalidPdf = new Error("Invalid PDF structure.");
  invalidPdf.name = "InvalidPDFException";
  assert.equal(toReaderOpenError(invalidPdf, "pdf").kind, "invalid-pdf");
});
