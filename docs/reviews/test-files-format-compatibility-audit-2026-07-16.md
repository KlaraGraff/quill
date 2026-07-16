# 测试文件格式兼容性审计

- **日期:** 2026-07-16
- **范围:** `/Users/lijianwei/vibecoding/Lantern/测试文件/` 下 5 个真实书籍文件
- **结论:** 全部可导入，但**并非全部达到预期显示效果**。EPUB / TXT 完整；PDF 可渲染但本文件文本层薄弱；MOBI / AZW3 可显示正文但按设计丢失核心 AI 阅读能力。

## 1. 阅读器设计上支持的格式

导入白名单 `IMPORTABLE_BOOK_EXTENSIONS`（`src-tauri/src/commands/books/mod.rs:78`）:

```
epub, pdf, txt, md, markdown, html, htm, mobi, azw, azw3, fb2, fbz, cbz
```

导入分派 `do_import_from_path`（`src-tauri/src/commands/books/import.rs:509`）把它们归入四条渲染路径：

| 渲染路径 | 覆盖格式 | 渲染引擎 |
|---|---|---|
| `epub` | epub | foliate-js EPUB |
| `pdf` | pdf | pdfium 抽取 + foliate-js PDF (pdf.js) |
| `text` | txt / md / markdown / html / htm | 后端转换为分章 JSON，前端自绘文本阅读器 |
| 原生（native） | mobi / azw / azw3 / fb2 / fbz / cbz | foliate-js MOBI/KF8/FB2/漫画解析器 |

各路径能力由 `getReaderCapabilities`（`src/components/reader-settings.ts:57`）定义。**关键差异：只有 `epub` 与 `text` 拥有完整能力集；`mobi/azw/azw3/fb2/fbz` 是最弱的一档。**

| 能力 | epub | text | pdf | mobi/azw/azw3/fb2 | cbz |
|---|---|---|---|---|---|
| 选中文本 (selection) | ✅ | ✅ | ✅ | ❌ | ❌ |
| 手动标注/高亮 | ✅ | ✅ | ✅ | ❌ | ❌ |
| 生词标记 (word markers) | ✅ | ✅ | ❌ | ❌ | ❌ |
| CFI 导航（书签/生词面板） | ✅ | ✅ | ✅ | ❌ | ❌ |
| 排版设置 (reflow) | ✅ | ✅ | ❌ | ✅ | ❌ |
| 缩放 (zoom) | ❌ | ❌ | ✅ | ❌ | ❌ |

> AI 查词、翻译、划词解释等均建立在 selection 之上；因此 **selection = ❌ 意味着 Lantern 的 AI 阅读能力对该书整体不可用**。

## 2. 逐个文件核验

对每个文件做了字节级头部/编码检查，判断能否被导入检测识别、走哪条渲染路径、以及预期显示效果。

### 2.1 谈美…epub — 15.1 MB
- **检测:** `PK\x03\x04` + `mimetype == application/epub+zip` + `META-INF/container.xml` → `ImportFormat::Epub`。
- **能力:** 完整（selection / 标注 / 生词 / CFI / reflow）。
- **判定:** ✅ **达到预期。** 这是 Lantern 的一等公民格式。

### 2.2 The Alchemist牧羊少年奇幻之旅.txt — 210 KB
- **检测:** 无 BOM、无 NUL 字节、扩展名 `txt` → `ImportFormat::Txt`；UTF-8 解码通过（`decode_txt` 用 `chardetng` 兜底）。
- **大小:** 210 KB « 25 MB 上限（`MAX_TEXT_IMPORT_BYTES`），通过。
- **路径:** 导入后置为 `preparation_state=pending`，异步 `schedule_text_book_preparation` 切章为可回流文本（`render_format=text`）。
- **能力:** 完整。
- **判定:** ✅ **达到预期**（导入后需等待后台 preparation 完成，首次打开可能短暂显示"准备中"）。

### 2.3 被讨厌的勇气…pdf — 14.4 MB
- **检测:** 头部 `%PDF-` → `ImportFormat::Pdf`；pdfium 抽取封面/元数据后按 pdf 渲染。
- **文件画像:** Producer = `itext-paulo`（iText 重新封装），内含 **773 个 /Image 对象**，原始字节中文本绘制算子（`BT…ET`）极少。强烈提示正文以**扫描图像/图片页**为主，文本层稀薄或缺失。
- **预期显示:** 页面**能正常渲染出来**（图像照常显示），分页/缩放/滚动可用。
- **风险:** 若文本层缺失，则：
  - 选中/复制得到空或错乱内容；
  - 触发 `pdfTextLayerNotice`（`src/pages/Reader.tsx:1353`）提示无文本层；
  - AI 查词/翻译/解释在这些页上不可用；
  - PDF 本就无 reflow、无生词标记。
- **判定:** ⚠️ **部分达到。** 视觉渲染 OK，但本文件很可能是图片型 PDF，AI 阅读能力大幅受限。需在真机打开确认文本层是否存在（字节级启发式无法穿透压缩流，仅为强提示）。

### 2.4 西学三书…azw3 — 7.4 MB
- **检测:** 扩展名 `azw3` + 偏移 60–68 为 `BOOKMOBI` → `ImportFormat::Mobi`（AZW/AZW3 复用 Foliate MOBI 解析器，保留原扩展名）。
- **文件画像:** MOBI header version = **8（KF8）**，压缩类型 **17480（HUFF/CDIC）**。foliate-js `mobi.js` 支持 KF8 与 HUFF/CDIC 解压。
- **预期显示:** 正文**能渲染**，支持 reflow 排版、分栏、连续滚动。
- **按设计缺失:** selection、手动标注、高亮、生词标记、CFI 导航、书签面板、生词面板、**全部 AI 划词能力**（见 `getReaderCapabilities` 的 mobi 分支）。
- **判定:** ⚠️ **部分达到。** 能读，但退化为"纯阅读"，丢掉 Lantern 的核心卖点。

### 2.5 重读20世纪中国小说…mobi — 6.9 MB
- **检测:** 扩展名 `mobi` + `BOOKMOBI` → `ImportFormat::Mobi`。
- **文件画像:** MOBI header version = **6（MOBI6，旧格式）**，压缩类型 1（无压缩）。走 foliate-js `MOBI6` 解析器。
- **预期显示:** 同 AZW3——正文可渲染，能力集与 mobi 分支一致。
- **判定:** ⚠️ **部分达到。** 同 2.4。

## 3. 阅读进度/续读说明

`relocate` 事件在拿到合成 CFI 时即 `queueReadingProgress`（`src/pages/reader/useFoliateView.ts:376`），foliate 对 MOBI/KF8 也会生成合成 CFI，故**续读位置对 MOBI/AZW3 仍能保存与恢复**。`supportsCfiNavigation=false` 只关闭书签/生词等**依赖 CFI 的 UI 面板**，不影响基本续读。

## 4. 汇总

| 文件 | 大小 | 检测格式 | 渲染路径 | 能否打开 | 达到预期显示 |
|---|---|---|---|---|---|
| 谈美…epub | 15.1 MB | epub | epub | ✅ | ✅ 完整 |
| The Alchemist…txt | 210 KB | txt | text | ✅ | ✅ 完整（需后台准备） |
| 被讨厌的勇气…pdf | 14.4 MB | pdf | pdf | ✅ | ⚠️ 渲染 OK，疑似图片型 PDF，AI/选中受限 |
| 西学三书…azw3 | 7.4 MB | mobi(KF8 v8) | native | ✅ | ⚠️ 仅纯阅读，无选中/AI/标注/书签 |
| 重读20世纪中国小说…mobi | 6.9 MB | mobi(v6) | native | ✅ | ⚠️ 仅纯阅读，无选中/AI/标注/书签 |

**总体:** 5 个文件全部可导入并显示正文；但仅 EPUB、TXT 达到"预期显示效果（完整能力）"。PDF 受本文件文本层质量制约；MOBI/AZW3 按当前设计天然缺失 selection 及其上的全部 AI 阅读能力。

## 5. 建议

1. **导入时暴露能力差异**：对 mobi/azw/azw3/fb2/cbz 在书库/首次打开处提示"该格式为受限阅读模式，暂不支持划词、AI 查词与标注"，避免用户误以为功能失效。
2. **考虑 MOBI/AZW3 → EPUB 转换管线**：若要让这类文件享受完整 AI 能力，最彻底的方案是导入时转换为 EPUB（类似现有 TXT→text 的 preparation 流程），而非直接交给 foliate 原生解析。
3. **PDF 文本层探测**：导入阶段用 pdfium 抽样判断是否存在可选文本，若为纯图片 PDF 则提前提示"扫描版，AI 划词不可用"，而非等运行时 `pdfTextLayerNotice`。
4. **验证方式**：本审计基于字节级静态检查（头部魔数、MOBI 版本、PDF 算子/图像计数、TXT 编码）与代码路径走读，未在真机运行时逐本打开。PDF 文本层的最终结论建议在应用内实测确认。
