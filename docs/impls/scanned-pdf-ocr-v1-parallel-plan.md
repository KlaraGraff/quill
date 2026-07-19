# 扫描 PDF OCR v1 — 精简范围与四路并行实施方案

> 状态：**当前生效的实施方案**，2026-07-18 与维护者对齐后定稿
>
> 本文取代 [scanned-pdf-ocr-pipeline.md](scanned-pdf-ocr-pipeline.md) 的 Phase C–H 细节，以及
> [评审交接稿](../reviews/scanned-pdf-ocr-review-handoff-2026-07-18.md) 中与 §1 冲突的「已确认」项。
> 冲突之处一律以本文为准。
>
> 读者：执行本方案的 AI 代理。每个工作流（W1–W4）可以由一个独立代理承担；本文即是它的任务书，
> 不依赖任何会话上下文。

## 1. 维护者裁决（2026-07-18，覆盖旧文档）

| # | 决定 | 说明 |
|---|---|---|
| 1 | **内测规则：不做任何旧版本兼容** | Lantern 处于内测阶段。凡是为旧客户端、旧数据、旧协议服务的工作（backfill、gating、双协议共存、legacy 桥接）一律不做；旧数据用重新导入解决。此规则直到维护者宣布大规模分发才失效 |
| 2 | 平台：macOS + Windows 双端发布 | 但 **Windows 只保证扩展包能构建成功**（维护者无 Windows 验证环境）；iCloud 同步保持仅 macOS，不为 Windows 做同步兼容 |
| 3 | 模型：**仅 fast**（`chi_sim+eng`） | 不做 best 高精度档位，不做模型独立下载/卸载/切换/重识别 UI。数据库保留 `quality_profile` 字段以便未来加回 |
| 4 | 数据：**不做 source 资产桥接/回填** | `book_assets` 只存派生资产；派生行用 `book_id + source_sha256` 关联源文件（`books` 表已有）。删除工作区原型中全部桥接钩子 |
| 5 | 同步：**直接 bump schema v7，无 gating** | 不做能力协商、ratchet、stale-peer 规则、阻塞设备 UI、local-only 晋升协议。所有设备升级新版即可；v6 旧设备收不到新日志是内测阶段可接受的 |
| 6 | 不做 preferred_asset_id | v1 每本书最多一份有效 OCR 资产，resolver 规则全设备一致（最新已验证 OCR PDF → 否则原文件），无需同步偏好指针 |
| 7 | 保留的旧决定 | searchable PDF 为唯一产物；原 PDF 永不覆盖；OCRmyPDF+Tesseract 唯一后端（Vision 后置）；不做 OSD/EPUB/`book_asset_positions`/Tesseract.js 覆盖层；并发自动、无固定数值上限、`OMP_THREAD_LIMIT=1`；数字签名 PDF 拒绝 OCR |

打包签名：v1 扩展包用 **HTTPS + SHA-256 校验 + 安装后 self-test**，不建签名/密钥体系（主程序本身还是 ad-hoc 签名，扩展不应超过主程序的安全等级；签名随既有 Gatekeeper 项目后补）。

## 2. M0 — 工作区原型处置（W1 的第一个任务，其他工作流的前置）

当前 working tree 有约 3100 行未提交原型（`src-tauri/src/commands/ocr/`、`migrations/028_book_assets.sql`
及散布在同步路径上的桥接钩子）。处置如下，完成后作为一个 commit 落地（feature flag `ocr-pipeline` 保持默认关闭）：

**丢弃**（这些 diff 全部是全格式 source 桥接/回填，按裁决 #4 删除；逐 hunk 检查确认无无关改动后 `git checkout` 恢复）：

- `src-tauri/src/db.rs`（+243 行启动桥接）
- `src-tauri/src/sync/merge.rs`（+40 行 replay 桥接）
- `src-tauri/src/sync/replay.rs`（+12 行）
- `src-tauri/src/sync/snapshot/apply.rs`、`snapshot/tests.rs`（桥接钩子与测试）
- `src-tauri/src/commands/books/import.rs`、`books/tests.rs`（导入桥接）

**重写**：`migrations/028_book_assets.sql` 按 §3 的精简 schema。

**保留并修改**（`src-tauri/src/commands/ocr/`）：

- `assets.rs`：删掉 source-bridge、UUIDv5 backfill、source-identity 锁定、单 source 约束；只留派生资产 CRUD 与完整性约束（发布必须有 hash/size/page_count；不可变；重做创建新行 + `supersedes_asset_id`）。
- `jobs.rs`：保留状态机与唯一活跃任务索引；**修复 P1**——`update_state_guarded()` 目前所有状态变更都要求 source 仍 `available_verified`，导致源文件丢失后任务卡死唯一槽：只有「继续推进」和「publishing→ready」要求源有效，`failed/cancelled/waiting_source` 必须允许无条件 CAS 收尾。删除 `source_asset_id` 字段引用，改用 `source_sha256`。
- `backend.rs`：保留 staging 边界；**修复 P1**——wrapper 必须先把源 PDF 复制进 staging（复制时校验 SHA-256、设只读、禁 hardlink），backend 只接触 staging 输入/输出，真实源路径不外泄。
- `resolver.rs`：简化为「该书最新且本机 `available_verified` 的 `ocr_pdf` → 否则 `books.file_path`」。
- `mod.rs`、`Cargo.toml`：相应精简。

完成标准：默认构建与 `--features ocr-pipeline` 下 `cargo test` 全绿、`cargo clippy -- -D warnings` 干净；同步路径（db/merge/replay/snapshot/import）相对 `main` 零 diff。commit 后通知其他工作流开工。

## 3. 数据模型（migration 028 最终版）

```sql
CREATE TABLE book_assets (          -- v1 只存派生资产，随 v7 同步
  id                  TEXT PRIMARY KEY,
  book_id             TEXT NOT NULL,
  role                TEXT NOT NULL DEFAULT 'ocr_pdf',
  format              TEXT NOT NULL DEFAULT 'pdf',
  relative_path       TEXT NOT NULL,      -- books/ 平铺，见下方文件名
  content_sha256      TEXT NOT NULL,
  byte_size           INTEGER NOT NULL,
  source_sha256       TEXT NOT NULL,      -- 关联生成时的源文件哈希
  pipeline            TEXT NOT NULL,      -- 'ocrmypdf'
  pipeline_version    TEXT,
  language_profile    TEXT NOT NULL,      -- 'chi_sim+eng'
  quality_profile     TEXT NOT NULL DEFAULT 'fast',
  page_count          INTEGER NOT NULL,
  supersedes_asset_id TEXT,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  updated_by_device   TEXT NOT NULL
);
CREATE UNIQUE INDEX book_assets_relative_path_idx ON book_assets(relative_path);
CREATE INDEX book_assets_book_idx ON book_assets(book_id, updated_at DESC);

CREATE TABLE book_asset_local_state (   -- 本机可用性，永不同步
  asset_id     TEXT PRIMARY KEY,
  availability TEXT NOT NULL,  -- remote_only | downloading | available_verified | corrupt
  verified_at  INTEGER,
  error_code   TEXT,
  updated_at   INTEGER NOT NULL
);

-- ocr_jobs：沿用原型结构（本机执行状态，永不同步），去掉 source_asset_id
```

- 派生文件名：`books/{book_id}.ocr.{asset_id}.pdf`（元数据都在资产行里，文件名只需唯一防覆盖）。
- 整书删除的显式 cascade 扩展到 `book_assets`、`book_asset_local_state`、`ocr_jobs`（W4 再加资产 tombstone）；
  测试必须先 seed 再删再断言为空，防假绿。

## 4. 四路并行工作流

> 文件所有权是硬边界：每个工作流只改自己名下的文件。共享文件的一次性注册改动
> （`commands/mod.rs` 注册命令、`db.rs` 注册 migration）归 W1，在 M0 一并完成。
> 各工作流按 §5 契约独立开发测试，不等其他流的实现。

### W1 — Rust 后端：任务管线与本地闭环（关键路径）

**文件**：`src-tauri/src/commands/ocr/**`、`migrations/028`、`commands/books/query.rs`（active path 接入）、
`ai/grounding/`（resolver 接入）、`commands/mod.rs`、`db.rs`（仅注册行）。

任务（M0 之后）：

1. **OcrJobManager**：全局单书队列（内存队列 + `ocr_jobs` 持久化）；启动恢复（`recognizing/validating` → 可重试）；
   自动并发 `jobs = max(1, min(physical_cores-1, memory_slots))`，无固定数值上限；`memory_slots` 使用当前可用内存扣除 3 GiB 系统预留和 768 MiB OCR 主进程预留后，按 512 MiB/worker 计算。取不到资源信息时 `jobs=1`。
2. **OcrmypdfBackend**：调用已安装 runtime（路径由 W2 的 PackageManager 提供，接口见 §5.4）；
   参数固定 `--mode skip --output-type pdf --rasterizer pypdfium --optimize 0 --fast-web-view 999999
   --jobs N -l chi_sim+eng`，外加 `OMP_THREAD_LIMIT=1`、清理后的 PATH、独立 process group（Windows: Job Object）；
   参数数组不过 shell，用户路径用 `--` 隔离；stdout 按 §5.3 JSONL 解析进度，stderr 持续 drain 截断保存；
   取消先优雅后杀进程树。
3. **输出验证**（`validate.rs`）：PDFium 打开；页数与源一致；首/中/末页可渲染；预期扫描页有可提取文字；
   计算 SHA-256/byte_size；记录 recognized/skipped/timed_out/failed 页数。任何失败 → 不建资产、不动 Reader、保留原 PDF、存稳定错误码。
4. **本地发布**（`publish.rs`）：staging → `data_dir/books/` 临时文件 → 原子 rename → 同一 SQLite 事务插入
   `book_assets` + `book_asset_local_state(available_verified)`。长 OCR 阶段不持有任何同步锁。
   **预留 W4 接缝**：入口函数签名 `publish_verified_output(db, NewAssetRow) -> AppResult<AssetId>`，
   M2 时 W4 把事务换成 `SyncWriter.with_tx()` 并追加 outbox 事件（改动限于此函数）。
5. **Tauri commands + 事件**：按 §5.1/5.2 实现 `ocr_start/cancel/retry/job_status/assets_overview/asset_delete`，
   发 `ocr-job-changed`（节流 4 Hz）与 `book-assets-changed`。
6. **resolver 接入**：`books/query.rs` 打开书返回 resolver 解析后的 active path；grounding 索引改用同一
   resolver 并以资产 `content_sha256` 作索引 key 成分，资产 ready 后旧 `PDF_TEXT_LAYER_UNAVAILABLE` 状态自愈重建。
7. 测试：fake backend 成功/失败/超时/页数错误/取消；stdout 洪水不死锁；源哈希变化不发布陈旧结果；
   崩溃注入（staging 中、rename 前、rename 后 DB 前）重启无半成品 ready；cascade 防假绿。

**完成标准（M1 后端侧）**：真实扫描 PDF 在 macOS 上从 `ocr_start` 到资产 ready、Reader 拿到新路径全程可跑，全套测试绿。

### W2 — 扩展打包与安装管理

**文件**：`scripts/ocr-runtime/**`（新建）、`.github/workflows/ocr-runtime.yml`（新建）、
`src-tauri/src/commands/ocr/package.rs`（新建，含其 Tauri commands 注册段——与 W1 协调各自追加，不改对方行）。

任务：

1. **macOS arm64 自包含 runtime**：python-build-standalone（或等价自包含 Python 3.12+）+ `ocrmypdf` + `pypdfium2`
   + Tesseract 二进制及依赖 + `tessdata_fast` 的 `eng`/`chi_sim` + §5.3 的 JSONL 进度 plugin，打成 `tar.zst`。
   目录形态（非 one-file 冻结），附 `THIRD_PARTY_NOTICES` 与 SBOM 清单。macOS 12+ 实测可运行。
2. **Windows x64 包**：同构成，在 GitHub Actions windows runner 构建产出 artifact。**只要求构建成功**；
   runner 上跑一次 `--version` + 单页 fixture smoke 是免费的，可做但不阻塞。
3. **manifest**：`{package_id, version, platform, arch, minimum_os_version, download_size, installed_size, sha256, url}`，
   与包一起发布到本仓库 GitHub Release（tag 如 `ocr-runtime-v1`）。UI 只显示 manifest 真实字节数，不硬编码估算。
4. **OcrPackageManager**（`package.rs`）：Rust 后端下载（非 webview fetch）到 `.partial` → 校验长度 + SHA-256 →
   解压到版本化临时目录（拒绝绝对路径/`..`/符号链接逃逸）→ self-test（`ocrmypdf --version` + 内置单页 fixture OCR）→
   原子切换 `current` 指针 → 清理旧版本。卸载：有活跃任务先要求取消；不删任何书籍资产。
   实现 §5.1 的 package commands，发 `ocr-package-changed`。
5. 测试：fake HTTP server 下断线重试、哈希不符拒装、解包逃逸拒绝、self-test 失败回滚、卸载后资产仍在。

**完成标准**：macOS 真机从设置一键下载→安装→self-test 通过→W1 backend 能调用；Windows 包在 CI 构建成功。

### W3 — 前端：设置与 Reader 交互

**文件**：`src/**`（组件、hooks、i18n）。对 §5.1/5.2 契约开发，后端未就绪时用 mock。

任务：

1. **设置子视图**：「阅读辅助 → 扫描件 OCR」，沿用 `GeneralSettings.tsx` 行模式与 `ROW_CONTROL_WIDTH`。
   两个区块（**没有**质量档位区块）：
   - OCR 组件：未安装/下载中(字节进度)/校验中/安装中/已安装(版本+占用)/失败(摘要+可复制详情)；下载、重试、卸载操作。
   - 存储：已识别书籍总占用、逐书列表、删除（本机释放 / 从所有设备删除——后者 M2 前置灰）。
2. **设置深链接**：`open-settings` payload 向后兼容扩展为 `SettingsSection | { section, view: "ocr" }`；
   Reader CTA 经后端命令统一 show + 聚焦主窗口再发结构化 destination。
3. **Reader OCR controller**：现底部提示条升级为 `pointer-events-auto` 非模态操作条，与快捷键 HUD 同容器
   分优先级，一次交互只出一条。触发限于「用户想用文字」的动作（单击/双击/拖选后空 Selection、右键、文字型快捷键）；
   翻页/缩放/滚动不触发；按 `book_id+page+source_hash` 缓存节流。状态矩阵：未安装→前往下载；下载中→进度；
   已装未识别→开始识别；排队→取消；识别中→页数进度或不确定进度+取消；正在完成→无操作；失败→重试/详情；
   完成→无提示。
4. **完成切换**：监听 `book-assets-changed`；记录页码/缩放/模式/双页 → 仅本机 `available_verified` 且页数一致才
   重载恢复同页 → 新文字层实际渲染后顶部 Toast（`role=status`，2.5–5 s，不抢焦点）：「文字识别完成，现在可以
   选词和使用 AI 功能」。失败保持原 PDF + 重试，不白屏。远端同步来的资产不打断当前阅读，下次打开生效。
5. hooks（`useOcrPackage`、`useOcrJob`）：mount 时主动查询全量状态，事件只做增量。
6. i18n：全部文案进 `src/i18n/en.json` + `zh.json`，前缀 `ocr.*`；Rust 错误码由前端映射文案。

**完成标准**：mock 下全部状态可走查；对接真实命令后 M1 全链路可用；`npx tsc --noEmit` + lint + 前端单测绿。

### W4 — 同步 schema v7

**文件**：`src-tauri/src/sync/**`；M2 集成时另改 W1 的 `publish.rs`（仅该函数）。

任务：

1. `EVENT_SCHEMA_VERSION` 6→7（`src-tauri/src/sync/events.rs:25`）；新事件：
   - `book.asset.publish`：携带 `book_assets` 整行不可变元数据；
   - `book.asset.delete`：tombstone，阻止离线设备复活已删资产。
   无 gating、无 ratchet、无 preferred 事件（裁决 #5/#6）。
2. validation：新事件的 `relative_path` 必须匹配 `books/{book_id}.ocr.{asset_id}.pdf` 模式并通过既有
   `validate_book_path` 级穿越/扩展名检查。
3. merge/replay：资产事件落 `book_assets`；接收侧初始 `book_asset_local_state = remote_only`；
   发现 `.icloud` placeholder 触发物化 → 下载完成校验 size+SHA-256 → `available_verified`；
   哈希不符 → `corrupt` 保留 source 可重试。文件先到事件未到 = 暂时孤儿，不激活。
4. snapshot v7：包含 `book_assets`；tombstone 进 snapshot；`local_state`、`ocr_jobs` 永不进入。
5. 删除语义：「从所有设备删除」在同一事务写 tombstone + 回退（本机 local_state 清理）→ 提交后 best-effort
   删共享文件；顺序不可反。整书删除 cascade 联动资产 tombstone。
6. **M2 集成**：把 W1 `publish_verified_output` 的事务换成 `SyncWriter.with_tx()`，同事务插资产行 + outbox
   `book.asset.publish`。发布顺序不变量：文件先落盘并验证，事件后写；禁止先事件后文件。
7. 测试：事件 roundtrip/validation/merge/snapshot；事件先到 blob 后到、blob 先到事件后到、placeholder、
   哈希错误；双设备各自生成同书资产共存（唯一 id/文件名防覆盖）；删除 tombstone 对离线设备生效；
   删整书后延迟 publish 不复活；OCR 期间开关同步不破坏 data_dir 不变量。

**完成标准**：双设备（或双 data_dir 模拟）A 生成 → B 物化校验后打开即用 OCR 版；上述测试全绿。

## 5. 接口契约（并行开发的锚点，改动需四方同步）

### 5.1 Tauri commands

```ts
// package（W2 实现，W3 消费）
ocr_package_status(): OcrPackageStatus
ocr_package_download(): void
ocr_package_cancel(): void
ocr_package_uninstall(): void

// jobs（W1 实现，W3 消费）
ocr_start(bookId: string): void          // 已在队列/运行中则幂等返回
ocr_cancel(bookId: string): void
ocr_retry(bookId: string): void
ocr_job_status(bookId: string): OcrJobView | null

// assets（W1 实现，W3 消费；all_devices 分支 M2 起生效）
ocr_assets_overview(): { totalBytes: number; items: OcrAssetItem[] }
ocr_asset_delete(assetId: string, allDevices: boolean): void
```

```ts
type OcrPackageStatus = {
  state: "not_installed" | "downloading" | "verifying" | "installing"
       | "installed" | "uninstalling" | "failed";
  version?: string; downloadedBytes?: number; totalBytes?: number;
  installedBytes?: number; errorCode?: string;
};
type OcrJobView = {
  state: "queued" | "waiting_source" | "preparing" | "recognizing"
       | "validating" | "publishing" | "ready" | "failed" | "cancelled";
  pagesDone?: number; pagesTotal?: number; errorCode?: string;
};
type OcrAssetItem = {
  assetId: string; bookId: string; title: string;
  byteSize: number; createdAt: number; availability: string;
};
```

### 5.2 前端事件

`ocr-package-changed`（W2 发）、`ocr-job-changed`（W1 发，≤4 Hz）、`book-assets-changed`（W1/W4 发）。
载荷即对应 status DTO；窗口 mount 主动查询，事件只做增量唤醒。

### 5.3 进度 JSONL（W2 的 plugin 产出，W1 解析）

```json
{"type":"phase","phase":"analyzing"}
{"type":"progress","phase":"ocr","completed":36,"total":284}
{"type":"phase","phase":"finalizing"}
{"type":"warning","code":"PAGE_TIMEOUT","page":117}
{"type":"complete","pages":284,"ocr_pages":278,"skipped_pages":6}
```

拿不到可信页级进度时 UI 退化为不确定进度条；`complete` 后仍有验证/发布，Reader 显示「正在完成」，不得提前 Toast。

### 5.4 W1↔W2 运行时接口

`package.rs` 暴露 `fn installed_runtime() -> Option<RuntimeInfo { root: PathBuf, version: String }>`；
W1 backend 由此取可执行入口与 `TESSDATA_PREFIX`，不自行探测 PATH。卸载与活跃任务互斥由双方各自持锁检查。

### 5.5 W1↔W4 发布接缝

M1 期间 `publish_verified_output` 只做本地事务；M2 由 W4 在同一函数内换成 `SyncWriter.with_tx()` + outbox。
函数签名与调用点不变，避免并行期互相阻塞。

## 6. 里程碑与集成顺序

```
M0  W1 处置原型并落地数据基础 commit（其余工作流的开工信号）
      ↓（此后 W1/W2/W3/W4 全并行）
M1  macOS 本地闭环：设置装扩展 → 扫描书识别 → Reader 选词/AI 可用（W1+W2+W3）
M2  同步闭环：v7 事件 + 资产双机同步（W4 + 发布接缝集成）
M3  Windows 包 CI 构建通过；发布（feature flag 移除，/release 流程）
```

- M1 与 W4 的开发互不阻塞；只有 M2 的接缝集成（§5.5）需要 W1 的 `publish.rs` 已存在。
- 发布顺序：M2 合入后所有设备升级同一版本（内测规则，无兼容窗口）。

## 7. 验收清单（v1 发布前）

- [ ] macOS：下载→安装→self-test→识别→选词/复制/高亮/翻译/Ask AI 全链路真机通过
- [ ] 混合 PDF 只识别无文字页；已有文字页不被重栅格化；页数/视觉不变
- [ ] OCR 期间可继续阅读原件；取消/失败/崩溃注入后原 PDF 完好、无半成品 ready、可重试
- [ ] 卸载 runtime 不影响已生成资产；删资产不删原书
- [ ] 双设备同步：A 识别 → B 校验后可用；乱序/placeholder/哈希错误不白屏、不激活坏文件
- [ ] grounding 自动从 `PDF_TEXT_LAYER_UNAVAILABLE` 恢复并索引 active asset
- [ ] Windows runtime 包 CI 构建成功（弱验证即可）
- [ ] 中英文 i18n 完整；Toast/状态条可访问（`role=status`，不抢焦点）
- [ ] 全部 Rust/前端测试与 clippy/lint 绿；SBOM 与 THIRD_PARTY_NOTICES 随包发布
