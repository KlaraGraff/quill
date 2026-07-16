# 格式规整管线（Format Normalization Pipeline）

> **这是一份"活文档"（living doc）。** 它同时记录**计划**与**当前进度**，任何接手者都应先读本文件，再看"进度看板"确认下一步。每完成一项，请更新对应勾选框与"变更日志"。

- **作者/发起:** Claude Code 会话（2026-07-16）
- **状态:** 🟡 进行中（Phase 0 完成，Phase 1 未开始）
- **关联审计:** [`docs/reviews/test-files-format-compatibility-audit-2026-07-16.md`](../reviews/test-files-format-compatibility-audit-2026-07-16.md)

## 1. 背景与目标

Lantern 的完整能力（划词选中、AI 查词/翻译/解释、手动标注、生词标记、CFI 书签/生词面板）只在 `render_format ∈ {epub, text}` 时可用（见 `getReaderCapabilities`，[`src/components/reader-settings.ts:57`](../../src/components/reader-settings.ts)）。

当前短板：
- **MOBI / AZW / AZW3**：走 `do_import_native`，原样交给 foliate 原生解析，`render_format` 记为 mobi/azw3 → 退化为"纯阅读"，selection 及其上的全部 AI 能力不可用。
- **扫描版 PDF**：无文本层，selection 得到空内容，AI 划词不可用（本次实测的 `被讨厌的勇气.pdf` 疑似此类）。

**目标：** 建立一条统一的"导入时把非一等格式规整为 EPUB"的后台管线，让上述格式最终以 `render_format=epub` 呈现，从而**无需改前端渲染逻辑**即获得完整 AI 能力。

**非目标：** 不改变 EPUB / TXT / 原生可读 PDF 的既有路径；不追求 100% 保真转换（排版可有损，正文与结构优先）。

## 2. 现状与关键代码位置（接手者必读）

| 关注点 | 位置 |
|---|---|
| 导入分派 | `do_import_from_path` → [`src-tauri/src/commands/books/import.rs:509`](../../src-tauri/src/commands/books/import.rs) |
| 原生格式导入 | `do_import_native` → `import.rs:224` |
| TXT 管线（可复用的状态机范式） | [`src-tauri/src/commands/books/text_prepare.rs`](../../src-tauri/src/commands/books/text_prepare.rs) |
| 导入后调度 | `import_user_selected_path` → `import.rs:440`（现仅对 `render_format=="text"` 调 `schedule_text_book_preparation`） |
| 准备状态枚举 | 前端 `preparation_state: "pending"｜"preparing"｜"ready"｜"failed"`（[`src/hooks/useBooks.ts:16`](../../src/hooks/useBooks.ts)）；后端默认 `default_preparation_state()="ready"`（[`books/mod.rs:185`](../../src-tauri/src/commands/books/mod.rs)） |
| 前端准备中/失败 UI | `BookGrid.tsx` / `BookList.tsx`（进度覆盖层 + 重试）|
| 阅读器 init 超时 | `useFoliateView.ts:510` 的 `view.init` + `withTimeout(..., "READER_INIT_TIMEOUT")` |
| foliate submodule fork | `public/foliate-js`（fork: `yicheng47/foliate-js`）——可改 paginator |
| AI provider（OCR 用） | [`src-tauri/src/ai/`](../../src-tauri/src/ai/) |
| 派生物不应进 iCloud 同步 | 参考封面/索引的排除处理，`src-tauri/src/sync/` |

**转换产物落盘约定（提案，待 Phase 1 定稿）：** 原文件仍存 `books/{slug}_{id}.{ext}`，转换出的 EPUB 存 `prepared/{id}.converted.epub`（与 text 管线的 `prepared/{id}.v3.json` 同目录风格）。`render_format` 改为 `epub`，`source_format` 保留原始（mobi/azw3/pdf），`source_file_path` 指向原文件，`file_path` 指向转换产物。

## 3. 分阶段计划（Phases）

### Phase 0 — 阅读器健壮性（先止血）✅ 已完成
- [x] **Layer A：`view.init()` 超时后，若带 saved location，清位置重试一次 `showTextStart`。** 命中"坏 CFI 导致 init 挂起"这一最可能主因；对其他情况无回归。改动：`useFoliateView.ts:510` 附近的 try/catch 重试。
- [ ] **Layer B：给 foliate paginator 的 iframe `load` promise 加超时。** 让卡死的子资源 reject 而非永久挂起。改动点：`public/foliate-js/paginator.js` 的 `load()`（`~253`，`new Promise(resolve => { iframe.addEventListener('load', …) })`）——加一个 `setTimeout` reject 或 fallback。**注意：这是 submodule，需在 fork 仓库提交后更新指针；本仓库单独一次 commit 更新 submodule sha。**
- [ ] 复现验证：拿一本带失效 CFI 的 epub 实测 Layer A 生效（当前会话无法跑 GUI，需真机）。

> **⚠️ 待办 / 悬而未决：** 本次排查发现活动数据库（iCloud `~/Library/Mobile Documents/com~apple~CloudDocs/quill`）中**并无谈美.epub**，且 sync 日志持续报 `SYNC_BLOB_PATH_INVALID` / `rejecting invalid event log`。谈美.epub 卡死的**触发源尚未 100% 复现确认**（Layer A 是基于"坏 CFI"最可能主因的止血，非确诊修复）。接手者应：① 让用户在干净库里重新导入谈美.epub 复现；② 抓前端 DevTools console（`Failed to initialize foliate-js` / `READER_*_TIMEOUT`）确诊；③ 若确诊是首屏 iframe 而非 CFI，则 Layer B 才是根治。**同步损坏是独立问题，另开工单。**

### Phase 1 — 管线骨架（与具体转换器解耦）⬜ 未开始
- [ ] 新增 `render_format=="epub" && source_format ∈ {mobi,azw,azw3,pdf(scanned)}` 的导入分支：置 `preparation_state="pending"`，落盘约定见 §2。
- [ ] 定义 `CONVERSION_VERSION` 常量（类比 `TEXT_DOCUMENT_VERSION`），用于产物失效重算。
- [ ] 抽象 `schedule_conversion(app, book_id)` + 状态机（pending→preparing→ready/failed），复用 `text_prepare.rs` 的调度/重试/恢复骨架（`resume_interrupted_*`、`retry_*`）。
- [ ] 前端：让 `BookGrid`/`BookList` 的"准备中/失败/重试"覆盖层对这些书同样生效（当前判断写死 `render_format === "text"`，需放宽）。
- [ ] Reader 端：`retry_text_book_preparation` 之外，为转换类书提供 `retry_conversion`（`Reader.tsx:1057` 的重试分支需按类型分派）。
- [ ] sync：转换产物 `prepared/*.converted.epub` 加入同步排除名单（本地派生物，不上传 iCloud）。
- [ ] 单元测试：状态机转移、产物版本失效、导入回滚（`ImportFileCleanup` 语义）。

### Phase 2 — MOBI / AZW3 → EPUB 转换器 ⬜ 未开始
两条候选路线，**先做 A（务实），B 作为后续无依赖方案：**
- [ ] **路线 A（外部工具）：** 检测系统 Calibre 的 `ebook-convert`，`ebook-convert in.azw3 out.epub`。检测不到则**优雅降级**回当前 foliate 原生只读模式（保持 mobi/azw3 render_format）。
  - [ ] 可执行探测 + 版本校验；命令注入防护（路径用参数数组，不拼字符串）。
  - [ ] 超时 + 失败 → `preparation_state="failed"` + `preparation_error`。
- [ ] **路线 B（纯 Rust，后续）：** 自解 PalmDB + HUFF/CDIC + KF8 skeleton/fragment 重组 → 生成 XHTML+OPF+container.xml 打 zip。工作量大，KF8 尤复杂；可参考 foliate `mobi.js` 的解析逻辑移植。
- [ ] 用测试文件验收：`西学三书.azw3`(KF8 v8, HUFF/CDIC)、`重读20世纪中国小说.mobi`(MOBI6 v6) 转 EPUB 后可选中/查词/标注。

### Phase 3 — 扫描版 PDF → OCR → EPUB ⬜ 未开始
- [ ] 导入时用 pdfium 抽样若干页判定文本层：可提取文本≈0 → 标记为 `needs_ocr`，走管线；否则维持现有 pdf 直读。
  - [ ] 复用/修复 pdfium 绑定（**注意：** 生产日志出现 `extract_pdf: pdfium unavailable: LoadLibraryError`，`libpdfium.dylib` 未随包分发——这是独立 bug，OCR 判定依赖 pdfium，需先修，见"关联 bug"）。
- [ ] OCR 阶段（后台）：逐页 PDF→图 → 送多模态模型（`src-tauri/src/ai/`，OpenAI 兼容 provider）识别为结构化 Markdown/HTML。
  - [ ] 进度反馈（复用 `ai.overviewPreparing` 的 `{done}/{total}` 样式）、可取消、失败可重试。
  - [ ] 无 AI provider 时优雅降级；大部头耗时/耗 token 提前告知用户。
- [ ] 组装 EPUB（XHTML+OPF），`render_format=epub`。
- [ ] 保留原始 PDF，提供"原版对照"切换入口（OCR 有错字风险）。

## 4. 关联 bug（本次排查顺带发现，不属本管线但需跟踪）
- [ ] **pdfium 未随生产包分发**：`libpdfium.dylib` LoadLibraryError → PDF 封面/元数据抽取静默失败（当前无封面）。Phase 3 强依赖，需先修。
- [ ] **iCloud 同步损坏**：`SYNC_BLOB_PATH_INVALID` + `rejecting invalid event log for peer`，活动库缺书。独立工单。

## 5. 验收标准（Definition of Done）
1. AZW3/MOBI 导入后，等待准备完成，能在阅读器中**选中文本并触发 AI 查词**。
2. 扫描版 PDF 导入后可选择 OCR 转 EPUB，产物可选中/查词，且能切回原版 PDF。
3. 无 Calibre / 无 AI provider / 无 pdfium 时，全部**优雅降级**，不 crash、不无限转圈。
4. 转换产物不污染 iCloud 同步。
5. 相关后端命令有单元测试；`cargo test` / `npx tsc` / `npm run lint` 全绿。

## 6. 变更日志（Changelog）
- **2026-07-16** — 创建文档。完成 Phase 0 Layer A（`useFoliateView.ts` init 超时清位置重试）。Layer B 及 Phase 1–3 未开始。记录关联 bug：pdfium 缺失、sync 损坏。
