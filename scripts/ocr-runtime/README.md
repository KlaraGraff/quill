# Lantern OCR runtime

This directory builds the optional OCR runtime described by
`docs/impls/scanned-pdf-ocr-v1-parallel-plan.md` W2.

The package is deliberately narrow:

- macOS arm64 (minimum macOS 12) and Windows x64 only;
- OCRmyPDF with the pypdfium rasterizer and Tesseract;
- `tessdata_fast` `chi_sim+eng` only;
- no Vision backend, best models, updater signature, or one-file freeze.

Each archive contains a relocatable `lantern-ocr-runtime/` directory. The
stable executable is `bin/lantern-ocr` on macOS and
`bin/lantern-ocr.exe` on Windows. `runtime.json` records the same entrypoint,
the bundled language/model hashes, and the runtime version.

Normal OCR invocations reserve stdout for one JSON object per line. Human
diagnostics remain on stderr. The supported maintenance commands are:

```text
lantern-ocr --version
lantern-ocr --self-test
```

`--self-test` checks both language files, runs the bundled image-only PDF
fixture through the production OCR flags, validates the result with PDFium,
and verifies every stdout line is valid JSON.

The GitHub workflow builds and smoke-tests both packages. A tag named
`ocr-runtime-v<version>` publishes the archives, per-platform HTTPS/SHA-256
manifests, SBOMs, notices, and checksum files to a GitHub Release. Manual runs
only upload workflow artifacts.
