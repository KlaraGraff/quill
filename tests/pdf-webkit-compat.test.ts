import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import {
  createPdfJsLoader,
  selectPdfJsVariant,
  snapshotPdfCapabilities,
} from "../public/foliate-js/pdf-compat.js";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const pdfJsRoot = path.join(repoRoot, "public/foliate-js/vendor/pdfjs");
const execFileAsync = promisify(execFile);
const modernCapabilities = {
  urlParse: true,
  abortSignalAny: true,
  promiseTry: true,
  promiseWithResolvers: true,
  structuredClone: true,
  uint8ArrayFromBase64: true,
  uint8ArrayToBase64: true,
  uint8ArrayToHex: true,
  setIntersection: true,
};
const legacyCapabilities = {
  urlParse: false,
  abortSignalAny: false,
  promiseTry: false,
  promiseWithResolvers: false,
  structuredClone: false,
  uint8ArrayFromBase64: false,
  uint8ArrayToBase64: false,
  uint8ArrayToHex: false,
  setIntersection: false,
};

const requireFoliateDependency = createRequire(
  path.join(repoRoot, "public/foliate-js/package.json"),
);
let createCanvas;
let canvasModule;
try {
  canvasModule = requireFoliateDependency("@napi-rs/canvas");
  ({ createCanvas } = canvasModule);
} catch {
  // pdfjs-dist's optional canvas dependency is present after nested npm ci.
}
for (const name of ["DOMMatrix", "ImageData", "Path2D"]) {
  if (globalThis[name] === undefined) {
    globalThis[name] = canvasModule?.[name] ?? class {};
  }
}

const makeMinimalPdf = () => {
  const stream = "BT\n/F1 12 Tf\n20 100 Td\n(Lantern PDF compatibility) Tj\nET\n";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
      + "/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}endstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    "<< /Title (W2 compatibility smoke) /Author (Lantern) >>",
  ];
  let source = "%PDF-1.4\n";
  const offsets = [0];
  for (const [index, object] of objects.entries()) {
    offsets.push(Buffer.byteLength(source));
    source += `${index + 1} 0 obj\n${object}\nendobj\n`;
  }
  const xrefOffset = Buffer.byteLength(source);
  source += `xref\n0 ${objects.length + 1}\n`;
  source += "0000000000 65535 f \n";
  source += offsets.slice(1)
    .map(offset => `${String(offset).padStart(10, "0")} 00000 n \n`)
    .join("");
  source += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R /Info 6 0 R >>\n`;
  source += `startxref\n${xrefOffset}\n%%EOF\n`;
  return new TextEncoder().encode(source);
};

const smokePdf = async (pdfjsLib, data, {
  expectedText,
  expectedTitle,
  textPageLimit = 1,
}) => {
  const loadingTask = pdfjsLib.getDocument({
    data,
    isEvalSupported: false,
    useWorkerFetch: false,
  });
  const document = await loadingTask.promise;
  try {
    const metadata = await document.getMetadata();
    assert.ok(metadata.info);
    assert.ok(document.numPages > 0);
    if (expectedTitle) {
      assert.match(String(metadata.info.Title ?? ""), expectedTitle);
    }

    let text = "";
    for (let pageNumber = 1;
      pageNumber <= Math.min(textPageLimit, document.numPages);
      pageNumber += 1) {
      const textPage = await document.getPage(pageNumber);
      const textContent = await textPage.getTextContent();
      text += textContent.items
        .map(item => "str" in item ? item.str : "")
        .join(" ");
      if (expectedText.test(text)) break;
    }
    assert.match(text, expectedText);

    const page = await document.getPage(1);
    const viewport = page.getViewport({ scale: 1 });
    assert.ok(viewport.width > 0);
    assert.ok(viewport.height > 0);
    const canvas = createCanvas(Math.ceil(viewport.width), Math.ceil(viewport.height));
    await page.render({
      canvasContext: canvas.getContext("2d"),
      viewport,
    }).promise;
    assert.ok(canvas.toBuffer("image/png").byteLength > 100);
  } finally {
    await document.destroy();
  }
};

test("snapshots only callable modern PDF.js capabilities", () => {
  const fakeGlobal = {
    URL: { parse() {} },
    AbortSignal: { any() {} },
    Promise: { try() {}, withResolvers() {} },
    structuredClone() {},
    Uint8Array: {
      fromBase64() {},
      prototype: { toBase64() {}, toHex() {} },
    },
    Set: { prototype: { intersection() {} } },
  };
  assert.deepEqual(snapshotPdfCapabilities(fakeGlobal), modernCapabilities);
  assert.deepEqual(snapshotPdfCapabilities({}), legacyCapabilities);
  assert.deepEqual(snapshotPdfCapabilities(undefined), legacyCapabilities);
});

test("selects modern only when every required capability is present", async t => {
  assert.equal(selectPdfJsVariant(modernCapabilities), "modern");
  for (const missing of Object.keys(modernCapabilities)) {
    await t.test(`missing ${missing}`, () => {
      assert.equal(
        selectPdfJsVariant({ ...modernCapabilities, [missing]: false }),
        "legacy",
      );
    });
  }
});

test("shares an in-flight import and returns a matched modern worker", async () => {
  let modernImports = 0;
  const modernModule = { version: "5.5.207" };
  const load = createPdfJsLoader({
    getCapabilities: () => modernCapabilities,
    importModern: async () => {
      modernImports += 1;
      return modernModule;
    },
    importLegacy: async () => assert.fail("legacy must not load"),
    workerUrls: { modern: "modern-worker", legacy: "legacy-worker" },
  });

  const first = load();
  const second = load();
  assert.strictEqual(first, second);
  assert.deepEqual(await first, {
    pdfjsLib: modernModule,
    workerUrl: "modern-worker",
    variant: "modern",
  });
  assert.equal(modernImports, 1);
});

test("falls back before Worker creation when the modern import rejects", async () => {
  const modernError = new Error("modern parse failure");
  const legacyModule = { version: "5.5.207" };
  const load = createPdfJsLoader({
    getCapabilities: () => modernCapabilities,
    importModern: async () => { throw modernError; },
    importLegacy: async () => legacyModule,
    workerUrls: { modern: "modern-worker", legacy: "legacy-worker" },
  });

  assert.deepEqual(await load(), {
    pdfjsLib: legacyModule,
    workerUrl: "legacy-worker",
    variant: "legacy",
  });
});

test("Retry keeps the first selected variant after a rejected import", async () => {
  let attempts = 0;
  let modernImports = 0;
  const capabilities = { ...legacyCapabilities };
  const legacyModule = { version: "5.5.207" };
  const load = createPdfJsLoader({
    getCapabilities: () => capabilities,
    importModern: async () => {
      modernImports += 1;
      return { version: "modern" };
    },
    importLegacy: async () => {
      attempts += 1;
      Object.assign(capabilities, modernCapabilities);
      if (attempts === 1) throw new Error("transient import failure");
      return legacyModule;
    },
    workerUrls: { modern: "modern-worker", legacy: "legacy-worker" },
  });

  const failed = load();
  await assert.rejects(failed, /transient import failure/);
  const retried = load();
  assert.notStrictEqual(retried, failed);
  assert.strictEqual((await retried).pdfjsLib, legacyModule);
  assert.equal(attempts, 2);
  assert.equal(modernImports, 0);
});

test("uses literal dynamic imports and preserves explicit Worker cleanup", async () => {
  const [compatSource, pdfSource] = await Promise.all([
    readFile(path.join(repoRoot, "public/foliate-js/pdf-compat.js"), "utf8"),
    readFile(path.join(repoRoot, "public/foliate-js/pdf.js"), "utf8"),
  ]);
  assert.match(compatSource, /import\('\.\/vendor\/pdfjs\/pdf\.mjs'\)/);
  assert.match(compatSource, /import\('\.\/vendor\/pdfjs\/legacy\/pdf\.mjs'\)/);
  assert.doesNotMatch(compatSource, /new Worker\s*\(/);
  assert.doesNotMatch(pdfSource, /import\s+['"]\.\/vendor\/pdfjs\/pdf\.mjs/);
  assert.doesNotMatch(pdfSource, /globalThis\.pdfjsLib/);
  assert.match(pdfSource, /new Worker\(workerUrl, \{ type: 'module' \}\)/);
  assert.match(pdfSource, /GlobalWorkerOptions\.workerPort = worker/);
  assert.equal(pdfSource.match(/worker\.terminate\(\)/g)?.length, 2);
});

test("ships same-version modern and legacy main-worker pairs without legacy maps", async () => {
  const assets = [
    "pdf.mjs",
    "pdf.worker.mjs",
    "legacy/pdf.mjs",
    "legacy/pdf.worker.mjs",
  ];
  const versions = [];
  for (const asset of assets) {
    const source = await readFile(path.join(pdfJsRoot, asset), "utf8");
    const version = source.match(/pdfjsVersion = ([^\s]+)/)?.[1];
    assert.ok(version, `${asset} must contain a PDF.js version header`);
    if (asset.startsWith("legacy/")) {
      assert.doesNotMatch(source, /\bstatic\s*\{/);
      assert.doesNotMatch(source, /sourceMappingURL/);
    }
    versions.push(version);
  }
  assert.deepEqual([...new Set(versions)], ["5.5.207"]);
  assert.deepEqual(
    (await readdir(path.join(pdfJsRoot, "legacy"))).sort(),
    ["pdf.mjs", "pdf.worker.mjs"],
  );
});

test("loads matching module exports for both variants", async () => {
  const modern = await createPdfJsLoader({
    getCapabilities: () => modernCapabilities,
  })();
  const legacy = await createPdfJsLoader({
    getCapabilities: () => legacyCapabilities,
  })();

  assert.equal(modern.variant, "modern");
  assert.match(modern.workerUrl, /\/vendor\/pdfjs\/pdf\.worker\.mjs$/);
  assert.equal(legacy.variant, "legacy");
  assert.match(legacy.workerUrl, /\/vendor\/pdfjs\/legacy\/pdf\.worker\.mjs$/);
  for (const loaded of [modern, legacy]) {
    assert.equal(loaded.pdfjsLib.version, "5.5.207");
    assert.equal(typeof loaded.pdfjsLib.getDocument, "function");
    assert.equal(typeof loaded.pdfjsLib.PDFDataRangeTransport, "function");
    assert.equal(typeof loaded.pdfjsLib.GlobalWorkerOptions, "function");
  }
});

test("legacy cold-loads when all modern APIs are missing", async () => {
  const compatUrl = new URL(
    "../public/foliate-js/pdf-compat.js",
    import.meta.url,
  ).href;
  const pdfBase64 = Buffer.from(makeMinimalPdf()).toString("base64");
  const script = `
    globalThis.DOMMatrix = class DOMMatrix {};
    globalThis.ImageData = class ImageData {};
    globalThis.Path2D = class Path2D {};
    Object.defineProperty(Promise, 'withResolvers', {
      configurable: true,
      writable: true,
      value: undefined,
    });
    Object.defineProperty(globalThis, 'structuredClone', {
      configurable: true,
      writable: true,
      value: undefined,
    });
    Object.defineProperty(Uint8Array, 'fromBase64', {
      configurable: true,
      writable: true,
      value: undefined,
    });
    for (const [target, key] of [
      [URL, 'parse'],
      [AbortSignal, 'any'],
      [Promise, 'try'],
      [Uint8Array.prototype, 'toBase64'],
      [Uint8Array.prototype, 'toHex'],
      [Set.prototype, 'intersection'],
    ]) {
      Object.defineProperty(target, key, {
        configurable: true,
        writable: true,
        value: undefined,
      });
    }

    const { loadPdfJs } = await import(${JSON.stringify(compatUrl)});
    const loaded = await loadPdfJs();
    loaded.pdfjsLib.GlobalWorkerOptions.workerSrc = loaded.workerUrl;
    const task = loaded.pdfjsLib.getDocument({
      data: Uint8Array.from(Buffer.from(${JSON.stringify(pdfBase64)}, 'base64')),
      isEvalSupported: false,
      useWorkerFetch: false,
    });
    const pdf = await task.promise;
    const page = await pdf.getPage(1);
    const content = await page.getTextContent();
    const text = content.items.map(item => item.str ?? '').join(' ');
    process.stdout.write('W2_RESULT=' + JSON.stringify({
      variant: loaded.variant,
      version: loaded.pdfjsLib.version,
      pages: pdf.numPages,
      text,
      capabilities: {
        urlParse: typeof URL.parse,
        abortSignalAny: typeof AbortSignal.any,
        promiseTry: typeof Promise.try,
        promiseWithResolvers: typeof Promise.withResolvers,
        structuredClone: typeof structuredClone,
        uint8ArrayFromBase64: typeof Uint8Array.fromBase64,
        uint8ArrayToBase64: typeof Uint8Array.prototype.toBase64,
        uint8ArrayToHex: typeof Uint8Array.prototype.toHex,
        setIntersection: typeof Set.prototype.intersection,
      },
    }) + '\\n');
    await pdf.destroy();
  `;
  const { stdout } = await execFileAsync(
    process.execPath,
    ["--input-type=module", "--eval", script],
    { cwd: repoRoot, maxBuffer: 4 * 1024 * 1024 },
  );
  const resultLine = stdout
    .split(/\r?\n/)
    .find(line => line.startsWith("W2_RESULT="));
  assert.ok(resultLine, `legacy cold-load did not report a result:\n${stdout}`);
  const result = JSON.parse(resultLine.slice("W2_RESULT=".length));
  assert.deepEqual(result, {
    variant: "legacy",
    version: "5.5.207",
    pages: 1,
    text: "Lantern PDF compatibility",
    capabilities: {
      urlParse: "function",
      abortSignalAny: "function",
      promiseTry: "function",
      promiseWithResolvers: "function",
      structuredClone: "function",
      uint8ArrayFromBase64: "function",
      uint8ArrayToBase64: "function",
      uint8ArrayToHex: "function",
      setIntersection: "function",
    },
  });
});

test("renders a minimal PDF with modern and legacy PDF.js", {
  skip: createCanvas ? false : "nested pdfjs-dist canvas dependency is not installed",
}, async t => {
  const variants = [
    await createPdfJsLoader({ getCapabilities: () => modernCapabilities })(),
    await createPdfJsLoader({ getCapabilities: () => legacyCapabilities })(),
  ];
  for (const loaded of variants) {
    await t.test(loaded.variant, async () => {
      await smokePdf(
        loaded.pdfjsLib,
        makeMinimalPdf(),
        { expectedText: /Lantern PDF compatibility/ },
      );
    });
  }
});

const hayekPdfPath = path.join(
  repoRoot,
  "测试文件",
  "The Road to Serfdom - Text and Documents (The Definite Edition, 2010) "
    + "(Friedrich August Hayek) (z-library.sk, 1lib.sk, z-lib.sk).pdf",
);

test("opens and renders the Hayek PDF with both variants", {
  skip: !createCanvas || !existsSync(hayekPdfPath)
    ? "local Hayek smoke prerequisites are unavailable"
    : false,
  timeout: 120_000,
}, async t => {
  const bytes = await readFile(hayekPdfPath);
  const variants = [
    await createPdfJsLoader({ getCapabilities: () => modernCapabilities })(),
    await createPdfJsLoader({ getCapabilities: () => legacyCapabilities })(),
  ];
  for (const loaded of variants) {
    await t.test(loaded.variant, async () => {
      await smokePdf(
        loaded.pdfjsLib,
        new Uint8Array(bytes),
        {
          expectedText: /ROAD TO SERFDOM/i,
          expectedTitle: /The Road to Serfdom/i,
          textPageLimit: 2,
        },
      );
    });
  }
});
