# 扫描 PDF OCR 与可同步派生资产 — 实施方案

> 状态：执行中；Phase A、Phase B 已完成，Phase C PoC 进行中
>
> 日期：2026-07-18
>
> 评审对象：[扫描 PDF OCR 升级方案（评审稿）](../reviews/scanned-pdf-ocr-upgrade-proposal-2026-07-17.md)
>
> 评审方式：逐条对照 Lantern 源码核实关键论断，外部依赖对照 OCRmyPDF 官方文档核查
>
> 本文是可执行的实施计划。产品决定、风险全表、术语、验收细目沿用评审稿，不在此重复；本文只记录裁决、修正、裁剪和落地步骤。

## 0. 评审结论

**总体判定：方案成立，采纳为主路线。**「用本地 OCR 生成标准 searchable PDF，作为可同步的派生阅读资产」方向正确：它复用现有 PDF.js 文字层与全部 AI 链路，衔接点找得准，风险清单完整。相对评审稿，本方案做了 6 处修正/裁剪（§0.3），并把两个发布门槛从 OCR 项目中拆出提前交付（§2、§3）。

### 0.1 代码论断核实结果

评审稿引用的关键代码论断已逐条对照源码核实，**全部属实**：

| 论断（评审稿章节） | 核实位置 | 结论 |
|---|---|---|
| `source_file_path` 路径契约不一致（§2.5） | `src-tauri/src/commands/books/import.rs:135,281,430`；`src-tauri/src/sync/validation.rs:293-295,475-477` | 属实，且比原文严重（§0.2） |
| 无效事件拒绝整份 peer 日志（§2.5） | `src-tauri/src/sync/replay.rs:441-449` | 属实 |
| snapshot 不验证 `source_file_path`（§2.5） | `src-tauri/src/sync/snapshot/apply.rs:151` | 属实（只验 `file_path`） |
| 信封完整但版本/类型不支持 → 拒整份日志；撕裂 JSON → 跳过（§15.5） | `src-tauri/src/sync/log.rs:290` 起 | 属实，注释明确以 watermark 安全为设计理由 |
| 事件 schema v6、强类型 tagged enum（§15.2） | `src-tauri/src/sync/events.rs:25,93` | 属实 |
| `prepared/` 是本地非同步派生、各设备自行重转（§2.3） | `src-tauri/src/commands/books/convert_prepare.rs:6-8` | 属实 |
| `preparation_state` 未 ready 时阻止开书（§11.2） | `src/hooks/useBooks.ts:175` | 属实（因此 OCR 不能复用该字段） |
| 当前页文字层为空检测（§2.2） | `src/pages/reader/useReaderInteractions.ts:97-105` | 属实 |
| `open-settings` 事件只传 section 字符串（§9.1） | `src-tauri/src/commands/settings.rs:582`、`src/pages/Home.tsx:77` | 属实 |
| grounding 扫描判定 `page_count > 5 && total_chars < 500`（§12.2） | `src-tauri/src/ai/grounding/extract.rs:182-183`、`index.rs:221` | 属实 |
| 捆绑 libpdfium 最低系统高于 README 宣称的 macOS 11（§17.4） | `otool -l src-tauri/binaries/macos-aarch64/libpdfium.dylib` → `minos 12.0, sdk 26.0` | 属实 |
| `move_dir_contents` 只处理一层文件（§14.3） | `src-tauri/src/commands/sync.rs:806` 及其测试 | 属实（派生资产必须平铺在 `books/`） |
| OCRmyPDF v17 已将 Ghostscript 降为可选、支持 pypdfium rasterizer（§8.2） | ocrmypdf.readthedocs.io v17 release notes | 属实。注意 v17 默认 `--output-type auto`（尽力 PDF/A），必须显式传 `--output-type pdf` |

### 0.2 P0：`source_file_path` 是确定性、正在发生的同步缺陷

评审稿说「这**很可能**让合法的原生书籍导入事件报 `SYNC_BLOB_PATH_INVALID`」。核实结果：不是可能，是确定发生，链路如下——

1. 原生 EPUB/PDF 导入把 `source_file_path` 设为与 `file_path` 相同的 `books/...`（`import.rs:135`、`281`、`352`）。
2. `BookImport` 事件原样携带该字段写入本机日志（`import.rs:430`）。
3. 本地写入端不做事件验证（`writer.rs` 无 validate 调用），导入照常成功，用户无感。
4. 对端 replay 对日志中每条事件执行 `validate_event`；`source_file_path` 走 `validate_source_path`，只接受 `sources/` 前缀（`validation.rs:293-295,475-477`）→ 事件无效 → **整份 peer 日志被拒**（`replay.rs:448`）。
5. 后果：任何设备导入一本原生书之后，该设备日志中**此后全部事件**（阅读进度、高亮、词汇、笔记——所有）对其他设备不可见，只能靠周期性 snapshot 粗粒度兜底。事件级同步实质失效。

实施修复时（2026-07-18）发现缺陷还有评审稿未覆盖的另一半：`do_import_text`（txt/markdown/html 导入）把 **`file_path` 本身**也写在 `sources/`（`import.rs:200`），而事件与 snapshot 验证对 `file_path` 只接受 `books/`。因此文本书不但事件被拒，**含文本书的 snapshot 也整份不可应用**（`snapshot/apply.rs:151`，replay 每 tick 跳过重试）——对文本书而言事件与快照两条兜底通道同时失效。

因此该修复不应放在 OCR Phase 3 的第一项（评审稿位置），而是**立即独立修复、随最近一个 patch 发布**（本文 Phase A，§2）。

### 0.3 相对评审稿的修正与裁剪

1. **阶段重排**：`source_file_path` 修复（Phase A）与同步能力通告字段（Phase B）拆为独立交付，不与 OCR 绑定。旧客户端兼容窗口的长短取决于所有设备升级到「会通告能力」版本的时间——越早上车越短。
2. **旧客户端策略选型**（评审稿 §15.5 列了三个候选）：**不改解析器**。现有「信封完整但不支持 → 拒整份日志」是正确的数据安全设计（防 watermark 越过丢行），保持不动。选择 **peer manifest 能力通告 + 发送端 gating**：设备只在全部活跃 peer 通告支持 ≥ v7 后才开始发送资产事件（§7.3）。
3. **macOS 基线先行决策**：捆绑 libpdfium 实测 `minos 12.0`，macOS 11 支持在 OCR 之前就已失效（该库在 Big Sur 上无法加载）。README/产品基线已修正为 macOS 12+；OCR 扩展和 Vision 后端按该基线打包。若未来要恢复 macOS 11，必须先重建并在真实 Big Sur 设备验证 PDFium。
4. **PoC 增加 OS 原生 OCR 对比**：评审稿未评估 Apple Vision（`VNRecognizeTextRequest`）与 Windows.Media.Ocr。若 Vision 中英质量达标，macOS 端可做到零 Python runtime 和零 OCR 引擎下载；代价是自建文字层写回（Vision 坐标 → PDFium grafting），且两平台引擎不一致。因为派生资产会同步，引擎跨平台不一致可接受。Phase C 已证明 Vision + 现有 PDFium 的单页闭环可行，但真实扫描、签名 app 和复杂 PDF 结构仍未验收。SDK 证据同时修正了最低版本判断：Revision 2 在 macOS 11+ 的 `accurate` 模式支持中文；Revision 3 是 macOS 13+ 的质量/旋转改进，不是中文支持的硬前提。
5. **v1 数据模型裁剪**：不建 `book_asset_positions` 表。v1 唯一新增资产是页数与源一致的 searchable PDF，现有 `books.progress/current_cfi` 在源 ⇄ OCR 资产之间可直接复用；扫描页本无文字层，也就不存在会漂移的既有文本锚点。该表推迟到 EPUB 派生资产进入范围时再建（评审稿 §14.5 保留为未来设计）。
6. **扩展签名依赖既有 Gatekeeper 工作**：主程序当前仍是 ad-hoc 签名（见 [macOS 分发计划](macos-distribution-gatekeeper-fix.md)），评审稿 §9.4 安装第 5 步「验证签名/公证」在主程序签名方案落地前无从谈起。OCR runtime 的签名与主程序签名同批解决；Phase E 排期以此为前置。

### 0.4 对评审稿 §26「编码前必答」问题的裁决

| # | 问题 | 裁决 |
|---|---|---|
| 1 | 无 Ghostscript 能否跑通 | 已在 Phase C 的空环境、无 `gs`/veraPDF/unpaper/pngquant/jbig2enc 条件下跑通单页 OCRmyPDF；完整合法语料仍待补 |
| 2 | 真实最低系统/体积 | README 已修正为 macOS 12+；Vision Rev2 可作为 macOS 12 基线，signed app、Windows 和最终包体仍待验证 |
| 3 | OCR 输出能否被 PDF.js 稳定选择 | Vision+PDFium 单页已通过 PDF.js 文本提取和 DOM 跨行 Range；真实鼠标选择、复杂版面和 Reader 全链路仍待验证 |
| 4 | 是否默认 `--rotate-pages` + OSD | v1 不做，不装 `osd.traineddata`（省 10 MiB 与处理时间）；Phase H 依样本收益再议 |
| 5 | 结构化进度走哪条路 | 扩展内置薄 wrapper，用 OCRmyPDF plugin API 输出 JSON Lines（评审稿 §11.4 格式）；拿不到就退化为不确定进度条 |
| 6 | 未知事件前向兼容 | peer manifest 能力通告 + 发送端 gating（§7.3），解析器语义不变 |
| 7 | `books/` 还是 `derived/` | `books/` 平铺（`move_dir_contents` 单层已核实，评审稿 §14.3 理由成立） |
| 8 | 活动资产选择模型 | `preferred_asset_id` 同步（LWW）+ 本机 resolver 兜底；本机覆盖不进事件（评审稿 §14.4 采纳） |
| 9 | 是否允许重新 OCR | 允许，仅显式操作；生成新 asset，旧 asset 以 `supersedes_asset_id` 链接并保留，不自动删除、不自动重跑 |
| 10 | 手机端消费格式 | 超出本方案范围；`book_assets` 模型对 searchable PDF 与未来 EPUB 均可消费，不预设 |

## 1. 范围

- 产品决定沿用评审稿 §4 全表（已确认项不再重议）。
- 目标/非目标沿用评审稿 §5，外加本文裁剪：v1 无 `book_asset_positions`、无 OSD、无 EPUB 派生。
- 发布门槛沿用评审稿 §1.3，其中「路径契约修复」「旧客户端兼容」由 Phase A/B 提前满足。

## 2. Phase A — 修复书籍路径契约（已实施，commit `75139ff`）

**状态：已实施并合入 `main`（2026-07-18），待随下一个 patch 版本发布。**

统一「`file_path` 与 `source_file_path` 按角色命名、不绑定目录」的契约，两字段均接受 `books/` 或 `sources/` 根：

1. `validation.rs` 新增 `validate_book_file_path()`：按前缀分派到既有的 `books/`/`sources/` 验证器（保留穿越/扩展名检查）。
2. `BookImport` 的 `file_path` 与 `source_file_path`、`BookMetadataSet` 的 `file_path` 字段、snapshot 书籍行的两个字段全部改用它（snapshot 侧此前对 `source_file_path` 完全没有验证）。
3. `resolve_blob_path` 本就接受两种前缀，未改动。
4. 历史数据自愈：被拒日志未推进 watermark，对端升级后下个 tick 重新解析整份日志即可全量补齐；被跳过的 snapshot 每 tick 重试。无需迁移或重写日志。

已落地的回归测试（全部通过，Rust 套件 471 项全绿 + clippy 干净）：

- `validation.rs`：`validate_book_file_path` 双根接受/逃逸拒绝；原生 EPUB 形态与文本形态的 `BookImport` 事件通过 `validate_event`，逃逸路径拒绝。
- `replay.rs`：原生 + 文本导入形态的 peer 日志端到端应用（此前整份被拒的场景），后续进度事件不丢。
- `snapshot/tests.rs`：含两种形态书籍的 snapshot `apply_peer` 成功；`source_file_path` 逃逸的 snapshot 拒绝。

## 3. Phase B — 同步能力通告（已实施，commit `1faf519`）

**状态：已实施并合入 `main`（2026-07-18），待随任意近期版本发布。**

- 本设备 manifest 写入当前支持的最大事件 schema 版本（现为 6，Phase G 后为 7）。
- 读取侧：缺失该字段的 manifest 视为 v6。
- 此版本不含任何行为变化，纯通告——目的是让「已升级设备集合」尽早可观测，缩短 Phase G 的 gating 等待期。
- Rust `PeerInfo` 与前端同步状态 DTO 同时透出该字段，供 Phase G 列出阻塞升级的设备。
- 已验证：peer 测试 21 项、replay heartbeat manifest 测试、Clippy、前端类型检查及相关文件格式检查通过。

## 4. Phase C — PoC（go/no-go 门）

样本矩阵沿用评审稿 §22 Phase 0（10–20 本：中英、混合、倾斜、双栏、脚注、低清、加密/损坏）。

### 4.1 OCRmyPDF 最小包验证

- 干净环境（清 PATH、确认无 `gs`/veraPDF/unpaper/pngquant/jbig2enc）跑通固定参数：`--mode skip --output-type pdf --rasterizer pypdfium --optimize 0 --fast-web-view 999999 -l chi_sim+eng`。
- macOS arm64（按 §0.3.3 修正后的基线）与 Windows 11 x64 自包含打包，实测下载/安装体积、真实最低 OS。
- 输出 SBOM、原生库清单、许可证清单。
- fast/best 准确率、耗时、峰值内存对比；`chi_sim+eng` vs `eng+chi_sim` vs 纯英文。
- Phase C 已完成单页空环境 smoke：OCRmyPDF 17.8.1 + Tesseract 5.5.2 + pypdfium2 在没有 Ghostscript 等可选工具时成功输出 searchable PDF。当前样本只能作为数字/混合 PDF 的控制性 raster smoke，不足以替代合法扫描语料。
- 当前实测中，`tessdata_best/chi_sim` 会加载 `chi_sim_vert`；高精度模型包必须把该依赖和其许可证/体积纳入 manifest。

### 4.2 OS 原生 OCR spike（timebox 2–3 天）

- Apple Vision `VNRecognizeTextRequest`（accurate, zh-Hans+en）跑同一样本，对比字符错误率与词级坐标质量；已证明 Rev2/Rev3 都能识别当前中英控制页，Rev2 的中文 accurate 支持 macOS 11+，Rev3 作为 macOS 13+ 改进版本。
- Windows.Media.Ocr 同样本快测（预期中文质量不足，需数据证实）。
- 评估文字层写回（word box → 不可见文字 + ToUnicode）实现成本：首选 Rust `pdfium-render` + 当前 Lantern PDFium；`pdf-writer`/`pikepdf` 作为回退候选。
- Vision + 现有 PDFium 已完成单页写回 PoC：30 行文字、PDF.js 可提取、渲染像素差异为 0；复杂版面、签名 app 和全 Reader 交互仍待验收。
- **决策规则**：若 Vision 质量达标且写回成本 < 扩展管理器（Phase E）成本，macOS 主路线切换为 OS 原生、Windows 保留 OCRmyPDF 扩展；否则双平台统一 OCRmyPDF。`OcrBackend` trait（§6.2）两种结局都兼容。

### 4.3 输出可用性验证

- OCR 输出在当前 Foliate/PDF.js 中：单击/双击/拖选、中文复制、跨行与双栏选择、AI 查词/翻译、页码/缩放/双页、高亮创建与恢复。
- 页数一致性、视觉页无变化（默认不 deskew/clean/force-ocr）。

**退出条件**：给出 go/no-go 报告、最终后端选型、固定参数与依赖清单。不通过则按评审稿 §8.2 次优先级评估 PDFium + native Tesseract + grafting，不转向实时覆盖层。

Phase C 当前仍为 **IN PROGRESS**。完整证据和未通过的门槛见 [PoC 报告](../reviews/scanned-pdf-ocr-phase-c-poc-2026-07-18.md)；在合法样本、signed `.app`、Windows 和 Reader 交互验证前，不得把 Vision 路线标记为最终 go。

## 5. 数据模型（migration `028_book_assets.sql`）

```sql
CREATE TABLE book_assets (
  id                  TEXT PRIMARY KEY,          -- 稳定 UUID
  book_id             TEXT NOT NULL REFERENCES books(id),
  role                TEXT NOT NULL,             -- source | ocr_pdf （EPUB 后续再加）
  format              TEXT NOT NULL,             -- pdf | epub
  relative_path       TEXT NOT NULL,             -- source: books/ or sources/; derived: books/
  content_sha256      TEXT,                     -- source bridge may be unknown until materialized
  byte_size           INTEGER,                  -- source bridge may be unknown until materialized
  source_asset_id     TEXT,
  source_sha256       TEXT,
  pipeline            TEXT,                      -- ocrmypdf | apple_vision | ...
  pipeline_version    TEXT,
  language_profile    TEXT,                      -- chi_sim+eng
  quality_profile     TEXT,                      -- fast | best
  conversion_version  INTEGER NOT NULL DEFAULT 1,
  page_count          INTEGER,
  supersedes_asset_id TEXT,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  updated_by_device   TEXT NOT NULL
);

CREATE UNIQUE INDEX book_assets_relative_path_idx ON book_assets(relative_path);
CREATE UNIQUE INDEX book_assets_one_source_idx
  ON book_assets(book_id) WHERE role = 'source';
CREATE INDEX book_assets_book_updated_idx
  ON book_assets(book_id, updated_at DESC);

CREATE TABLE book_asset_local_state ( -- 本机状态，永不进入事件日志或快照
  asset_id TEXT PRIMARY KEY REFERENCES book_assets(id),
  book_id TEXT NOT NULL,
  availability TEXT NOT NULL,        -- remote_only|downloading|available_verified|corrupt|missing
  observed_size INTEGER,
  observed_mtime INTEGER,
  verified_at INTEGER,
  error_code TEXT,
  updated_at INTEGER NOT NULL
);
CREATE INDEX book_asset_local_state_book_idx ON book_asset_local_state(book_id);

ALTER TABLE books ADD COLUMN preferred_asset_id TEXT;  -- 可空，同步偏好

CREATE TABLE ocr_jobs (        -- 本机执行状态，永不进入事件日志
  id TEXT PRIMARY KEY, book_id TEXT NOT NULL,
  source_asset_id TEXT, source_sha256 TEXT NOT NULL,
  state TEXT NOT NULL,         -- queued|waiting_source|preparing|recognizing|validating|publishing|ready|failed|cancelled
  phase TEXT, pages_done INTEGER, pages_total INTEGER,
  backend TEXT, backend_version TEXT,
  language_profile TEXT, quality_profile TEXT, jobs INTEGER,
  conversion_version INTEGER NOT NULL DEFAULT 1,
  result_asset_id TEXT, recognized_pages INTEGER, skipped_pages INTEGER,
  timed_out_pages INTEGER, failed_pages INTEGER,
  temporary_path TEXT, error_code TEXT, error_detail TEXT,
  created_at INTEGER, started_at INTEGER, updated_at INTEGER
);
CREATE INDEX ocr_jobs_book_updated_idx ON ocr_jobs(book_id, updated_at DESC);
CREATE UNIQUE INDEX ocr_jobs_one_active_idx ON ocr_jobs(book_id)
  WHERE state IN ('queued','waiting_source','preparing','recognizing','validating','publishing');

-- Derived assets must be immutable and complete; source rows are a legacy bridge.
CREATE TRIGGER book_assets_validate_insert BEFORE INSERT ON book_assets
BEGIN
  SELECT CASE WHEN NEW.role <> 'source'
    AND (NEW.content_sha256 IS NULL OR NEW.byte_size IS NULL OR NEW.source_sha256 IS NULL)
    THEN RAISE(ABORT, 'derived asset metadata incomplete') END;
END;
```

要点（均采纳评审稿 §14，裁剪见 §0.3.5）：

- 现有 `source_format/render_format/source_file_path/source_sha256` 保留为兼容桥梁，本版本不删。
- `role=source` 迁移：资产 ID 由 `book_id` 确定性派生（UUIDv5），防两台离线设备各自迁移出不同 source asset；当前 legacy 行的 hash/size 可为空，物化后再补齐。UUIDv5 回填由 Rust 幂等 post-migration/lazy backfill 完成，不能假设 SQL migration 能计算 UUID。
- 派生文件路径必须包含唯一 `asset_id`，例如 `books/{book_id}.ocr-pdf.v1.{source_hash_prefix}.{profile}.{asset_id}.pdf`；同配置重做或两台设备并发生成不能覆盖旧资产。
- `book_asset_local_state` 将远端资产元数据与本机可用性分开；应用重启后不能仅凭文件存在或 preferred 指针激活资产。
- resolver 优先级：本机明确且有效的选择 → 同步 `preferred_asset_id` → 最新受支持 searchable PDF → source。本机可用性区分 `remote_only/downloading/available_verified/corrupt/missing`，不复用 `Book.available`。
- 删除整书的显式 cascade 与「防漏表」测试扩展到 `book_assets`、`book_asset_local_state`、`ocr_jobs`；测试必须先 seed 每张表再断言删除，避免假绿。

## 6. 后端实现（Phase D 本地管线，feature flag 内）

### 6.1 模块布局

```
src-tauri/src/commands/ocr/
  mod.rs        # Tauri 命令: 状态查询、开始/取消/重试、包管理入口
  package.rs    # OcrPackageManager: manifest 拉取、下载、校验、原子安装/卸载
  backend.rs    # OcrBackend trait + ocrmypdf(/vision) 实现、进度 JSONL 解析
  jobs.rs       # OcrJobManager: 全局单书队列、guarded update、崩溃恢复
 publish.rs    # BookAssetPublisher: 输出验证、哈希、原子发布 + 同一事务写 outbox
```

复用 `convert_prepare.rs` 的既有模式：源快照 + guarded update、临时文件原子 rename、启动恢复。不复用其 `Converter` trait（EPUB-only destination）与 `preparation_state`（会阻止开书，OCR 必须允许继续读原件）。

Phase D 的安全边界：migration、asset repository、resolver、fake backend 和本地 job 状态可以先落地；`package.rs` 属于 Phase E，不应成为数据模型切片的硬依赖；真实 publish 在 Phase G 前不得写同步 outbox。当前仓库没有现成 Cargo feature 约定，若引入 `ocr-pipeline` feature，默认 release 必须 fail closed（不启动 worker、不恢复任务、不写文件），并同时测试默认与 feature 构建。

### 6.2 `OcrBackend` trait

采纳评审稿 §8.3 原型（`probe/recognize_pdf/进度 sink/取消 token`）。后端禁止：覆盖源文件、碰数据库、发事件、拼 shell 字符串、把用户文件名当选项（必要时 `--` 分隔）。

### 6.3 执行与调度

采纳评审稿 §10：单书全局队列、`jobs = clamp(min(cpu_jobs, memory_jobs), 1, 4)`（常数 Phase H 定稿）、`OMP_THREAD_LIMIT=1`、独立 process group / Job Object、stdout/stderr 持续 drain（Calibre 执行器已有同款防护）、取消先优雅后杀进程树、不承诺断点续转。子进程不占 webview 线程。

### 6.4 验证与发布

采纳评审稿 §15.3 十步顺序，关键不变量：

- 长 OCR 阶段**不持有** sync transition lock；完成后才取 guard 并重读当前 `data_dir`。
- 验证 = 页数一致 + 首/中/末页 PDFium 渲染抽样 + 预期扫描页有文字层；页数不一致视为失败，不切换。
- Phase D 本地管线不得向现有 v6 outbox 写资产 body，也不得把 v7 body 包装成 v6 envelope。真正的资产 publish/preferred 写入、`SyncWriter.with_tx()` outbox 事务和 `data_dir/books/` 同步发布必须与 Phase G 的 schema v7、snapshot 和 peer gating 同批落地；Phase D 只允许写本地 staging、任务和验证结果。
- 禁止「先事件后文件」。
- 结果报告记录 `recognized/skipped/timed_out/failed` 页数；有失败页时完成文案如实说明。
- 数字签名 PDF：v1 直接拒绝自动 OCR 并说明原因。

### 6.5 AI grounding 衔接

资产 ready 后触发对活动资产重建索引；`PDF_TEXT_LAYER_UNAVAILABLE`（`extract.rs:220`）状态必须可自愈。grounding 的整书候选信号（`page_count > 5 && total_chars < 500`）可复用为「疑似扫描」缓存，但产品判断以当前页检测为准（评审稿 §12.2 的漏判/误判分析成立）。

grounding 必须调用与 Reader 相同的 active-asset resolver，并把 resolved asset 的 content hash 纳入索引 key；否则 OCR 完成后仍会固定读取 legacy `books.source_file_path`，重新索引扫描原件并再次得到 `PDF_TEXT_LAYER_UNAVAILABLE`。

## 7. 同步协议（Phase G，事件 schema v7）

### 7.1 新事件

```
book.asset.publish        # 完整不可变资产元数据；可带 make_preferred=true
book.asset.delete         # tombstone；同一事务先写 outbox 再 best-effort 删共享 blob
book.preferred_asset.set  # 用户显式切换
```

v1 不加 `book.asset_position.set`（§0.3.5）。validation 为新事件扩展路径白名单（`books/{book_id}.ocr-pdf.*` 模式）；merge/snapshot/tombstone 同步扩展；snapshot schema 随 v7 升级。

### 7.2 接收端

采纳评审稿 §15.3/15.4：metadata 与文件到达是两个状态；`.icloud` placeholder 触发物化；物化后验 size+SHA-256 才可成为活动资产；失败回退 source 并可重试。事件先到/文件先到/哈希错误三种错序均有测试（评审稿 §23.4）。

### 7.3 旧客户端 gating（发布阻断项的落地方式）

- 解析器语义不变：v6 客户端收到 v7 事件仍会拒该 peer 整份日志——这正是要避免触发的场景。
- 发送端规则：仅当**全部活跃 peer** 的 manifest 通告 `max_event_schema >= 7` 时才发送资产事件；否则 OCR 功能本机可用，但资产不发布到日志（UI 在设置中列出阻塞的设备名）。
- 活跃判定：manifest `last_seen` 超过 30 天的 peer 不计入（阈值可调）；提供手动忽略入口。
- 一旦开始发送 v7 事件即单向棘轮，不回退。
- 能力判定必须 fail-closed，不能直接对 `list_peers()` 的返回值调用 `all()`：当前列表会跳过损坏、不可读和 `.icloud` placeholder manifest，暂时不可见的旧设备不能因此被当作不存在。Phase G 需要保守的能力发现 API，处理 placeholder 与已知历史设备；未知能力默认阻塞，除非已经过期或被用户显式忽略。
- 「手动忽略」是本机 gating 决策，不等同于当前会删除对端 manifest/log/snapshot 的破坏性「移除设备」。单向棘轮状态和忽略记录都必须持久化。

### 7.4 删除与去重

采纳评审稿 §15.6/15.7：逻辑去重键 `book_id + source_sha256 + conversion_version + languages + quality`；收到同键 ready 资产后不自动重复生成；「仅移除本机副本」vs「全设备删除」两个独立操作，后者 tombstone 先行。

## 8. 前端实现（Phase E 设置 + Phase F 阅读器）

### 8.1 设置（`src/components/settings/`）

- 「阅读辅助」内新增「扫描件 OCR」子视图，遵循 `GeneralSettings.tsx` 行模式与 `ROW_CONTROL_WIDTH`。
- 区块：OCR 组件（状态机 8 态 + 下载进度 + 错误可复制）、识别质量（fast 默认 / best 独立下载卸载，选择未安装的 best 先确认下载）、存储与派生资产（总占用 + 管理入口 + 两种删除）。不显示 jobs/PSM/DPI。
- `open-settings` payload 向后兼容扩展：`SettingsSection | { section, view: "ocr" }`；旧字符串继续有效。Reader CTA 走后端命令统一 `show()` + 聚焦主窗口 + 发结构化 destination。

### 8.2 阅读器（`src/pages/reader/`）

- 现有底部提示条从 `pointer-events-none` 状态条升级为可交互非模态操作条，与 binding HUD 统一容器/优先级；OCR controller 在「需要选择内容」提示之前裁决，保证一次交互只出一条提示。
- 状态 → 文案/操作矩阵沿用评审稿 §13 表格。
- 触发规则沿用评审稿 §12.3（点击/拖选空 Selection/右键/文字型快捷键触发；翻页缩放滚动不触发）；检测结果按 `book + page + source_hash` 缓存并节流。
- 完成切换：记录页码/缩放/模式/布局 → 本地文件验证通过才重载 → 页数一致才恢复同页 → 新文字层实际渲染后才出 Toast（`role=status`，2.5–5 s，不抢焦点）→ 失败保持原 PDF + 重试。
- 事件：`ocr-package-changed` / `ocr-job-changed`（约 4 Hz 节流）/ `book-assets-changed`；窗口 mount 时主动查询，事件只做增量。

### 8.3 i18n

全部新文案同步 `src/i18n/en.json` 与 `zh.json`，键前缀 `ocr.*`（`ocr.package.*`、`ocr.job.*`、`ocr.reader.*`、`ocr.storage.*`）。Rust 层只出稳定错误码，前端映射文案。

## 9. Figma 设计提示词（高层意图，细节交给设计工具）

**Prompt 1 — 设置 · 扫描件 OCR 子视图**：桌面阅读应用设置弹窗中的一个子页面，延续现有 73px 行高、行间 1px 浅分隔线的列表风格。三个分组：①「OCR 组件」——一行显示组件名、版本与占用空间，右侧是随状态变化的主操作（下载/更新/卸载/重试），下载中在行内展示字节进度条；②「识别质量」——快速/高精度两选项，高精度未安装时显示预计下载体积并需确认；③「存储」——派生资产总占用与「管理已识别书籍」入口。需要覆盖状态：未安装、下载中、校验中、已安装、可更新、失败（错误摘要 + 可复制详情）。整体安静克制，不用醒目色块。

**Prompt 2 — 阅读器 · 扫描页操作条**：PDF 阅读器底部居中的非模态胶囊操作条，出现在用户尝试选择扫描页文字时。一句状态文案 + 一个主按钮（如「前往下载」「开始识别」「取消」），随状态切换：未安装 / 下载中 / 可识别 / 排队中 / 识别中（含可信页数进度）/ 失败（含重试）。不遮挡正文阅读，可手动关闭，与现有快捷键 HUD 共用同一底部区域时只显示优先级更高的一条。

**Prompt 3 — 完成 Toast 与远端下载态**：①识别完成后阅读器顶部的轻量 Toast：「文字识别完成，现在可以选词和使用 AI 功能」，数秒自动消失，不抢焦点；②另一设备打开该书时顶部的低调下载指示：「正在从 iCloud 下载已识别版本」，完成后无缝切换。两者均为信息性提示，弱于任何模态元素。

## 10. 测试计划

沿用评审稿 §23 全表（单元/后端集成/Reader UI/同步/性能矩阵），增补：

- Phase A 路径契约回归（§2 所列三组）。
- §7.3 gating：混合 v6/v7 peer 场景下不发送 v7 事件；全员 ≥7 后开始发送；stale peer 忽略逻辑。
- 性能矩阵的 macOS 最低版本项按 §0.3.3 修正后的基线执行（不再测 macOS 11）。
- 若 Phase C 选中 Vision 后端：双后端输出在同一验证器（页数/渲染抽样/文字层）下等价通过。

Definition of Done 沿用评审稿 §23.6，去掉 macOS 11 一项、按新基线替换。

## 11. 阶段依赖与发布顺序

```
Phase A（sync 修复）──→ 随下一 patch 发布（不等 OCR）
Phase B（能力通告）──→ 随任意近期版本发布
Phase C（PoC）───────→ go/no-go 决策 + 后端选型
Phase D（数据模型 + 本地管线）→ feature flag 内合入
Phase E（扩展管理 + 设置）──→ 依赖 Gatekeeper 签名方案落地
Phase F（阅读器交互）
Phase G（同步资产，schema v7）→ 发布门槛：§7.3 gating 测试全绿
Phase H（硬化：并发常数定稿、混合 PDF、1000 页压测、低内存设备）
```

工程量按评审稿 §24「OCRmyPDF + 完整同步资产」路线估算（MVP 6–10 周、硬化 8–14 周）；若 Phase C 选中 Vision 路线，Phase E 在 macOS 侧大幅缩减，总量预计下调 2–4 周。

## 12. 与现有计划的关系

- [格式规整管线](format-normalization-pipeline.md) Phase 3（AI provider OCR → EPUB → 本地 prepared/）按评审稿 §25 作废，由本文替代：本地开源引擎、首产物 searchable PDF、派生资产同步。方案获批后更新该文档，避免两份冲突设计并存。
- 现有 MOBI/AZW → EPUB 本地产物是否迁移 `book_assets` 并同步：**不在本方案范围**，单独立项决定。
- 评审稿归档：实施启动时将评审稿移入 `docs/reviews/`（保持原位）并在 feature 流程中建 issue。
