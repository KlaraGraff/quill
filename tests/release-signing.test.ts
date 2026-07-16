import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const repoFile = (path: string) => new URL(`../${path}`, import.meta.url);

test("macOS ad-hoc packages allow the bundled PDFium dylib", async () => {
  const baseConfig = JSON.parse(
    await readFile(repoFile("src-tauri/tauri.conf.json"), "utf8"),
  );
  const config = JSON.parse(
    await readFile(repoFile("src-tauri/tauri.macos.conf.json"), "utf8"),
  );
  const entitlements = await readFile(
    repoFile("src-tauri/Entitlements.adhoc.plist"),
    "utf8",
  );

  assert.equal(baseConfig.bundle.macOS.signingIdentity, "-");
  assert.equal(
    config.bundle.macOS.entitlements,
    "Entitlements.adhoc.plist",
  );
  assert.match(
    entitlements,
    /<key>com\.apple\.security\.cs\.disable-library-validation<\/key>\s*<true\/>/,
  );
});

test("Developer ID releases restore library validation and execute the PDFium smoke test", async () => {
  const workflow = await readFile(repoFile(".github/workflows/release.yml"), "utf8");

  assert.match(
    workflow,
    /"signingIdentity":"\$\{\{ steps\.certificate\.outputs\.identity \}\}","entitlements":null/,
  );
  assert.match(workflow, /bundle\/macos\/Lantern\.app/);
  assert.match(workflow, /CFBundleExecutable/);
  assert.match(workflow, /"\$APP_EXECUTABLE" pdfium-smoke/);
  assert.match(workflow, /test "\$APP_TEAM" = "\$PDFIUM_TEAM"/);
});
