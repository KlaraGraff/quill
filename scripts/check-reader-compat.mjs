import { readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { join, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";

import postcss from "postcss";
import ts from "typescript";

import {
  SAFARI_15_TARGET,
  READER_TRANSFORM_MANIFEST,
  listFiles,
} from "./build-reader-assets.mjs";

export const SIZE_REPORT_FILE = "reader-compat-size-report.json";

export const SIZE_BUDGETS = Object.freeze({
  legacyPdfRawBytes: 3_800_000,
  legacyPdfGzipBytes: 900_000,
  zipBaselineGzipBytes: 27_164,
  zipGzipIncrementBytes: 100_000,
  dmgIncrementBytes: 1_500_000,
});

export const PDF_ASSETS = Object.freeze({
  modernMain: "foliate-js/vendor/pdfjs/pdf.mjs",
  modernWorker: "foliate-js/vendor/pdfjs/pdf.worker.mjs",
  legacyMain: "foliate-js/vendor/pdfjs/legacy/pdf.mjs",
  legacyWorker: "foliate-js/vendor/pdfjs/legacy/pdf.worker.mjs",
});

export const LEGACY_PDF_RUNTIME_MARKER =
  "Lantern Safari 15 runtime compatibility for the PDF.js legacy pair";

const PDF_IMPORTS = Object.freeze({
  modernMain: "./vendor/pdfjs/pdf.mjs",
  modernWorker: "./vendor/pdfjs/pdf.worker.mjs",
  legacyMain: "./vendor/pdfjs/legacy/pdf.mjs",
  legacyWorker: "./vendor/pdfjs/legacy/pdf.worker.mjs",
});

const BLOCKED_CALLS = new Set([
  "Array.fromAsync",
  "Object.groupBy",
  "Object.hasOwn",
  "Map.groupBy",
  "Promise.withResolvers",
  "Promise.try",
  "URL.canParse",
  "URL.parse",
  "AbortSignal.any",
  "Uint8Array.fromBase64",
]);

const BLOCKED_METHOD_CALLS = new Set([
  "at",
  "findLast",
  "findLastIndex",
  "intersection",
  "toBase64",
  "toHex",
  "toReversed",
  "toSorted",
  "toSpliced",
]);

export const REQUIRED_PDF_CAPABILITIES = Object.freeze([
  "Promise.withResolvers",
  "structuredClone",
  "Uint8Array.fromBase64",
  "URL.parse",
  "AbortSignal.any",
  "Promise.try",
  "Uint8Array.prototype.toBase64",
  "Uint8Array.prototype.toHex",
  "Set.prototype.intersection",
]);

const GUARDED_GLOBAL_CALLS = new Set(["crypto.randomUUID", "structuredClone"]);
const GUARDED_CONSTRUCTORS = new Set([
  "CompressionStream",
  "DecompressionStream",
]);

const MODERN_CSS_COLOR = /\b(?:oklch|oklab|lab|lch|color-mix)\(/iu;
const MODERN_CSS_UNIT = /-?(?:\d+(?:\.\d+)?|\.\d+)(?:dvh|svh|lvh|dvw|svw|lvw|rlh|lh)\b/iu;

const normalizePath = (path) => path.split(sep).join("/");

const fail = (message) => {
  throw new Error(message);
};

const readRequiredFile = async (root, path) => {
  const absolutePath = join(root, path);
  try {
    const info = await stat(absolutePath);
    if (!info.isFile()) fail(`Required asset is not a file: ${path}`);
  } catch (error) {
    if (error instanceof Error && error.message.startsWith("Required asset")) throw error;
    fail(`Missing required asset: ${path}`);
  }
  return readFile(absolutePath);
};

const assetMetrics = (path, bytes) => ({
  path,
  rawBytes: bytes.byteLength,
  gzipBytes: gzipSync(bytes, { level: 9 }).byteLength,
});

const parseJavaScript = (source, path) => {
  const sourceFile = ts.createSourceFile(
    path,
    source,
    ts.ScriptTarget.Latest,
    true,
    path.endsWith(".mjs") ? ts.ScriptKind.JS : ts.ScriptKind.JS,
  );
  const diagnostics = sourceFile.parseDiagnostics ?? [];
  if (diagnostics.length > 0) {
    const diagnostic = diagnostics[0];
    const message = ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n");
    fail(`${path} is not valid JavaScript: ${message}`);
  }
  return sourceFile;
};

const nodeLocation = (sourceFile, node) => {
  const { line, character } = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
  return `${sourceFile.fileName}:${line + 1}:${character + 1}`;
};

const expressionName = (node) => {
  if (ts.isIdentifier(node)) return node.text;
  if (ts.isPropertyAccessExpression(node)) {
    const owner = expressionName(node.expression);
    return owner ? `${owner}.${node.name.text}` : node.name.text;
  }
  if (
    ts.isElementAccessExpression(node)
    && node.argumentExpression
    && ts.isStringLiteralLike(node.argumentExpression)
  ) {
    const owner = expressionName(node.expression);
    return owner ? `${owner}.${node.argumentExpression.text}` : node.argumentExpression.text;
  }
  return undefined;
};

const matchesQualifiedName = (actual, expected) =>
  actual === expected || actual?.endsWith(`.${expected}`);

const isInsideTry = (node) => {
  for (let current = node.parent; current; current = current.parent) {
    if (ts.isTryStatement(current)) return true;
  }
  return false;
};

const conditionGuardsName = (condition, name, sourceFile) => {
  const text = condition.getText(sourceFile);
  return text.includes("typeof") && text.includes(name);
};

const isGuardedByTypeof = (node, name, sourceFile) => {
  let child = node;
  for (let parent = node.parent; parent; child = parent, parent = parent.parent) {
    if (
      ts.isConditionalExpression(parent)
      && parent.whenTrue === child
      && conditionGuardsName(parent.condition, name, sourceFile)
    ) {
      return true;
    }
    if (
      ts.isIfStatement(parent)
      && parent.thenStatement === child
      && conditionGuardsName(parent.expression, name, sourceFile)
    ) {
      return true;
    }
    if (
      ts.isBinaryExpression(parent)
      && parent.right === child
      && parent.operatorToken.kind === ts.SyntaxKind.AmpersandAmpersandToken
      && conditionGuardsName(parent.left, name, sourceFile)
    ) {
      return true;
    }
  }
  return false;
};

const scanJavaScriptCompatibility = (
  sourceFile,
  { restrictedApis = true, staticBlocks = true } = {},
) => {
  const errors = [];

  const visit = (node) => {
    if (staticBlocks && ts.isClassStaticBlockDeclaration(node)) {
      errors.push(`${nodeLocation(sourceFile, node)} uses a class static block`);
    }

    if (ts.isRegularExpressionLiteral(node)) {
      const literal = node.text;
      const finalSlash = literal.lastIndexOf("/");
      if (finalSlash >= 0 && literal.slice(finalSlash + 1).includes("v")) {
        errors.push(`${nodeLocation(sourceFile, node)} uses the Safari 17 RegExp v flag`);
      }
    }

    if (restrictedApis && ts.isCallExpression(node)) {
      const name = expressionName(node.expression);
      for (const blocked of BLOCKED_CALLS) {
        if (matchesQualifiedName(name, blocked)) {
          errors.push(`${nodeLocation(sourceFile, node)} directly calls ${blocked}`);
        }
      }
      if (
        ts.isPropertyAccessExpression(node.expression)
        && BLOCKED_METHOD_CALLS.has(node.expression.name.text)
      ) {
        errors.push(
          `${nodeLocation(sourceFile, node)} directly calls Safari 15-incompatible method `
          + `${node.expression.name.text}()`,
        );
      }
      for (const guarded of GUARDED_GLOBAL_CALLS) {
        if (
          matchesQualifiedName(name, guarded)
          && !isInsideTry(node)
          && !isGuardedByTypeof(node, guarded, sourceFile)
        ) {
          errors.push(
            `${nodeLocation(sourceFile, node)} calls ${guarded} outside a guarded probe`,
          );
        }
      }
    }

    if (restrictedApis && ts.isNewExpression(node)) {
      const name = expressionName(node.expression);
      if (name && GUARDED_CONSTRUCTORS.has(name) && !isInsideTry(node)) {
        errors.push(`${nodeLocation(sourceFile, node)} constructs ${name} outside try/catch`);
      }
    }

    ts.forEachChild(node, visit);
  };

  visit(sourceFile);
  return errors;
};

const isModernPdfSpecifier = (specifier) => {
  const normalized = specifier.replaceAll("\\", "/");
  return normalized.endsWith("/vendor/pdfjs/pdf.mjs")
    && !normalized.includes("/vendor/pdfjs/legacy/");
};

const checkNoStaticModernPdfImport = (sourceFile) => {
  const errors = [];
  for (const statement of sourceFile.statements) {
    const moduleSpecifier = (
      ts.isImportDeclaration(statement) || ts.isExportDeclaration(statement)
    ) && statement.moduleSpecifier;
    if (
      moduleSpecifier
      && ts.isStringLiteralLike(moduleSpecifier)
      && isModernPdfSpecifier(moduleSpecifier.text)
    ) {
      errors.push(
        `${nodeLocation(sourceFile, statement)} statically imports modern PDF.js`,
      );
    }
  }
  return errors;
};

const collectPdfLoaderPaths = (sourceFile) => {
  const dynamicImports = new Set();
  const literals = new Set();

  const visit = (node) => {
    if (ts.isStringLiteralLike(node)) literals.add(node.text);
    if (
      ts.isCallExpression(node)
      && node.expression.kind === ts.SyntaxKind.ImportKeyword
      && node.arguments.length === 1
      && ts.isStringLiteralLike(node.arguments[0])
    ) {
      dynamicImports.add(node.arguments[0].text);
    }
    ts.forEachChild(node, visit);
  };

  visit(sourceFile);
  return { dynamicImports, literals };
};

const unwrapExpression = (node) => {
  let current = node;
  while (
    ts.isParenthesizedExpression(current)
    || ts.isAsExpression(current)
    || ts.isTypeAssertionExpression(current)
  ) {
    current = current.expression;
  }
  return current;
};

const findNamedFunction = (sourceFile, name) => {
  for (const statement of sourceFile.statements) {
    if (ts.isFunctionDeclaration(statement) && statement.name?.text === name) {
      return statement;
    }
    if (!ts.isVariableStatement(statement)) continue;
    for (const declaration of statement.declarationList.declarations) {
      if (
        ts.isIdentifier(declaration.name)
        && declaration.name.text === name
        && declaration.initializer
        && (
          ts.isArrowFunction(declaration.initializer)
          || ts.isFunctionExpression(declaration.initializer)
        )
      ) {
        return declaration.initializer;
      }
    }
  }
  return undefined;
};

const findVariableInitializer = (sourceFile, name) => {
  for (const statement of sourceFile.statements) {
    if (!ts.isVariableStatement(statement)) continue;
    for (const declaration of statement.declarationList.declarations) {
      if (
        ts.isIdentifier(declaration.name)
        && declaration.name.text === name
        && declaration.initializer
      ) {
        return declaration.initializer;
      }
    }
  }
  return undefined;
};

const functionReturnExpression = (functionNode) => {
  if (!ts.isBlock(functionNode.body)) return unwrapExpression(functionNode.body);
  for (const statement of functionNode.body.statements) {
    if (ts.isReturnStatement(statement) && statement.expression) {
      return unwrapExpression(statement.expression);
    }
  }
  return undefined;
};

const propertyName = (name) => {
  if (ts.isIdentifier(name) || ts.isStringLiteralLike(name)) return name.text;
  return undefined;
};

const subtreeContainsApi = (root, api) => {
  let found = false;
  const visit = (node) => {
    if (found) return;
    const name = expressionName(node);
    if (matchesQualifiedName(name, api)) {
      found = true;
      return;
    }
    ts.forEachChild(node, visit);
  };
  visit(root);
  return found;
};

const subtreeContainsStringLiteral = (root, value) => {
  let found = false;
  const visit = (node) => {
    if (found) return;
    if (ts.isStringLiteralLike(node) && node.text === value) {
      found = true;
      return;
    }
    ts.forEachChild(node, visit);
  };
  visit(root);
  return found;
};

const checkPdfCapabilityContract = (sourceFile) => {
  const snapshot = findNamedFunction(sourceFile, "snapshotPdfCapabilities");
  const selector = findNamedFunction(sourceFile, "selectPdfJsVariant");
  if (!snapshot) fail("PDF loader is missing snapshotPdfCapabilities");
  if (!selector) fail("PDF loader is missing selectPdfJsVariant");

  const snapshotResult = functionReturnExpression(snapshot);
  if (!snapshotResult || !ts.isObjectLiteralExpression(snapshotResult)) {
    fail("snapshotPdfCapabilities must return an object literal for AST auditing");
  }

  const capabilityProperties = new Map();
  for (const api of REQUIRED_PDF_CAPABILITIES) {
    for (const property of snapshotResult.properties) {
      if (!ts.isPropertyAssignment(property)) continue;
      const name = propertyName(property.name);
      if (name && subtreeContainsApi(property.initializer, api)) {
        capabilityProperties.set(api, name);
        break;
      }
    }
  }

  const missingSnapshots = REQUIRED_PDF_CAPABILITIES.filter(
    (api) => !capabilityProperties.has(api),
  );
  if (missingSnapshots.length > 0) {
    fail(`snapshotPdfCapabilities does not probe: ${missingSnapshots.join(", ")}`);
  }

  const selectorResult = functionReturnExpression(selector);
  if (!selectorResult) fail("selectPdfJsVariant must return an auditable expression");
  const missingSelectorChecks = [];
  for (const [api, capability] of capabilityProperties) {
    if (!subtreeContainsApi(selectorResult, `capabilities.${capability}`)) {
      missingSelectorChecks.push(`${capability} (${api})`);
    }
  }
  if (missingSelectorChecks.length > 0) {
    fail(
      `selectPdfJsVariant does not require: ${missingSelectorChecks.join(", ")}`,
    );
  }

  return Object.fromEntries(capabilityProperties);
};

const extractPdfHeader = (source, path) => {
  const header = source.slice(0, 4096);
  const version = header.match(/pdfjsVersion\s*=\s*([^\s*]+)/u)?.[1];
  const build = header.match(/pdfjsBuild\s*=\s*([^\s*]+)/u)?.[1];
  if (!version || !build) fail(`Missing PDF.js version header: ${path}`);
  return { version, build };
};

export const checkPdfAssets = async (distDir) => {
  const root = resolve(distDir);
  const entries = {};

  for (const [name, path] of Object.entries(PDF_ASSETS)) {
    const bytes = await readRequiredFile(root, path);
    const source = bytes.toString("utf8");
    entries[name] = {
      ...assetMetrics(path, bytes),
      ...extractPdfHeader(source, path),
    };

    if (name.startsWith("legacy") && source.includes("sourceMappingURL=")) {
      fail(`Legacy PDF.js source map reference must be removed: ${path}`);
    }
    if (name.startsWith("legacy") && !source.includes(LEGACY_PDF_RUNTIME_MARKER)) {
      fail(`Legacy PDF.js is missing the Safari 15 runtime prelude: ${path}`);
    }
  }

  const versions = new Set(Object.values(entries).map(({ version }) => version));
  const builds = new Set(Object.values(entries).map(({ build }) => build));
  if (versions.size !== 1 || builds.size !== 1) {
    fail("PDF.js modern/legacy main-worker headers do not match");
  }

  const legacyRoot = join(root, "foliate-js/vendor/pdfjs/legacy");
  const legacyMaps = await listFiles(legacyRoot, (path) => path.endsWith(".map"));
  if (legacyMaps.length > 0) {
    fail(
      `Legacy PDF.js source maps must not be bundled: ${legacyMaps
        .map((path) => normalizePath(relative(root, path)))
        .join(", ")}`,
    );
  }

  const legacyRawBytes = entries.legacyMain.rawBytes + entries.legacyWorker.rawBytes;
  const legacyGzipBytes = entries.legacyMain.gzipBytes + entries.legacyWorker.gzipBytes;
  if (legacyRawBytes > SIZE_BUDGETS.legacyPdfRawBytes) {
    fail(
      `Legacy PDF.js raw size ${legacyRawBytes} exceeds ${SIZE_BUDGETS.legacyPdfRawBytes}`,
    );
  }
  if (legacyGzipBytes > SIZE_BUDGETS.legacyPdfGzipBytes) {
    fail(
      `Legacy PDF.js gzip size ${legacyGzipBytes} exceeds ${SIZE_BUDGETS.legacyPdfGzipBytes}`,
    );
  }

  for (const name of ["legacyMain", "legacyWorker"]) {
    const path = PDF_ASSETS[name];
    const source = (await readRequiredFile(root, path)).toString("utf8");
    const sourceFile = parseJavaScript(source, path);
    const errors = scanJavaScriptCompatibility(sourceFile, {
      restrictedApis: false,
      staticBlocks: true,
    });
    if (errors.length > 0) fail(`Legacy PDF.js is not Safari 15 syntax:\n${errors.join("\n")}`);
  }

  return {
    version: entries.modernMain.version,
    build: entries.modernMain.build,
    assets: entries,
    modern: {
      rawBytes: entries.modernMain.rawBytes + entries.modernWorker.rawBytes,
      gzipBytes: entries.modernMain.gzipBytes + entries.modernWorker.gzipBytes,
    },
    legacy: { rawBytes: legacyRawBytes, gzipBytes: legacyGzipBytes },
    budgets: {
      legacyRawBytes: SIZE_BUDGETS.legacyPdfRawBytes,
      legacyGzipBytes: SIZE_BUDGETS.legacyPdfGzipBytes,
    },
  };
};

const isInsideAtRule = (node, name, predicate = () => true) => {
  for (let parent = node.parent; parent; parent = parent.parent) {
    if (parent.type === "atrule" && parent.name === name && predicate(parent)) return true;
  }
  return false;
};

const declarationKey = (declaration) => {
  const rule = declaration.parent;
  if (!rule || rule.type !== "rule") return undefined;
  const contexts = [];
  for (let parent = rule.parent; parent; parent = parent.parent) {
    if (parent.type === "atrule" && parent.name !== "supports") {
      contexts.push(`@${parent.name} ${parent.params}`);
    }
  }
  contexts.reverse();
  return `${contexts.join("|")}::${rule.selector}::${declaration.prop}`;
};

const hasPreviousCompatibleDeclaration = (declaration, property, predicate) => {
  for (let node = declaration.prev(); node; node = node.prev()) {
    if (node.type === "decl" && node.prop === property && predicate(node.value)) return true;
  }
  return false;
};

const auditCssFile = (source, path) => {
  const root = postcss.parse(source, { from: path });
  const errors = [];
  const compatibleDeclarations = new Set();
  const propertyFallbacks = new Set();
  let modernColorFallbacks = 0;
  let propertyRegistrations = 0;
  let themeColors = 0;
  let legacyUnitFallbacks = 0;
  let textWrapFallbacks = 0;

  root.walkAtRules((atRule) => {
    if (atRule.name === "layer") errors.push(`${path} still contains @layer`);
    if (atRule.name === "container") {
      errors.push(`${path} contains @container without a Safari 15 fallback`);
    }
    if (atRule.name === "property") {
      propertyRegistrations += 1;
      const property = atRule.params.trim();
      if (property) propertyFallbacks.add(property);
    }
  });

  root.walkRules((rule) => {
    if (rule.selector.includes(":has(")) {
      errors.push(`${path} contains :has() without a Safari 15 fallback`);
    }
    if (rule.parent?.type === "rule") {
      errors.push(`${path} contains untransformed CSS nesting at ${rule.selector}`);
    }
  });

  root.walkDecls((declaration) => {
    const insideProperty = isInsideAtRule(declaration, "property");
    if (!insideProperty && declaration.prop.startsWith("--")) {
      propertyFallbacks.delete(declaration.prop);
    }

    const key = declarationKey(declaration);
    if (key && !MODERN_CSS_COLOR.test(declaration.value)) {
      compatibleDeclarations.add(key);
    }

    if (declaration.prop.startsWith("--color-") && !insideProperty) {
      themeColors += 1;
      if (MODERN_CSS_COLOR.test(declaration.value)) {
        errors.push(
          `${path} leaves theme color ${declaration.prop} as ${declaration.value}`,
        );
      }
    }

    if (MODERN_CSS_COLOR.test(declaration.value)) {
      const guarded = isInsideAtRule(
        declaration,
        "supports",
        (atRule) => MODERN_CSS_COLOR.test(atRule.params),
      );
      const priorFallback = hasPreviousCompatibleDeclaration(
        declaration,
        declaration.prop,
        (value) => !MODERN_CSS_COLOR.test(value),
      );
      if (!guarded && !priorFallback) {
        errors.push(
          `${path} has unguarded Safari 15-incompatible color in ${declaration.prop}`,
        );
      } else if (guarded && (!key || !compatibleDeclarations.has(key))) {
        errors.push(
          `${path} has guarded modern color without a prior compatible ${declaration.prop} fallback`,
        );
      } else {
        modernColorFallbacks += 1;
      }
    }

    if (MODERN_CSS_UNIT.test(declaration.value)) {
      const hasFallback = hasPreviousCompatibleDeclaration(
        declaration,
        declaration.prop,
        (value) => !MODERN_CSS_UNIT.test(value),
      );
      if (!hasFallback) {
        errors.push(
          `${path} has ${declaration.prop}: ${declaration.value} without a vh/vw/em fallback`,
        );
      } else {
        legacyUnitFallbacks += 1;
      }
    }

    if (declaration.prop === "text-wrap") {
      const expected = declaration.value.trim() === "nowrap" ? "nowrap" : "normal";
      const hasFallback = hasPreviousCompatibleDeclaration(
        declaration,
        "white-space",
        (value) => value.trim() === expected,
      );
      if (!hasFallback) {
        errors.push(
          `${path} has text-wrap: ${declaration.value} without white-space: ${expected}`,
        );
      } else {
        textWrapFallbacks += 1;
      }
    }
  });

  for (const property of propertyFallbacks) {
    errors.push(`${path} registers ${property} without a non-@property fallback`);
  }

  return {
    errors,
    modernColorFallbacks,
    propertyRegistrations,
    themeColors,
    legacyUnitFallbacks,
    textWrapFallbacks,
  };
};

export const auditCss = async (distDir) => {
  const root = resolve(distDir);
  const files = await listFiles(root, (path) => path.endsWith(".css"));
  if (files.length === 0) fail(`No final CSS found under ${root}`);

  const result = {
    files: files.length,
    modernColorFallbacks: 0,
    propertyRegistrations: 0,
    themeColors: 0,
    legacyUnitFallbacks: 0,
    textWrapFallbacks: 0,
  };
  const errors = [];

  for (const path of files) {
    const source = await readFile(path, "utf8");
    const audit = auditCssFile(source, normalizePath(relative(root, path)));
    errors.push(...audit.errors);
    for (const key of Object.keys(result)) {
      if (key !== "files") result[key] += audit[key];
    }
  }

  if (result.themeColors === 0) {
    errors.push("Final CSS contains no auditable --color-* theme values");
  }
  if (errors.length > 0) fail(`Safari 15 CSS audit failed:\n${errors.join("\n")}`);
  return result;
};

const auditJavaScriptFiles = async (
  root,
  files,
) => {
  const errors = [];
  let rawBytes = 0;
  let gzipBytes = 0;

  for (const path of files) {
    const source = await readFile(path, "utf8");
    const relativePath = normalizePath(relative(root, path));
    const sourceFile = parseJavaScript(source, relativePath);
    errors.push(...scanJavaScriptCompatibility(sourceFile));

    const bytes = Buffer.from(source);
    rawBytes += bytes.byteLength;
    gzipBytes += gzipSync(bytes, { level: 9 }).byteLength;
  }

  if (errors.length > 0) fail(`Safari 15 JavaScript audit failed:\n${errors.join("\n")}`);
  return { files: files.length, rawBytes, gzipBytes };
};

const verifyReaderTransformManifest = async (root, readerFiles) => {
  const bytes = await readRequiredFile(root, READER_TRANSFORM_MANIFEST);
  let manifest;
  try {
    manifest = JSON.parse(bytes.toString("utf8"));
  } catch {
    fail(`${READER_TRANSFORM_MANIFEST} is not valid JSON`);
  }
  if (manifest.schemaVersion !== 1 || manifest.target !== SAFARI_15_TARGET) {
    fail(`${READER_TRANSFORM_MANIFEST} does not target ${SAFARI_15_TARGET}`);
  }

  const expectedFiles = readerFiles
    .map((path) => normalizePath(relative(root, path)))
    .sort();
  const manifestedFiles = Object.keys(manifest.files ?? {}).sort();
  if (
    expectedFiles.length !== manifestedFiles.length
    || expectedFiles.some((path, index) => path !== manifestedFiles[index])
  ) {
    fail(`${READER_TRANSFORM_MANIFEST} does not cover every Reader .js asset`);
  }

  for (const path of expectedFiles) {
    const source = await readRequiredFile(root, path);
    const hash = createHash("sha256").update(source).digest("hex");
    if (manifest.files[path] !== hash) {
      fail(`${path} changed after the Safari 15 Reader transform`);
    }
  }
};

const checkPdfLoader = async (root) => {
  const pdfPath = "foliate-js/pdf.js";
  const compatPath = "foliate-js/pdf-compat.js";
  const pdfSource = (await readRequiredFile(root, pdfPath)).toString("utf8");
  const compatSource = (await readRequiredFile(root, compatPath)).toString("utf8");
  const pdfFile = parseJavaScript(pdfSource, pdfPath);
  const compatFile = parseJavaScript(compatSource, compatPath);

  const staticImports = checkNoStaticModernPdfImport(pdfFile);
  if (staticImports.length > 0) fail(staticImports.join("\n"));

  const { dynamicImports, literals } = collectPdfLoaderPaths(compatFile);
  for (const path of [PDF_IMPORTS.modernMain, PDF_IMPORTS.legacyMain]) {
    if (!dynamicImports.has(path)) fail(`PDF loader is missing literal dynamic import ${path}`);
  }
  for (const path of [PDF_IMPORTS.modernWorker, PDF_IMPORTS.legacyWorker]) {
    if (!literals.has(path)) fail(`PDF loader is missing paired Worker path ${path}`);
  }

  for (const [variant, path] of [
    ["modern", PDF_IMPORTS.modernWorker],
    ["legacy", PDF_IMPORTS.legacyWorker],
  ]) {
    const initializer = findVariableInitializer(compatFile, `${variant}WorkerUrl`);
    if (!initializer || !subtreeContainsStringLiteral(initializer, path)) {
      fail(`PDF loader does not pair ${variant} main with Worker ${path}`);
    }
  }
  return checkPdfCapabilityContract(compatFile);
};

export const checkReaderCompatibility = async ({ distDir = resolve("dist") } = {}) => {
  const root = resolve(distDir);
  const readerRoot = join(root, "foliate-js");
  const appAssetsRoot = join(root, "assets");

  try {
    await stat(join(readerRoot, "node_modules"));
    fail("Nested foliate-js/node_modules must not be present in the production bundle");
  } catch (error) {
    if (!(error && typeof error === "object" && error.code === "ENOENT")) throw error;
  }

  const capabilityContract = await checkPdfLoader(root);
  const pdfjs = await checkPdfAssets(root);
  pdfjs.capabilityContract = capabilityContract;

  const readerFiles = await listFiles(readerRoot, (path) => path.endsWith(".js"));
  if (readerFiles.length === 0) fail(`No Reader JavaScript found under ${readerRoot}`);
  await verifyReaderTransformManifest(root, readerFiles);
  const reader = await auditJavaScriptFiles(root, readerFiles);
  reader.transformManifest = READER_TRANSFORM_MANIFEST;

  const appFiles = await listFiles(appAssetsRoot, (path) => path.endsWith(".js"));
  if (appFiles.length === 0) fail(`No app-shell JavaScript found under ${appAssetsRoot}`);
  const appShell = await auditJavaScriptFiles(root, appFiles);

  const css = await auditCss(root);

  const zipPath = "foliate-js/vendor/zip.js";
  const zipBytes = await readRequiredFile(root, zipPath);
  const zip = assetMetrics(zipPath, zipBytes);
  const zipGzipIncrementBytes = Math.max(
    0,
    zip.gzipBytes - SIZE_BUDGETS.zipBaselineGzipBytes,
  );
  if (zipGzipIncrementBytes > SIZE_BUDGETS.zipGzipIncrementBytes) {
    fail(
      `ZIP fallback gzip increment ${zipGzipIncrementBytes} exceeds `
      + `${SIZE_BUDGETS.zipGzipIncrementBytes}`,
    );
  }

  const report = {
    schemaVersion: 1,
    target: SAFARI_15_TARGET,
    pdfjs,
    zip: {
      ...zip,
      baselineGzipBytes: SIZE_BUDGETS.zipBaselineGzipBytes,
      gzipIncrementBytes: zipGzipIncrementBytes,
      gzipIncrementBudgetBytes: SIZE_BUDGETS.zipGzipIncrementBytes,
    },
    reader,
    appShell,
    css,
    releaseOnly: {
      dmgIncrementBudgetBytes: SIZE_BUDGETS.dmgIncrementBytes,
      status: "requires-package-comparison",
    },
  };

  await writeFile(
    join(root, SIZE_REPORT_FILE),
    `${JSON.stringify(report, null, 2)}\n`,
    "utf8",
  );
  return report;
};

const distArgument = (arguments_) => {
  const index = arguments_.indexOf("--dist");
  if (index === -1) return resolve("dist");
  if (!arguments_[index + 1]) throw new Error("--dist requires a directory");
  return resolve(arguments_[index + 1]);
};

const isMain = process.argv[1]
  && pathToFileURL(resolve(process.argv[1])).href === import.meta.url;

if (isMain) {
  try {
    const report = await checkReaderCompatibility({
      distDir: distArgument(process.argv.slice(2)),
    });
    console.log(
      `Reader compatibility: PDF.js ${report.pdfjs.version}; `
      + `legacy ${report.pdfjs.legacy.rawBytes} raw / `
      + `${report.pdfjs.legacy.gzipBytes} gzip bytes; `
      + `ZIP +${report.zip.gzipIncrementBytes} gzip bytes.`,
    );
    console.log(`Size report: dist/${SIZE_REPORT_FILE}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
