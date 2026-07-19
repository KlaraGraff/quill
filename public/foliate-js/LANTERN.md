# Lantern vendored Foliate.js

This directory is a vendored copy of Foliate.js used by Lantern's reader. It was
converted from the `KlaraGraff/foliate-js` commit
`112eb278e4fc04f48f494dd71b213b2536bf4062` on 2026-07-17 so a Lantern checkout
does not require a Git submodule.

The original project remains MIT-licensed; see `LICENSE`. This copy includes
Lantern's iframe-load reliability fixes formerly maintained on
`lantern/iframe-load-timeout`. Make future reader-engine updates directly in
this directory and commit them with the Lantern change that requires them.

## Reader compatibility assets

Lantern supports the Safari 15 WKWebView shipped with macOS 12. Keep these
local compatibility paths when updating Foliate or its dependencies:

- `rollup/zip.js` uses zip.js's `zip-core-native.js`. It prefers platform
  Compression Streams and falls back to zip.js's pure-JavaScript streaming
  DEFLATE codec when `deflate-raw` is unavailable.
- `epub.js` uses local `groupByObject` and `groupByMap` helpers instead of the
  Safari 17.4 `Object.groupBy` and `Map.groupBy` APIs.
- PDF.js modern and legacy main/worker files must come from the same locked
  `pdfjs-dist` version. `pdf-compat.js` capability-selects a matched pair before
  importing either build; never add a static import of the modern module.
- PDF.js 5.5.207's official legacy files still contain a class static block and
  expect `structuredClone`. The Rollup copy step lowers the static block, and
  `pdf-compat.js` installs a legacy-only synchronous clone fallback before
  import. Revalidate both requirements when updating PDF.js.
- Do not ship legacy PDF.js source maps or duplicate CMaps, standard fonts, or
  CSS under the legacy directory.

Regenerate vendored assets after changing the nested dependency lockfile or
Rollup inputs:

```bash
npm --prefix public/foliate-js ci
npm --prefix public/foliate-js run build
```

The root `npm run build` then transpiles copied Reader `.js` files for Safari
15 and runs `scripts/check-reader-compat.mjs`. Commit source and generated
vendor changes together, and keep the compatibility tests green.
