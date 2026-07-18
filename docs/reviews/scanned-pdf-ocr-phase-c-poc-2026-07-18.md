# Scanned PDF OCR Phase C PoC Report

> Status: **IN PROGRESS — no final go/no-go decision**
>
> Date: 2026-07-18
>
> Plan: [scanned PDF OCR pipeline](../impls/scanned-pdf-ocr-pipeline.md)

## 1. Scope and Limits

This report records reproducible evidence collected for the Phase C backend decision. It is not a release qualification report.

The repository currently contains one PDF sample under `测试文件/`. It is a 253-page, 15,097,846-byte digital/mixed PDF, not a complete scanned book:

- 108,730 extracted text characters;
- 11 pages with no text layer;
- 24 pages with fewer than 20 characters;
- 22 page images;
- filename and source indicate Z-Library provenance, so it is not suitable as redistributable test corpus.

It is useful for a controlled rasterization smoke test and for mixed-PDF `--mode skip` behavior. It does not prove accuracy on noisy scans, skew, complex columns, footnotes, or a legal 10–20-book corpus.

## 2. Environment

```text
macOS 26.5.2, arm64 Apple Silicon
Python 3.14.6
OCRmyPDF 17.8.1
Tesseract 5.5.2
pypdfium2 5.12.1 in the PoC virtualenv
Lantern pdfium-render 0.9.1
Bundled Lantern PDFium: about 6.8 MiB, Mach-O minos 12.0
```

The OCRmyPDF Python environment is isolated at `/private/tmp/lantern-ocr-poc/ocrmypdf-venv`. Tesseract was installed with Homebrew for this local PoC; the repository does not yet contain a distributable runtime package, SBOM, or Windows package.

## 3. OCRmyPDF Ghostscript-Free Smoke Test

The fixed command was run with an empty environment and a restricted `PATH` containing only the PoC Python runtime, Homebrew binaries, `/usr/bin`, and `/bin`:

```text
--mode skip
--output-type pdf
--rasterizer pypdfium
--optimize 0
--fast-web-view 999999
--jobs 1
-l chi_sim+eng
```

The environment explicitly confirmed that `gs`, `verapdf`, `unpaper`, `pngquant`, and `jbig2enc` were absent. OCRmyPDF 17.8.1 successfully produced a searchable PDF, so the ordinary PDF path does not require Ghostscript on this machine.

### 3.1 Controlled benchmark

The page-4 image was rendered from the existing PDF at 180 DPI and wrapped into a one-page image-only PDF. The original PDF text layer was used only as approximate ground truth. CER below is therefore a clean digital-page proxy, not a scan-quality guarantee.

| Case | Elapsed | Peak process-tree RSS | Output | CER proxy | Sequence ratio |
|---|---:|---:|---:|---:|---:|
| fast `chi_sim+eng` | 2.13 s | 282 MiB | 337 KiB | 5.35% | 94.84% |
| fast `eng+chi_sim` | 2.06 s | 275 MiB | 337 KiB | 5.35% | 94.84% |
| best `chi_sim+eng` | 3.13 s | 335 MiB | 337 KiB | 5.71% | 94.55% |
| best `eng+chi_sim` | 3.28 s | 328 MiB | 337 KiB | 10.34% | 92.87% |

The best model was not automatically better on this page. The quality mode must remain a user-selectable profile, not a promise that `best` is always more accurate.

The `tessdata_best/chi_sim.traineddata` config declares `tessedit_load_sublangs chi_sim_vert`; without `chi_sim_vert.traineddata`, Tesseract emits a load warning. The high-accuracy Chinese pack therefore needs the vertical model as a dependency, adding about 12.4 MiB beyond the earlier fast/best estimate. The final package manifest must declare this explicitly.

OCRmyPDF also warned that no installed font had CJK glyphs and fell back to a glyphless invisible layer. Text remained searchable/copyable, but a production package must either ship an appropriate licensed font or deliberately implement a glyphless ToUnicode strategy.

## 4. Apple Vision Evidence

The same 180 DPI bilingual page was processed by `VNRecognizeTextRequest` with:

```text
recognitionLevel = accurate
recognitionLanguages = ["zh-Hans", "en-US"]
usesLanguageCorrection = true
```

| Revision | Observations | Characters | OCR time | CER proxy | Sequence ratio |
|---|---:|---:|---:|---:|---:|
| Rev2 | 30 | 881 | 0.503 s | 2.02% | 98.38% |
| Rev3 | 30 | 881 | 0.437 s | 2.02% | 98.38% |

Vision also returned substring-level quadrilateral coordinates for a Chinese substring (`青年`), not only line boxes. This is sufficient to investigate a PDF text-layer graft.

### 4.1 Corrected minimum-system conclusion

The earlier plan wording that Chinese recognition requires Revision 3/macOS 13+ was incorrect. The SDK header states:

- Revision 2, macOS 11+, supports Chinese in `accurate` mode;
- Revision 2 `fast` mode does not support Chinese;
- Revision 3, macOS 13+, improves rotation, handwriting, and language coverage.

For a macOS 12 baseline, the safe Vision choice is `accurate` + Revision 2. Revision 3 is an optional macOS 13+ improvement. This conclusion still needs a real macOS 12 runtime test and signed-app test.

The Vision request failed inside the Codex execution sandbox with a generic Objective-C error, then succeeded in a permitted non-sandbox process. Lantern does not currently enable the App Sandbox entitlement, so this is not evidence that the shipped app fails; a real signed `.app` probe remains required.

## 5. Vision + Existing PDFium Graft

A temporary Rust PoC used the repository's existing `pdfium-render 0.9.1` and bundled `libpdfium.dylib` to append 30 invisible text objects to the one-page image PDF:

- CID TrueType font loaded through `PdfFonts::load_true_type_from_file(..., true)`;
- line text objects created with `create_text_object`;
- `PdfPageTextRenderMode::Invisible`;
- horizontal and vertical scaling to Vision boxes;
- `PdfPageContentRegenerationStrategy::Manual` followed by one regeneration;
- output saved through PDFium.

Using a 58 KiB font subset containing only the page's recognized characters produced a 368 KiB output PDF. Verification results:

- page count unchanged;
- page size unchanged (`612 x 792 pt`);
- `pdftotext` extracted 911 characters;
- normalized CER proxy and sequence ratio matched Vision (2.02% / 98.38%);
- PyMuPDF render pixel hash was identical before and after grafting;
- changed render channels: `0`;
- current PDF.js extracted one logical text item per Vision line and exposed 30 text spans;
- a programmatic DOM Range spanning three Chinese lines returned the expected continuous text.

This is strong evidence that Vision + the existing PDFium can produce a standard searchable PDF without adding a Python runtime on macOS. It does not yet prove complex PDF structure preservation, CropBox/rotation mapping, font licensing for a shipped font, or mouse selection in the full Lantern Reader.

## 6. Provisional Decision

The evidence changes the backend ranking:

1. **macOS:** Vision + existing PDFium graft is the strongest candidate and should be implemented as the next narrow spike.
2. **Windows:** Windows.Media.Ocr remains untested; no Windows host or VM is available in this environment. Keep OCRmyPDF/Tesseract as the cross-platform fallback candidate.
3. **Fallback:** OCRmyPDF remains valuable for Windows and for macOS fallback if signed-app Vision or PDFium grafting fails.
4. **Realtime Tesseract.js overlay:** still rejected as the primary route; the standard-PDF output route is now demonstrably viable.

This is not yet a final go decision because the following gates remain open:

- legal 10–20-book corpus with real scans;
- Vision on noisy, skewed, low-resolution and multi-column pages;
- signed Lantern `.app` probe on macOS 12 and current macOS;
- CJK font/ToUnicode licensing and replacement strategy;
- CropBox, MediaBox, rotation, annotations, forms, bookmarks and metadata preservation;
- full Foliate/PDF.js click, double-click, drag selection, highlight and AI lookup;
- Windows.Media.Ocr and Windows 11 package validation;
- final runtime/package sizes, SBOM and third-party notices.

## 7. Reproducible PoC Artifacts

Temporary artifacts are under `/private/tmp/lantern-ocr-poc/` and `/private/tmp/lantern-vision-graft-poc/`:

- `vision_ocr.swift`: Vision JSON-lines/JSON report with normalized line boxes;
- `run_ocrmypdf_matrix.py`: isolated OCRmyPDF benchmark runner and RSS sampler;
- `compare_ocr.py`: normalized edit-distance comparison;
- `pdfjs_text_check.mjs`: current vendored PDF.js text-content check;
- `models/fast` and `models/best`: downloaded official Tesseract model files;
- `target/release/lantern-vision-graft-poc`: temporary PDFium graft executable.

No temporary PoC file is part of the Lantern repository.
