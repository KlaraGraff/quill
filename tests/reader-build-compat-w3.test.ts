import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import {
  buildReaderAssets,
  makeCssCompatibleWithSafari15,
} from "../scripts/build-reader-assets.mjs";
import {
  LEGACY_PDF_RUNTIME_MARKER,
  REQUIRED_PDF_CAPABILITIES,
  SIZE_REPORT_FILE,
  checkReaderCompatibility,
} from "../scripts/check-reader-compat.mjs";

const PDF_HEADER = `/**
 * pdfjsVersion = 5.5.207
 * pdfjsBuild = 527964698
 */
export const version = "5.5.207";
`;
const LEGACY_PDF_HEADER = `/* ${LEGACY_PDF_RUNTIME_MARKER} */\n${PDF_HEADER}`;

const capabilitySource = ({ includeUrlParse = true } = {}) => `
const modernWorkerUrl = new URL(
  "./vendor/pdfjs/pdf.worker.mjs", import.meta.url,
).toString();
const legacyWorkerUrl = new URL(
  "./vendor/pdfjs/legacy/pdf.worker.mjs", import.meta.url,
).toString();
const importModern = () => import("./vendor/pdfjs/pdf.mjs");
const importLegacy = () => import("./vendor/pdfjs/legacy/pdf.mjs");

export const snapshotPdfCapabilities = globalObject => ({
  promiseWithResolvers: typeof globalObject?.Promise?.withResolvers === "function",
  structuredClone: typeof globalObject?.structuredClone === "function",
  uint8ArrayFromBase64: typeof globalObject?.Uint8Array?.fromBase64 === "function",
  ${includeUrlParse ? "urlParse: typeof globalObject?.URL?.parse === \"function\"," : ""}
  abortSignalAny: typeof globalObject?.AbortSignal?.any === "function",
  promiseTry: typeof globalObject?.Promise?.try === "function",
  uint8ArrayToBase64:
    typeof globalObject?.Uint8Array?.prototype?.toBase64 === "function",
  uint8ArrayToHex: typeof globalObject?.Uint8Array?.prototype?.toHex === "function",
  setIntersection: typeof globalObject?.Set?.prototype?.intersection === "function",
});

export const selectPdfJsVariant = capabilities =>
  capabilities.promiseWithResolvers
  && capabilities.structuredClone
  && capabilities.uint8ArrayFromBase64
  ${includeUrlParse ? "&& capabilities.urlParse" : ""}
  && capabilities.abortSignalAny
  && capabilities.promiseTry
  && capabilities.uint8ArrayToBase64
  && capabilities.uint8ArrayToHex
  && capabilities.setIntersection
    ? "modern"
    : "legacy";

export const loadPdfJs = async () => {
  const variant = selectPdfJsVariant(snapshotPdfCapabilities(globalThis));
  return variant === "modern"
    ? { pdfjsLib: await importModern(), workerUrl: modernWorkerUrl, variant }
    : { pdfjsLib: await importLegacy(), workerUrl: legacyWorkerUrl, variant };
};
`;

const writeFixtureFile = async (root: string, path: string, contents: string) => {
  const output = join(root, path);
  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, contents, "utf8");
};

const createDistFixture = async (options = {}) => {
  const root = await mkdtemp(join(tmpdir(), "lantern-reader-w3-"));
  await writeFixtureFile(root, "assets/app.js", "export const ready = true;\n");
  await writeFixtureFile(
    root,
    "assets/app.css",
    `@layer theme {
      :root { --color-accent: oklch(62% .2 280); }
      @property --tw-x { syntax: "*"; inherits: false; initial-value: 0; }
      * { --tw-x: 0; }
      .viewport { height: 100dvh; text-wrap: wrap; color: #111;
        color: color-mix(in srgb, #111 50%, transparent); }
      .annotationLayer section {
        &:has(div.annotationContent) { canvas.annotationContent { display: none; } }
      }
    }`,
  );
  await writeFixtureFile(
    root,
    "foliate-js/sample.js",
    `class Sample { static { this.ready = true } }
     export const url = import.meta.url;
     export const load = () => import("./lazy.js");
     export default Sample;
    `,
  );
  await writeFixtureFile(root, "foliate-js/lazy.js", "export default true;\n");
  await writeFixtureFile(
    root,
    "foliate-js/pdf.js",
    `import { loadPdfJs } from "./pdf-compat.js";
     export const makePDF = () => loadPdfJs();
    `,
  );
  await writeFixtureFile(
    root,
    "foliate-js/pdf-compat.js",
    capabilitySource(options),
  );
  await writeFixtureFile(root, "foliate-js/vendor/zip.js", "export const zip = true;\n");
  for (const path of [
    "foliate-js/vendor/pdfjs/pdf.mjs",
    "foliate-js/vendor/pdfjs/pdf.worker.mjs",
    "foliate-js/vendor/pdfjs/legacy/pdf.mjs",
    "foliate-js/vendor/pdfjs/legacy/pdf.worker.mjs",
  ]) {
    await writeFixtureFile(
      root,
      path,
      path.includes("/legacy/") ? LEGACY_PDF_HEADER : PDF_HEADER,
    );
  }
  await writeFixtureFile(
    root,
    "foliate-js/node_modules/not-runtime.js",
    "throw new Error('must not ship');\n",
  );
  return root;
};

test("W3 transforms Reader JavaScript and emits Safari 15 CSS fallbacks", async () => {
  const { css, stats } = await makeCssCompatibleWithSafari15(
    `@layer utilities {
      :root { --color-test: oklch(62% .2 280); }
      .dialog { max-height: 100dvh; text-wrap: wrap; }
    }`,
    "fixture.css",
  );

  assert.doesNotMatch(css, /@layer|oklch\(/u);
  assert.match(css, /100vh/u);
  assert.match(css, /100dvh/u);
  assert.match(css, /white-space:normal/u);
  assert.equal(stats.flattenedLayers, 1);
  assert.equal(stats.convertedCustomColors, 1);
  assert.equal(stats.legacyUnitFallbacks, 1);
  assert.equal(stats.textWrapFallbacks, 1);
});

test("W3 compatibility gate validates PDF assets and writes a size report", async () => {
  const distDir = await createDistFixture();
  const build = await buildReaderAssets({ distDir });
  const report = await checkReaderCompatibility({ distDir });
  const transformedReader = await readFile(join(distDir, "foliate-js/sample.js"), "utf8");
  const css = await readFile(join(distDir, "assets/app.css"), "utf8");
  const sizeReport = JSON.parse(
    await readFile(join(distDir, SIZE_REPORT_FILE), "utf8"),
  );

  assert.equal(build.target, "safari15");
  assert.doesNotMatch(transformedReader, /static\s*\{/u);
  assert.match(transformedReader, /import\.meta\.url/u);
  assert.match(transformedReader, /import\("\.\/lazy\.js"\)/u);
  assert.doesNotMatch(css, /@layer|oklch\(|:has\(/u);
  assert.equal(report.pdfjs.version, "5.5.207");
  assert.deepEqual(
    Object.keys(report.pdfjs.capabilityContract),
    [...REQUIRED_PDF_CAPABILITIES],
  );
  assert.equal(sizeReport.schemaVersion, 1);
  await assert.rejects(
    readFile(join(distDir, "foliate-js/node_modules/not-runtime.js"), "utf8"),
    /ENOENT/u,
  );
});

test("W3 gate rejects an incomplete modern PDF.js capability selector", async () => {
  const distDir = await createDistFixture({ includeUrlParse: false });
  await buildReaderAssets({ distDir });
  await assert.rejects(
    checkReaderCompatibility({ distDir }),
    /snapshotPdfCapabilities does not probe: URL\.parse/u,
  );
});

test("W3 gate rejects Safari 15.4-only runtime methods", async () => {
  const readerFixture = await createDistFixture();
  await writeFixtureFile(
    readerFixture,
    "foliate-js/lazy.js",
    "export const last = [1].at(-1);\n",
  );
  await buildReaderAssets({ distDir: readerFixture });
  await assert.rejects(
    checkReaderCompatibility({ distDir: readerFixture }),
    /Safari 15-incompatible method at\(\)/u,
  );

  const appFixture = await createDistFixture();
  await writeFixtureFile(
    appFixture,
    "assets/app.js",
    "export const requestId = crypto.randomUUID();\n",
  );
  await buildReaderAssets({ distDir: appFixture });
  await assert.rejects(
    checkReaderCompatibility({ distDir: appFixture }),
    /crypto\.randomUUID outside a guarded probe/u,
  );
});
