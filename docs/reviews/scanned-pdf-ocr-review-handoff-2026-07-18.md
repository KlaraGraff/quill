# 扫描 PDF OCR 与可同步派生资产升级方案：独立审核交接稿

> 文档状态：**方案已对齐，实施进行中；尚未完成最终 go/no-go**
>
> 整理日期：2026-07-18
>
> 目标读者：对 Lantern 代码库不熟悉、需要独立审查方案可行性与优化空间的 AI 或工程师
>
> 关联实施计划：[扫描 PDF OCR 与可同步派生资产 — 实施方案](../impls/scanned-pdf-ocr-pipeline.md)
>
> 关联 PoC：[Scanned PDF OCR Phase C PoC Report](scanned-pdf-ocr-phase-c-poc-2026-07-18.md)
>
> 早期完整评审稿：[扫描 PDF OCR、可选文字与跨设备派生资产升级方案](scanned-pdf-ocr-upgrade-proposal-2026-07-17.md)

早期评审稿仍保留了部分历史候选表述（例如 macOS 11 目标、约 100 MB 扩展占位和未裁剪的 asset position 设计）。本文和当前实施计划的修正优先于早期稿；早期稿只作为背景来源。

### 证据等级

- **已实施**：代码已合入本地 `main`，但不等于已经随用户可下载版本发布。
- **PoC 已证**：在临时环境/控制样本上跑通，不等于真实语料或发布包通过。
- **设计草案**：SQL、模块树和协议字段用于讨论，尚未在仓库落地。
- **估算**：容量、工期或资源启发式，必须用最终包和目标设备实测替换。
- **待验证**：尚无足够证据，不能作为功能承诺。

## 0. 本文用途与审核约定

本文不是面向用户的宣传稿，也不是已经完成的功能说明。它把目前已经对齐的产品要求、代码背景、架构裁决、PoC 证据、阶段边界、风险和验收条件集中到一份文档，供另一个 AI 做独立技术审核。

审核时请严格区分三类信息：

1. **已验证事实**：已经对代码、二进制或 PoC 产物做过检查，本文会附证据或状态。
2. **已对齐决定**：产品和工程方向已经确定，除非发现致命风险，不应重新发散到所有历史候选路线。
3. **开放门槛**：仍需样本、目标系统、签名安装包或完整 Reader 测试证明；在通过前不能当成发布承诺。

希望审核者重点回答：

- 主路线是否能在 Lantern 现有架构中安全落地；
- 数据模型和同步协议是否存在会导致数据丢失、旧客户端停摆或派生文件不可达的问题；
- macOS Vision + PDFium 与 Windows OCRmyPDF/Tesseract 的双后端策略是否合理；
- Phase D 与 Phase G 的切分是否会产生半成品协议或错误 outbox；
- 自动并发、扩展安装、卸载、任务恢复和 Reader 状态是否遗漏关键边界；
- 是否有更小、更稳但仍满足跨设备与 AI 功能的首发范围。

## 1. 一句话方案与执行摘要

### 1.1 一句话方案

Lantern 在桌面端按需对扫描 PDF 做本地 OCR，生成一份**视觉页面不变、增加不可见文字层的标准 searchable PDF**；原始 PDF 永久保留，新 PDF 作为不可变、可校验、可同步的书籍派生资产，通过现有 PDF.js/Foliate 阅读链路提供选词、选句、高亮、复制、翻译和 AI 查询。未来具备相应 PDF Reader/同步协议实现的其他设备可以消费 searchable PDF，不重复执行 OCR；当前仓库和发布范围仍是桌面端，手机消费者尚未交付。

### 1.2 当前推荐路线

```text
原始扫描 PDF（始终保留）
        │
        ├─ macOS 候选：Apple Vision accurate + 现有 PDFium 写入不可见文字层
        │
        └─ Windows/回退候选：OCRmyPDF + Tesseract + pypdfium
                     │
                     ▼
       临时 searchable PDF（未验证，不可发布）
                     │
       页数/几何/渲染/文字层/SHA-256 验证
                     │
                     ▼
       不可变 book_asset + 本机 available_verified
                     │
          iCloud 同步元数据和 PDF 字节
                     │
                     ▼
      其他桌面或手机直接使用标准 PDF 文字层
```

### 1.3 为什么第一产物不是 EPUB

第一期选择 searchable PDF，而不是把扫描书直接转换为 EPUB，原因是：

- OCR 只能得到文字和坐标，不能可靠恢复章节、段落、图片浮动、脚注、双栏阅读顺序和语义结构；
- PDF → 可重排 EPUB 还会引入版面重建、位置迁移、字体和图片处理等另一组风险；
- Lantern 当前 PDF 阅读器已经依赖 PDF.js 的标准文字层，增加文字层后可以直接复用现有选择和 AI 链路；
- searchable PDF 与原 PDF 页数一致，首期可以共享页码和粗粒度阅读进度；
- 手机端只需支持标准 PDF，无需实现 OCR 或一套自定义透明覆盖层。

因此“扫描 PDF → 可重排 EPUB”保留为未来独立项目，不是本次升级的发布条件。

### 1.4 当前总体结论

- **架构方向成立**：标准 searchable PDF 是风险最低、与现有 Reader 和 AI 链路衔接最自然的首产物。
- **macOS 技术候选已得到强 PoC 证据**：Vision + Lantern 现有 PDFium 已完成单页识别、文字层写入、PDF.js 提取和跨行 DOM Range 闭环。
- **尚不能宣布最终 go**：合法真实扫描语料、signed `.app`、macOS 12 真机、复杂 PDF 结构、完整 Reader 鼠标交互和 Windows 包仍未通过。
- **同步资产是本项目的核心，不是附加项**：若 searchable PDF 不同步，手机端和未安装 OCR 的设备无法消费结果，违背已确认需求。

## 2. 产品背景与用户要求

### 2.1 用户问题

扫描 PDF 的页面本质上是图片。即使肉眼看到中文或英文，PDF 内也可能没有可提取的文字对象，因此 Lantern 当前会出现：

- 单击、双击或拖动无法产生文本 Selection；
- 无法复制、选词、选句和创建文字高亮；
- 翻译、解释、查词和基于选区的 AI 功能没有输入文本；
- AI grounding 对整本书提取到的文字过少，返回 `PDF_TEXT_LAYER_UNAVAILABLE`；
- 手机上即使同步了原 PDF，也仍然只能看图，不能直接使用文字型 AI 功能。

### 2.2 已确认的用户体验要求

1. OCR 能力是**可选组件**，不强制增大 Lantern 主安装包。
2. 设置中可以下载、更新或卸载 OCR 组件。
3. 标准/快速模型和高精度模型独立管理；高精度模型可单独下载或卸载。
4. 不向用户显示“并发数”设置；应用按 CPU、内存和历史失败情况自动选择。
5. 未安装 OCR 时，用户在无文字层页面尝试点击选词、拖选、右键或触发文字型快捷键，应出现可交互提示；点击后打开设置并定位到 OCR 下载区。
6. OCR 进行中，同一交互显示“排队中/识别中/正在完成”等当前状态，不创建重复任务。
7. OCR 完成后，Reader 在文件验证成功并切换到新文字层之后，于顶部显示轻量 Toast；不弹模态框、不抢焦点。
8. 转换期间仍可继续阅读原 PDF，翻页、缩放和滚动不受阻塞。
9. 原始 PDF 不覆盖、不删除；已生成的 searchable PDF 即使卸载 OCR 组件仍可使用。
10. searchable PDF 应随书库同步；手机或另一台电脑不安装 OCR 组件也能使用已识别结果。
11. Lantern 项目继续开源并保持 MIT；第三方依赖按各自许可证履行义务。

### 2.3 产品约束

- Lantern 是本地优先阅读器，OCR 默认只在本机处理，不上传书页到云端 OCR 服务。
- OCR 全本识别本身不上传整本书或整页；用户显式触发查词、翻译或 Ask AI 时，所选文本及既有 grounding 片段仍可能按 Lantern 现有隐私设置发送给用户配置的 AI provider，这不是 OCR 新增的云端处理路径。
- 主平台是 Apple Silicon macOS；Windows 11 x64 为次要桌面平台。
- 当前 iCloud 文件同步只在 macOS 上提供；“手机端”是协议和资产消费目标，不等于当前仓库已经包含完整移动客户端。
- 需要保护现有书库、进度、高亮和同步日志，不能为了 OCR 牺牲数据安全。
- OCR 可能持续数分钟甚至更久，不能占用 webview 主线程或长时间持有同步锁。

## 3. 术语

| 术语 | 本文含义 |
|---|---|
| 扫描 PDF | 一页或多页主要由图像构成、没有足够可用文字层的 PDF；混合 PDF 也可能只在部分页面需要 OCR |
| searchable PDF | 保留固定版面和可见页面，同时加入不可见文字对象与 Unicode 映射的 PDF |
| source asset | 用户导入的原始文件；在本项目中始终保留 |
| derived asset | 从 source 生成、带来源和管线信息的不可变文件，如 OCR PDF |
| active asset | 当前设备经 resolver 判定后实际交给 Reader 或 grounding 的可用文件 |
| preferred asset | 同步的用户偏好指针；它不代表文件在当前设备已经下载和验证 |
| local availability | 资产在当前设备的状态，例如远端、下载中、已校验、损坏或缺失 |
| OCR runtime | 执行 OCR 所需的引擎、wrapper 和原生依赖，不包含已生成书籍资产 |
| fast / best | Tesseract 标准速度档和高精度模型档；“best”是模型系列名称，不保证每本书都更准确 |
| gating | 在确认所有活跃 peer 能理解新事件 schema 前，阻止发送 v7 资产事件的兼容机制 |

## 4. Lantern 当前架构与已核实事实

### 4.1 当前技术栈

- 前端：React 19、TypeScript、Tailwind CSS 4、Vite。
- 桌面后端：Tauri 2、Rust、SQLite。
- Reader：仓库内 vendored `foliate-js`；PDF 由 PDF.js 链路渲染和产生文字层。
- AI：OpenAI-compatible provider 与本地 grounding 索引。
- 同步：本机 SQLite 物化视图 + iCloud 文件夹内 per-device JSONL 事件日志、snapshot、书籍与封面 blob。

### 4.2 当前书籍模型只有一个主阅读文件

现有 `books` 表和同步 `BookImport`/snapshot 主要围绕一个 `file_path` 工作，并同时保存：

- `source_format` / `render_format`；
- `source_file_path` / `source_sha256`；
- `conversion_version` / `preparation_state`；
- 单一 `progress` / `current_cfi`。

它无法完整表达同一本逻辑书同时拥有：

- 原始扫描 PDF；
- fast OCR PDF；
- best OCR PDF；
- 未来 EPUB；
- 每个资产独立的来源、哈希、生成器、模型和可用状态。

仅把 `books.file_path` 原地改成 OCR 输出会丢失来源关系，也会让旧客户端、重做 OCR、高亮稳定性和文件回退变得不可靠。因此需要 `book_assets`，而不是覆盖原行。

### 4.3 当前格式准备管线不能直接照搬

`src-tauri/src/commands/books/convert_prepare.rs` 已有值得复用的工程模式：

- 源哈希快照；
- guarded update，防止旧任务覆盖新源；
- 临时文件和原子 rename；
- 崩溃恢复；
- 派生文件丢失后重新生成。

但它把 MOBI/AZW → EPUB 产物定义为 `prepared/` 下的**本地、不参与同步的缓存**，要求每台设备自行重转；`preparation_state != ready` 还会阻止开书。OCR 不能沿用这两个语义：

- OCR 期间必须仍可读原 PDF；
- 手机端不执行 OCR；
- 派生 searchable PDF 必须同步；
- 不同设备重复 OCR 会浪费资源并产生不同文字层。

结论：复用任务安全模式，不复用缓存和阻塞阅读语义。

### 4.4 当前 Reader 已有触发入口

`src/pages/reader/useReaderInteractions.ts` 已在 PDF 点击和右键交互中检查文字层/Selection。`src/pages/Reader.tsx` 已有“无文字层”提示容器。

这意味着首期不需要重新设计整个选择系统，只需：

- 在空文字层动作前由 OCR controller 统一裁决状态；
- 把当前不可交互的提示升级成带操作按钮的非模态条；
- OCR 完成后重新加载标准 PDF，让现有 PDF.js 文字层继续负责选择。

### 4.5 当前 AI 扫描判断只适合 grounding，不适合产品触发

`src-tauri/src/ai/grounding/extract.rs` 当前用：

```text
page_count > 5 && total_chars < 500
```

判定整本 PDF 是否像扫描件。它会漏掉：

- 5 页以内的扫描文件；
- 大部分页面有文字、少数页面为扫描图的混合 PDF；
- 当前正在阅读的单个无文字层页面。

因此产品入口以**当前页文字层**为准；整书启发式只可作为预热或提示缓存，不能单独决定下载或自动 OCR。

### 4.6 当前同步与 outbox

同步数据结构：

```text
data_dir/
  books/       原始 EPUB/PDF；iCloud 同步
  sources/     部分格式的源文件；iCloud 同步
  covers/
  logs/        每设备 append-only JSONL
  snapshots/
```

所有同步 mutation 应通过 `SyncWriter.with_tx()`：SQLite 修改与 `_pending_publish` outbox 入队在同一个事务；iCloud 暂时不可用时，事件留在 outbox 后续重试。

OCR 资产发布必须继续遵守这个不变量，但**只能在 schema v7 的完整实现同时上线后写资产事件**。Phase D 不能把 v7 body 塞进 v6 envelope，也不能提前向现有 outbox 写无法被当前 snapshot/replay 理解的资产事件。

### 4.7 已修复的 P0 路径契约缺陷

早期审查发现：原生 EPUB/PDF 导入会把 `source_file_path` 写成 `books/...`，文本导入又可能把 `file_path` 写成 `sources/...`；旧验证器却把两个字段分别锁死在单一路径根。对端 replay 遇到这种合法事件会拒绝整份 peer 日志，含文本书的 snapshot 也可能无法应用。

Phase A 已完成：

- commit：`75139ff`；
- `file_path` 与 `source_file_path` 都通过同一个角色无关、根目录白名单验证器；
- 允许经过验证的 `books/` 或 `sources/`；
- event、snapshot、replay 统一规则；
- 增加原生书、文本书和双设备 replay 回归测试。

### 4.8 已完成的同步能力通告

当前事件 schema 为 v6。旧客户端遇到不支持的完整事件会拒绝该 peer 整份日志而不推进 watermark，这是防止静默丢事件的安全语义，不准备修改。

Phase B 已完成：

- commit：`1faf519`；
- peer manifest 新增 `max_event_schema`；
- 新 manifest 写当前 `EVENT_SCHEMA_VERSION`；
- 缺失字段固定按旧 schema 6 解析；
- Rust `PeerInfo` 和前端同步 DTO 透出该字段；
- 保持 manifest 原子写入、UUID 验证与路径保护。

它目前只通告能力，不改变发送行为；真正的 v7 gating 在 Phase G 完成。

### 4.9 当前最低系统事实

Lantern 捆绑的 `libpdfium.dylib` 约 6.8 MiB，Mach-O `minos` 实测为 macOS 12.0。因此即使不做 OCR，当前 PDFium 也不能支持 macOS 11。工作副本已经把 README 基线修正为 macOS 12 Monterey+；这不是 OCR 单独提高的要求，而是对现有二进制事实的纠正。

Apple Vision Revision 2 的 accurate 模式从 macOS 11 起支持中文；Revision 3 从 macOS 13 起提供旋转、手写和语言方面改进。产品仍以 macOS 12 为最低版本，因为 PDFium 已决定更高下限。

## 5. 已对齐的产品与架构决定

| # | 决定 | 状态 | 理由 |
|---:|---|---|---|
| 1 | 首个正式产物是 searchable PDF，不是 EPUB | 已确认 | 复用 PDF.js 文字层，避免首期承担版面重建和跨格式锚点迁移 |
| 2 | 原始 PDF 永久保留，禁止覆盖 | 已确认 | 数据安全、重做 OCR、审计和旧端回退 |
| 3 | searchable PDF 是同步派生资产，不是 disposable cache | 已确认 | 手机和未安装 OCR 的设备必须能直接消费 |
| 4 | 手机端只消费资产，不执行 OCR | 已确认 | 降低移动端包体、耗电和实现复杂度 |
| 5 | OCR runtime/model 按需下载、可更新、可卸载 | 已确认 | 主包保持轻量 |
| 6 | fast 默认；best 独立下载/卸载 | 已确认 | 用户可选择质量档，不强制大模型 |
| 7 | 不向用户提供并发设置 | 已确认 | 后端自动按资源选择，减少误配置 |
| 8 | 未安装/安装中/未识别/排队/转换中/完成中/失败/完成都有明确 Reader 状态 | 已确认 | 用户动作有可解释反馈且不重复启动任务 |
| 9 | 完成后在顶部显示轻量 Toast | 已确认 | 不打断阅读；只有真正切换并渲染文字层后才提示 |
| 10 | Tesseract.js + 实时透明覆盖层不作为主路线 | 已确认 | 选择、坐标、高亮和移动端同步复杂度过高 |
| 11 | macOS 优先评估 Vision + 现有 PDFium | 候选优先级已确认，最终后端待 Phase C | 已有单页闭环，可能做到 macOS 零 OCR runtime 下载 |
| 12 | Windows 暂保留 OCRmyPDF/Tesseract | 候选，待 Windows 验证 | Windows.Media.Ocr 尚无质量和打包证据 |
| 13 | v1 不做 OSD/自动旋转 | 已确认 | 少约 10 MiB 模型和额外耗时；待真实样本再决定 |
| 14 | v1 不建 `book_asset_positions` | 已确认 | 首期仅同页数 PDF，先复用现有页码/进度；EPUB 进入范围时再建 |
| 15 | v1 不做 EPUB 派生、跨格式高亮迁移 | 已确认 | 作为独立后续项目 |
| 16 | 不改旧客户端未知事件解析语义 | 已确认 | 防止 watermark 越过无法理解的事件 |
| 17 | v7 通过 peer manifest 能力通告 + 发送端 fail-closed gating | 已确认 | 避免 v6 peer 因收到 v7 事件而停止后续同步 |
| 18 | Lantern 保持 MIT 和全项目开源 | 已确认 | 但第三方包仍保留自身许可证和 notices |
| 19 | 引擎/模型更新不自动重跑已有书 | 已确认 | 用户显式重做才创建新 immutable asset，避免高亮、同步体积和结果悄然变化 |

## 6. 第一版范围与非范围

### 6.1 第一版目标

- 按需识别扫描或混合 PDF 的无文字层页面；
- 生成标准 searchable PDF，视觉页面和页数保持一致；
- 支持中文简体 + 英文默认配置；
- 支持 fast 与 best 两种 Tesseract 质量档，或在 macOS 使用 Vision；
- 支持选词、选句、复制、高亮、翻译和 AI 查询；
- 保留原 PDF，并可在资产不可用时回退；
- 派生 PDF 在桌面设备间同步，并为未来手机消费提供标准资产协议；
- 支持下载、校验、原子安装、更新、卸载和 self-test；
- 支持自动并发、结构化进度、取消、失败重试和崩溃恢复；
- 完成同步 schema v7、snapshot、merge、tombstone 与兼容 gating。

### 6.2 第一版明确不做

- 不自动把扫描 PDF 转成可重排 EPUB；
- 不自动删除原 PDF；
- 不在导入时自动下载 OCR 或自动开始长任务；
- 不把 OCR 页面上传给第三方云服务；
- 不做实时 Tesseract.js DOM 覆盖层；
- 不默认 `--deskew`、`--clean`、`--remove-background` 或 `--force-ocr`；
- 不保证竖排古籍、手写体或任意复杂版面达到出版级准确率；
- 不支持逐页 checkpoint/resume；取消或崩溃后首版从头重试；
- 不承诺 fast/high-accuracy 之间自动迁移精确高亮；
- 不在 v1 引入 `book_asset_positions` 或 PDF/EPUB 位置空间重构；
- 不把现有 MOBI/AZW → EPUB `prepared/` 管线同时迁移到 `book_assets`。

## 7. 总体组件边界

### 7.1 组件分工

```text
React Settings / Reader
  ├─ 查询 package/job/asset 状态
  ├─ 发起下载、卸载、开始、取消、重试
  └─ 显示非模态状态条、进度、错误和完成 Toast

Rust OCR commands
  ├─ OcrPackageManager：包 manifest、下载、哈希、签名、自检、原子切换
  ├─ OcrJobManager：队列、持久状态、资源判断、取消、恢复
  ├─ OcrBackend：Vision 或 OCRmyPDF/Tesseract 的统一接口
  ├─ OutputValidator：PDFium 解析、页数/几何/渲染/文字层检查
  ├─ BookAssetRepository：资产元数据和本机可用性
  └─ ActiveAssetResolver：Reader 与 grounding 的统一活动文件选择

Sync layer（Phase G）
  ├─ schema v7 asset events
  ├─ snapshot / merge / tombstone
  ├─ peer capability gating
  └─ iCloud placeholder、下载、大小与 SHA-256 校验
```

### 7.2 `OcrBackend` 抽象要求

后端应有统一的概念接口：

```rust
trait OcrBackend {
    fn probe(&self) -> BackendCapabilities;
    fn recognize_pdf(
        &self,
        request: OcrRequest,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<OcrOutput, OcrError>;
}
```

边界约束：

- 后端只读源文件，只写指定 staging 输出；
- 后端不直接修改 SQLite、不发同步事件、不选择活动资产；
- 调用外部程序时使用参数数组，不拼 shell 字符串；
- 用户文件名不能被解释为命令行选项，必要时使用 `--`；
- stdout/stderr 持续 drain，防止子进程管道堵塞；
- 所有后端输出进入同一个验证器，不能因平台不同降低验收标准。

### 7.3 建议模块布局

```text
src-tauri/src/commands/ocr/
  mod.rs
  package.rs
  backend.rs
  vision.rs
  ocrmypdf.rs
  jobs.rs
  validate.rs
  publish.rs
  resolver.rs
```

实际实现可以按仓库模式调整，但职责边界不应合并成一个既跑 OCR、又写数据库、又发事件的长函数。

## 8. 数据模型

### 8.1 `book_assets`

当前对齐的 migration 草案：

```sql
CREATE TABLE book_assets (
  id                  TEXT PRIMARY KEY,
  book_id             TEXT NOT NULL REFERENCES books(id),
  role                TEXT NOT NULL,             -- source | ocr_pdf
  format              TEXT NOT NULL,             -- pdf | epub；现有 text/html/markdown 是否纳入需定稿
  relative_path       TEXT NOT NULL,             -- source: books/ or sources/; derived: books/
  content_sha256      TEXT,                      -- legacy source 物化前可空
  byte_size           INTEGER,                   -- legacy source 物化前可空
  source_asset_id     TEXT,
  source_sha256       TEXT,
  pipeline            TEXT,                      -- apple_vision | ocrmypdf | ...
  pipeline_version    TEXT,
  language_profile    TEXT,                      -- zh-Hans+en 或 chi_sim+eng
  quality_profile     TEXT,                      -- fast | best | accurate
  conversion_version  INTEGER NOT NULL DEFAULT 1,
  page_count          INTEGER,
  supersedes_asset_id TEXT,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL,
  updated_by_device   TEXT NOT NULL
);

CREATE UNIQUE INDEX book_assets_relative_path_idx
  ON book_assets(relative_path);

CREATE UNIQUE INDEX book_assets_one_source_idx
  ON book_assets(book_id) WHERE role = 'source';

CREATE INDEX book_assets_book_updated_idx
  ON book_assets(book_id, updated_at DESC);
```

关键约束：

- source asset ID 由 `book_id` 确定性生成，例如 UUIDv5；两台离线设备迁移同一本书不能得到两个 source ID；
- UUIDv5 由 Rust 幂等 backfill 完成，不假设 SQLite migration 能直接计算；
- legacy source 在 blob 尚未物化时允许 hash/size 暂为空；
- 所有 derived asset 发布前必须有 `content_sha256`、`byte_size`、`source_sha256`；
- derived asset 发布后不可原地改变；重做 OCR 永远创建新 `asset_id`；
- `supersedes_asset_id` 只表达替代关系，不自动删除旧资产；
- 物理文件名必须包含唯一 `asset_id`，避免两台设备并发生成或同配置重做时覆盖。
- Lantern 现有导入还包括 txt/markdown/html、MOBI/AZW/AZW3 等格式；需要明确是把这些源也桥接成 `role=source` asset，还是只为 PDF/EPUB 建 asset、其他格式继续使用 legacy bridge，不能让 migration 漏掉已有文本书和格式转换路径。

建议文件名：

```text
books/{book_id}.ocr-pdf.v{conversion_version}.{source_hash_prefix}.{profile}.{asset_id}.pdf
```

第一期继续平铺在 `books/`。当前 `move_dir_contents()` 只处理一层文件；改成 `assets/{book_id}/...` 会同时扩大同步启用、关闭和 copy-back 的数据安全范围。

### 8.2 本机可用性必须与同步元数据分离

```sql
CREATE TABLE book_asset_local_state (
  asset_id       TEXT PRIMARY KEY REFERENCES book_assets(id),
  book_id        TEXT NOT NULL,
  availability   TEXT NOT NULL,
  -- remote_only | downloading | available_verified | corrupt | missing
  observed_size  INTEGER,
  observed_mtime INTEGER,
  verified_at    INTEGER,
  error_code     TEXT,
  updated_at     INTEGER NOT NULL
);
```

理由：

- peer 可能先收到资产事件，后收到 PDF；
- iCloud 可能只留下 `.icloud` placeholder；
- 文件存在不代表完整或哈希正确；
- 同步的 `preferred_asset_id` 可能指向当前设备尚不可用的文件；
- 本机下载进度和损坏状态不应进入全局事件日志或 snapshot。
- legacy source 的 hash/size 为空时，不能因为路径存在就标成 `available_verified`；首次物化必须重新 stat、读取并计算 SHA-256，OCR job 应先处于 `waiting_source`/hashing，随后把 source hash 固化用于陈旧任务保护。

### 8.3 `preferred_asset_id` 与活动资产 resolver

`books` 新增可空的同步偏好：

```sql
ALTER TABLE books ADD COLUMN preferred_asset_id TEXT;
```

实际 active asset 由当前设备 resolver 决定：

1. 本机明确选择且 `available_verified` 的受支持资产；
2. 同步 `preferred_asset_id` 且本机已校验；
3. 最新、受支持且本机已校验的 searchable PDF；
4. source asset；
5. 若 source 也是远端 placeholder，则触发下载并显示等待，而不是指向不存在的路径。

Reader 和 AI grounding 必须使用同一个 resolver，避免 UI 已读 OCR PDF、索引器却继续读取原始扫描件。

### 8.4 本机 OCR 任务

```sql
CREATE TABLE ocr_jobs (
  id                 TEXT PRIMARY KEY,
  book_id            TEXT NOT NULL,
  source_asset_id    TEXT,
  source_sha256      TEXT NOT NULL,
  state              TEXT NOT NULL,
  phase              TEXT,
  pages_done         INTEGER,
  pages_total        INTEGER,
  backend            TEXT,
  backend_version    TEXT,
  language_profile   TEXT,
  quality_profile    TEXT,
  jobs               INTEGER,
  conversion_version INTEGER NOT NULL DEFAULT 1,
  result_asset_id    TEXT,
  recognized_pages   INTEGER,
  skipped_pages      INTEGER,
  timed_out_pages    INTEGER,
  failed_pages       INTEGER,
  temporary_path     TEXT,
  error_code         TEXT,
  error_detail       TEXT,
  created_at         INTEGER,
  started_at         INTEGER,
  updated_at         INTEGER
);

CREATE UNIQUE INDEX ocr_jobs_one_active_idx ON ocr_jobs(book_id)
  WHERE state IN (
    'queued', 'waiting_source', 'preparing',
    'recognizing', 'validating', 'publishing'
  );
```

`ocr_jobs` 是设备本地执行状态，永不进入同步事件或 snapshot。

### 8.5 v1 不建 `book_asset_positions`

这是相对早期评审稿的有意裁剪：

- 首期 source PDF 与 OCR PDF 页数必须相同；
- 扫描源页原本没有可用文字 CFI，高亮主要在 OCR 资产产生后创建；
- 先复用 `books.progress/current_cfi`，切换时至少保证同页和粗粒度进度；
- 自动重新 OCR 被禁止，已产生高亮的资产字节保持不变。

这不是“CFI 一定兼容”的证明。当前 Reader 会持久化 Foliate/PDF.js 产生的 `current_cfi`，而 OCR 可能改变文字 span 结构；尤其 `--mode skip` 的 mixed PDF 可能已有文字页、书签或高亮。v1 必须把“源 PDF → 同页 OCR PDF 的 current_cfi/高亮恢复”当作开放验收假设：至少测试跨设备、双页、旋转、缩放和已有标注；失败时按 PDF page index/progress 回退，并保留旧 asset 的标注，不宣称精确迁移。

一旦加入 reflow EPUB、自动在多个 OCR 资产间切换、或要求跨版本精确迁移高亮，就必须单独设计 `book_asset_positions` 和 asset-bound anchors；不能继续复用一个 CFI。

### 8.6 删除整书与防假绿测试

新增表后，整书删除的显式 cascade 必须覆盖：

- `book_assets`；
- `book_asset_local_state`；
- `ocr_jobs`；
- Phase G 新增的资产 tombstone/偏好相关行。

回归测试必须先在每张表 seed 数据再删除书并断言为空。只检查空表会产生“测试通过但忘记 cascade”的假绿。

## 9. OCR 后端候选与 PoC 证据

### 9.1 OCRmyPDF + Tesseract 候选

固定候选参数：

```text
--mode skip
--output-type pdf
--rasterizer pypdfium
--optimize 0
--fast-web-view 999999
--jobs <Lantern 自动计算>
-l chi_sim+eng
```

语义：

- `--mode skip`：已有文字页尽量保持原样，只处理无文字层页，适合混合 PDF；
- `--output-type pdf`：生成普通 PDF，避免 PDF/A 和 Ghostscript 路径；
- `--rasterizer pypdfium`：明确 PDFium，不允许静默依赖 Ghostscript；
- `--optimize 0`：首期不做体积优化，减少额外变换；
- `--fast-web-view 999999`：避免额外线性化；
- 不用 `--force-ocr`：它会栅格化原生文字和结构；
- 不默认 deskew/clean/remove-background：可能改变可见页面或照片。

Phase C 已在清理后的 PATH 中确认以下工具均不存在时成功运行：

```text
gs
veraPDF
unpaper
pngquant
jbig2enc
```

环境与版本：

```text
macOS 26.5.2 arm64
Python 3.14.6
OCRmyPDF 17.8.1
Tesseract 5.5.2
pypdfium2 5.12.1
```

这证明普通 searchable PDF 路径可无 Ghostscript 运行，但仅是单页控制性 smoke，不等于目标平台自包含包已经完成。

### 9.2 Apple Vision + 现有 PDFium 候选

识别配置：

```text
VNRecognizeTextRequest
recognitionLevel = accurate
recognitionLanguages = ["zh-Hans", "en-US"]
usesLanguageCorrection = true
```

PoC 已完成：

- Vision Rev2 / Rev3 均识别同一中英控制页；
- 得到行级 observation 和中文字串级四边形坐标；
- Rust `pdfium-render 0.9.1` 使用 Lantern 已有 PDFium 写入 30 行不可见文字；
- 使用子集 TrueType CID 字体和 ToUnicode；
- 当前 PDF.js 提取到 30 个文本 span；
- 程序化 DOM Range 可跨三行返回连续中文；
- 输入/输出渲染像素 hash 完全一致，变化通道数为 0；
- 输出约 368 KiB，比输入增加约 48 KiB。

这证明“Vision 坐标 → PDFium 标准文字层 → PDF.js Selection”闭环在单页上可行，不需要把 Tesseract.js 覆盖层常驻 Reader。

Vision 在 Codex 受限 sandbox 中曾出现通用 Objective-C 错误，在允许的非 sandbox 进程中成功；Lantern 当前没有 App Sandbox entitlement，因此这不是已发布应用失败的证据，但仍必须用真实 signed `.app` 在 macOS 12 和当前 macOS 上复测。

### 9.3 单页控制测试数据

OCRmyPDF：

| 配置 | 耗时 | 峰值进程树 RSS | CER proxy | 说明 |
|---|---:|---:|---:|---|
| fast `chi_sim+eng` | 2.13 s | 282 MiB | 5.35% | 当前控制页 |
| best `chi_sim+eng` | 3.13 s | 335 MiB | 5.71% | 未优于 fast |
| best `eng+chi_sim` | 3.28 s | 328 MiB | 10.34% | 语言顺序明显影响此页 |

Apple Vision：

| Revision | OCR 耗时 | CER proxy | 备注 |
|---|---:|---:|---|
| Rev2 | 0.503 s | 2.02% | macOS 11+ accurate 可支持中文 |
| Rev3 | 0.437 s | 2.02% | macOS 13+ 改进版本 |

这些数据仅来自一个由数字 PDF 页渲染得到的 180 DPI 控制样本。它不是噪声扫描、低清、倾斜、双栏、脚注或合法 10–20 本语料，不能据此承诺真实书籍准确率和整书耗时。

更具体地说，当前仓库唯一样本是约 253 页、15,097,846 字节的数字/混合 PDF：约 108,730 个可提取字符、11 页无文字层、24 页少于 20 个字符、22 个图像页。文件来源不适合作为可再分发测试语料，因此 Phase C 必须补齐合法样本矩阵。

### 9.4 当前后端排序

1. **macOS：Vision + 现有 PDFium 是首选候选。** 它可能不需要 Python/Tesseract runtime 下载，速度和当前控制页准确率也更好。
2. **Windows：OCRmyPDF/Tesseract 是保守候选。** Windows.Media.Ocr 尚未实测，不能提前假设其中文质量足够。
3. **macOS 回退：OCRmyPDF/Tesseract。** 若 signed app 中 Vision、复杂 PDF 写回或字体许可无法通过，则保留统一后端方案。
4. **中间候选：native Tesseract + 现有 PDFium graft。** 如果完整 Python/OCRmyPDF 包过大、而 Vision 只覆盖 macOS，可以比较原生 Tesseract、PDFium 页面复制和文字层写回；代价是自行承担混合页、ToUnicode、模型打包和跨平台维护，不能把它当成免费缩包方案。
5. **实时覆盖层：不作为回退首选。** 标准 PDF 输出路线已经得到可行证据。

最终选择必须等 Phase C 开放门槛通过后记录，当前不可写成“macOS 已确定使用 Vision”。

### 9.5 Phase C 仍未通过的门槛

- 合法可分发的 10–20 本真实样本矩阵；
- 噪声、倾斜、低清、双栏、脚注、竖排和混合 PDF；
- signed Lantern `.app` 内 Vision probe；
- 真实 macOS 12 运行验证；
- CropBox/MediaBox/旋转映射；
- 注释、表单、书签、元数据和数字签名处理；
- CJK 字体或 glyphless ToUnicode 的最终许可与兼容策略；
- 完整 Reader 鼠标单击、双击、拖选、跨行、跨栏、高亮和 AI 查询；
- Windows.Media.Ocr 快测；
- Windows 11 x64 自包含 OCRmyPDF/Tesseract 包；
- 最终下载/安装体积、SBOM 和第三方 notices。

## 10. 包体、模型、许可证与系统影响

### 10.1 主包与可选下载

“OCR 扩展约 100 MB”只能作为 UI 早期占位值，不能作为发布承诺。最终应按平台和架构分别显示 manifest 中的真实：

```text
download_size
installed_size
minimum_os_version
sha256
version
```

若 macOS 最终采用 Vision + 已捆绑 PDFium，macOS 可能不需要下载 OCR runtime，只需应用本身包含少量桥接代码和字体资源。Windows 的 Python/OCRmyPDF/Tesseract 自包含包仍需实际构建后测量。

平台能力矩阵当前应这样理解，而不是同时承诺所有平台都有同一套下载项：

| 平台/最终后端 | 是否下载 OCR runtime | fast/best 设置语义 | 是否可卸载 |
|---|---|---|---|
| macOS + Vision（若最终选中） | 可能不需要新增 OCR runtime/model 下载；仍有 PDFium/Rust/字体依赖 | Vision 使用系统模型，不天然对应 Tesseract fast/best；设置应显示“系统 OCR 可用”或另行定义质量档 | 不能卸载系统 Vision；只能管理本地派生资产 |
| macOS + OCRmyPDF/Tesseract 回退 | 需要按包 manifest 下载 | fast/best 独立模型 | 可卸载 runtime 和模型 |
| Windows + OCRmyPDF/Tesseract（当前保守候选） | 需要按包 manifest 下载 | fast/best 独立模型 | 可卸载 runtime 和模型 |
| Windows.Media.Ocr（未验证候选） | 可能使用系统能力 | 需另定质量映射 | 不能按 Tesseract 方式卸载 |

因此“fast/best 可下载、可卸载”是 Tesseract 后端的产品语义，不应硬套在 Vision 系统 OCR 上。Phase C 选型后，设置 UI 必须按平台能力渲染，或明确为了统一 UX 而仍携带 Tesseract，并解释为什么放弃 Vision 的包体优势。

### 10.2 Tesseract 模型大小事实

当前官方模型的近似大小：

| 模型 | 约 MiB |
|---|---:|
| fast `eng` | 3.92 |
| fast `chi_sim` | 2.35 |
| fast 中英合计 | 6.28 |
| best `eng` | 14.69 |
| best `chi_sim` | 12.47 |
| best 中英合计 | 27.16 |
| `osd` | 10.07 |

实测发现 `tessdata_best/chi_sim` 会加载 `chi_sim_vert`。高精度中文包还需约 12.4 MiB 的 `chi_sim_vert` 及其许可证，不能只把 best 中英两文件计入 manifest。

fast 和 best 文件名相同，应使用独立 tessdata 目录，通过 `TESSDATA_PREFIX` 明确选择；不能在同一目录相互覆盖。

### 10.3 性能与并发影响

OCR 是明显的 CPU 和内存任务，但不应提高“正常阅读”的长期资源消耗：

- 未安装或未运行 OCR 时，不启动 worker，不加载模型；
- OCR 子进程/任务使用后台优先级；
- 全局同一时刻只转换一本书；
- 书内页面可并行；
- 显式 `OMP_THREAD_LIMIT=1`，避免 worker 数与 Tesseract OpenMP 线程相乘；
- 默认并发上限 4，至少给系统和 Reader 留一个物理核心；
- `jobs` 在任务开始前按 CPU、可用内存和历史 OOM 计算；
- OCRmyPDF 不支持运行中无损动态降低 `jobs`，严重内存压力时只能取消并以更低并发重试。
- 无法可靠取得可用内存或系统压力状态时默认 `jobs=1`；低电量/省电模式可降至 1，睡眠/唤醒后不自动重复启动第二个任务。

初始启发式：

```text
cpu_slots = max(1, physical_cores - 1)
memory_slots = floor(available_memory / measured_per_worker_budget)
jobs = clamp(min(cpu_slots, memory_slots), 1, 4)
```

`measured_per_worker_budget` 必须从 300 DPI、600 DPI、彩色大页、fast/best 和目标设备矩阵得到，不能把单页 282–335 MiB 直接当成最终常数。

OCR 结束后不应持续占用 CPU，但 searchable PDF 的隐藏文字层会增加 PDF.js 解析、DOM span 和加载内存，尤其是 1000 页级别的大书。Phase H 必须比较原 PDF 与 searchable PDF 的打开、首屏、翻页、搜索、选区和 Reader 内存，而不能只测 OCR 子进程。

同步 searchable PDF 还可能接近“再存一份整书”：除了 runtime/model，用户会承担派生文件本身、OCR staging 临时空间、iCloud 上传带宽和共享存储。设置应分别显示 runtime、模型、每本派生资产和待清理 orphan 的占用。

### 10.4 系统版本影响

- 当前 Lantern PDFium 已经要求 macOS 12；README 修正不是 OCR 引入的新门槛。
- Vision Rev2 可以在 macOS 12 基线运行中文 accurate，但仍需真实 Monterey 和 signed app 验证。
- 若 macOS 改用 OCRmyPDF 官方 pypdfium2 预编译 PDFium，其当前平台包可能提高最低系统版本；必须避免在 PoC 前承诺。
- Windows 目标仍为 Windows 11 x64。

### 10.5 开源与许可证

Lantern 保持 MIT 并全量开源，但一个可选扩展中的组件不会因此自动变成 MIT。当前候选依赖至少包括：

| 组件 | 许可证/义务方向 |
|---|---|
| Lantern | MIT |
| OCRmyPDF | MPL-2.0 |
| Tesseract / tessdata | Apache-2.0 |
| Leptonica | BSD 风格 |
| pypdfium2 wrapper | Apache-2.0 OR BSD-3-Clause |
| PDFium | BSD 风格及大量第三方 notices |
| pikepdf | MPL-2.0 |
| qpdf | Artistic License 2.0 |
| fpdf2 / img2pdf | LGPL-3.0 |
| Ghostscript 社区版 | AGPL-3.0；首期明确不分发、不调用 |

发布要求：

- 按实际构建产物生成 SBOM；
- 包内保留许可证和第三方 notices；
- MPL/LGPL 组件提供相应源码和可替换/重组能力的合规评估；
- 不采用难以拆解、无法替换内部 LGPL 模块的 one-file 冻结包，除非完成专门审核；
- manifest 记录组件版本和哈希；
- 许可证结论以最终实际包为准，本文不是法律意见。

若 macOS 最终使用 Apple Vision，Lantern 自有代码仍可 MIT，但 OCR 能力依赖 Apple 专有系统框架；如果产品对“开源方案”的要求是 OCR 引擎本身也必须可审计/可替换，则仍需保留 Tesseract 后端作为非 Apple 路线，而不能把 Vision 作为唯一实现。

## 11. 为什么不采用 Tesseract.js 实时覆盖层

Tesseract.js 只提供图片 OCR/WASM 能力，不直接把 PDF 变成稳定的标准文字层。若选择覆盖层，Lantern 需要自行长期维护：

1. PDF 页到 OCR 图像的 DPI、CropBox、MediaBox 和旋转；
2. OCR 像素坐标 → PDF 坐标 → PDF.js viewport → CSS 坐标；
3. 缩放、旋转、适合宽度、双页、分页和滚动模式持续对齐；
4. block/line/word 到可被浏览器原生选择的透明 DOM；
5. 中文字符级选区、标点、断句、跨行和跨栏阅读顺序；
6. 页面虚拟化时文字层的挂载、销毁和缓存；
7. 多页滚动中的跨页 Selection；
8. 选择结果到 Lantern CFI/高亮锚点的映射；
9. 重新 OCR 或 PDF.js 升级后的旧高亮恢复；
10. WebView 主线程、worker 和 WASM 模型内存；
11. 同步到手机后，在移动 Reader 重做完全相同的覆盖层；
12. 最终若仍要导出 searchable PDF，又等于自行重写一套文字层 grafting 管线。

粗略工程量假设一名熟悉 PDF.js、DOM Selection 和 Lantern Reader 的工程师：

| 目标 | 粗略量级 |
|---|---|
| 单页点击 OCR word demo | 1–2 周 |
| 可见页选词、选句与基础缓存 | 4–8 周 |
| 覆盖现有滚动/分页/缩放/旋转和基础高亮 | 2–3 个月 |
| 达到复杂版面、跨平台、跨设备发布稳定性 | 3–6 个月持续打磨 |

它不必然让整个 Lantern 不稳定，但会把最敏感的交互代码变成自研核心路径，回归面显著大于“离线生成标准 PDF、完成后继续用 PDF.js”。因此只保留为非常规实验，不作为第一路线。

## 12. OCR 任务状态机与资源管理

### 12.1 本机状态

```text
queued
waiting_source
preparing
recognizing
validating
publishing
ready
failed
cancelled
```

说明：

- `waiting_source`：源 PDF 是 iCloud placeholder 或暂不可达；
- `preparing`：探测 PDF、创建 staging、计算配置；
- `recognizing`：OCR 后端运行；
- `validating`：PDFium 解析、页数/几何/文字层/抽样渲染；
- `publishing`：原子移动、写资产状态；Phase G 后同时走安全 outbox；
- `ready`：结果已验证并可由 resolver 激活；
- `failed`：稳定错误码 + 可复制详情；
- `cancelled`：不保留半成品，不删除源文件。

### 12.2 队列与去重

- 全局只运行一本书的 OCR；其他任务排队；
- 同一本书只允许一个活跃任务，由部分唯一索引保证；
- 逻辑去重至少使用 `book_id + source_sha256 + conversion_version + languages + quality`；Vision/Tesseract backend、pipeline/OS revision、PDFium build 和字体/ToUnicode recipe 是否也进入 key 尚未定稿。若不进入 key，就必须明确把不同 backend 当成同一逻辑 profile 并定义质量/LWW 规则，不能假设相同输入一定产生相同字节；
- 如果已经存在同键、已验证的资产，不自动重复生成；
- 用户显式“重新识别”时创建新 asset，不覆盖旧 asset；
- 两台设备离线同时生成可以得到两个 asset，通过唯一 ID 共存，不能按文件名互相覆盖。

### 12.3 结构化进度

OCRmyPDF 人类可读 Rich 进度条不是稳定协议。扩展应使用薄 wrapper/plugin 输出 JSON Lines：

```json
{"type":"phase","phase":"analyzing"}
{"type":"progress","phase":"ocr","completed":36,"total":284}
{"type":"phase","phase":"finalizing"}
{"type":"warning","code":"PAGE_TIMEOUT","page":117}
{"type":"complete","pages":284,"ocr_pages":278,"skipped_pages":6}
```

如果后端无法给出可信页级进度，UI 应退化为不确定进度条，不能伪造百分比。`pages_done == pages_total` 后仍有合并、验证、哈希和发布，Reader 应显示“正在完成”，不能提前 Toast。

### 12.4 取消与崩溃恢复

- macOS/Linux：独立 process group，取消时终止整个 group；
- Windows：Job Object + kill-on-close；
- 先尝试优雅退出，超时后杀整个进程树；
- 不能只杀 Python 主进程而遗留 Tesseract worker；
- 启动时把异常终止的活跃 job 标为可恢复/失败并清理 `.partial`；
- 首期不承诺逐页断点续转，重试从头开始；
- 每次状态更新带 source hash/任务 ID guard，陈旧任务不能发布到已经变化的书。

## 13. 输出验证与 crash-safe 发布

### 13.1 OCR 阶段

1. 解析并物化 source asset；
2. 对数字签名 PDF 首期拒绝自动 OCR，明确说明会破坏签名；
3. 在本地 staging 目录创建 `.partial.pdf`；
4. 长时间 OCR 不持有 sync transition lock；
5. Reader 继续打开原 PDF；
6. 后端退出后进入统一验证。

### 13.2 验证条件

至少检查：

- 文件非空且 Lantern 当前 PDFium 能打开；
- 页数与源完全一致；
- MediaBox/CropBox/旋转在允许范围内一致；
- 预期扫描页出现可提取文字；
- 已有文字页未被无故重栅格化；
- 首页、中间页、末页可渲染；
- 抽样像素或感知差异在阈值内；
- PDF.js 可提取 Unicode 文本；
- 计算 `byte_size` 和 SHA-256；
- 记录 recognized/skipped/timed_out/failed 页数。

任何关键验证失败：

- 不创建 ready asset；
- 不切换 Reader；
- 不写同步 asset publish；
- 保持原 PDF 可读；
- 保存稳定错误码和诊断详情。

### 13.3 发布顺序

Phase G 完成后的安全顺序：

1. 完成 staging 输出和验证；
2. 获取短时 mutation/transition guard；
3. 重新读取当前 `data_dir`，因为 OCR 期间用户可能切换同步；
4. 复制/移动到当前 `data_dir/books/` 的临时 sidecar；
5. 原子 rename 为包含 `asset_id` 的最终文件；
6. 在 `SyncWriter.with_tx()` 同一 SQLite 事务中插入资产、设置偏好、写 outbox；
7. 释放 guard；
8. resolver 校验本机最终文件并标为 `available_verified`；
9. Reader 保存页码/缩放/模式并切换；
10. 新文字层实际渲染后显示完成 Toast。

禁止先发事件后生成文件。

### 13.4 允许的崩溃结果

| 崩溃点 | 安全结果 |
|---|---|
| OCR 临时文件生成中 | 源 PDF 不受影响；重启清理或重试 |
| 最终 rename 前 | 只有 staging 半成品，不可激活 |
| 最终文件后、DB 事务前 | 产生孤儿文件；书仍读 source |
| DB/outbox 提交后、JSONL flush 前 | 本机资产存在；outbox 后续重试 |
| 事件已同步、PDF 尚未同步 | 对端保持 source，资产标 `remote_only`/`downloading` |
| PDF 下载不完整或哈希错误 | 标 `corrupt`，绝不激活 |

孤儿文件首期可以只做保守清理：仅删除未被任何资产引用且超过安全宽限期的文件。

## 14. 扩展和模型管理

### 14.1 包拆分

若某平台需要 Tesseract runtime，建议拆成：

```text
ocr-runtime
standard-model-pack
high-accuracy-model-pack
```

- 下载 runtime 时默认同时安装 standard pack；
- high-accuracy pack 独立下载、更新和卸载；
- 每个平台/架构分别打包，不用 universal 包重复携带原生二进制；
- 运行时、模型和已生成书籍资产分别统计空间。

### 14.2 package manifest

```json
{
  "package_id": "ocr-runtime",
  "version": "...",
  "platform": "windows",
  "architecture": "x86_64",
  "minimum_os_version": "Windows 11",
  "download_size": 0,
  "installed_size": 0,
  "sha256": "...",
  "signature": "...",
  "dependencies": [],
  "license_manifest": "THIRD_PARTY_NOTICES.json"
}
```

### 14.3 原子安装

1. 下载到 `.partial`；
2. 校验长度和 SHA-256；
3. 校验独立签名；
4. 解压到临时版本目录；
5. 执行 self-test，确认后端、模型和 PDFium/依赖可加载；
6. macOS 验证 Developer ID/公证，Windows 验证 Authenticode；
7. 原子切换 `current` 版本指针；
8. 只有全部通过才显示“已安装”。

当前 Lantern 发布包仍是 ad-hoc 签名，正式扩展的签名/公证依赖既有 Gatekeeper 分发项目先完成。这是 Phase E 的前置条件，不应在 UI 先假装“签名已验证”。

发布实现还需定义：manifest 签名信任根、密钥轮换/撤销、最低允许版本与反回滚、旧版本保留、下载断点/代理/离线安装、正在运行任务的版本锁定，以及 macOS hardened runtime/Library Validation、Windows Defender 和 Authenticode 的实机行为。HTTPS + SHA-256 只能解决传输和完整性，不能替代签名信任和版本策略。

### 14.4 卸载语义

- 转换进行中先取消/停止任务，再卸载 runtime；
- 卸载 runtime 不删除已生成的 searchable PDF；
- 卸载 high-accuracy 只影响未来任务；
- “释放本机 OCR runtime 空间”与“删除书籍派生资产”是不同操作；
- “仅移除本机派生副本”与“从所有设备删除资产”也是不同操作；
- 删除核心 runtime 时 standard pack 默认一并删除；best 是否保留需 UI 明示，不能形成隐形占用。

## 15. Reader 与设置交互规格

### 15.1 设置页

在“阅读辅助”中增加“扫描件 OCR”子视图，沿用现有设置行样式和 `ROW_CONTROL_WIDTH`。建议三组：

1. **OCR 组件**：未安装、下载中、校验中、安装中、已安装、可更新、卸载中、失败；显示版本、下载进度、安装占用、错误摘要与可复制详情。
2. **识别质量**：快速/标准为默认；高精度可独立下载/卸载；选择未安装的高精度时先确认下载。
3. **存储与派生资产**：runtime 占用、模型占用、已识别书籍占用；提供管理入口并区分本机释放和全设备删除。

设置中不显示：

- jobs/并发数；
- PSM；
- DPI；
- Tesseract 内部参数；
- PDFium rasterizer 选择。

### 15.2 Reader 触发规则

只在用户表达“我要使用文字”的动作中触发：

- 单击/双击后 Selection 为空；
- 拖选结束后 Selection 为空；
- 右键无文字层页面；
- 查词、翻译、解释等文字型快捷键没有文本。

以下动作不触发下载或 OCR 提示：

- 翻页；
- 缩放；
- 滚动；
- 打开目录；
- 仅阅读扫描页。

检测按 `book_id + page + source_hash` 缓存和节流，混合 PDF 可逐页判断。

### 15.3 状态与交互矩阵

| 状态 | 用户在扫描页尝试选词/选句时 | 主操作 |
|---|---|---|
| 组件未安装 | “扫描件需要 OCR 组件才能选择文字” | 前往下载 |
| 组件下载/安装中 | 显示字节进度和当前阶段 | 查看设置/取消下载（如支持） |
| 组件已安装、书未识别 | “为此书识别文字” | 开始识别 |
| high-accuracy 未安装但被选择 | 说明需额外下载 | 下载并使用 / 改用快速 |
| 排队中 | “等待识别” | 取消 |
| 识别中 | “正在识别文字 36/284”或不确定进度 | 取消 |
| 验证/发布中 | “正在完成” | 无重复任务 |
| 已识别版本在 iCloud 下载中 | “正在下载已识别版本” | 继续读原件 |
| 失败 | 本地化错误摘要 | 重试 / 查看详情 |
| 完成 | 正常产生 Selection | 无 OCR 提示 |

当前提示容器使用 `pointer-events-none`。升级为按钮后必须改成真正可交互组件，并与现有底部快捷键 HUD 使用统一容器/优先级，保证一次动作只出现一条提示。

### 15.4 打开设置

当前 `open-settings` 事件只接受 section 字符串。向后兼容扩展为：

```ts
type OpenSettingsDestination =
  | SettingsSection
  | { section: SettingsSection; view: "ocr" };
```

Reader 的“前往下载”应通过后端统一显示并聚焦主窗口，再发结构化 destination；旧字符串 payload 继续有效。

### 15.5 完成切换与 Toast

1. 记录当前页码、缩放、滚动/分页模式和双页布局；
2. 仅在本机文件 `available_verified` 时重载；
3. 页数一致才恢复同页；
4. 等待新 PDF.js 文字层实际渲染；
5. 顶部显示 2.5–5 秒轻量提示：

```text
文字识别完成，现在可以选词和使用 AI 功能
```

Toast 使用 `role=status`，不抢焦点。切换失败则保持原 PDF 并提供重试，不能白屏。

### 15.6 前端事件与 i18n

建议事件：

```text
ocr-package-changed
ocr-job-changed       # 约 4 Hz 节流
book-assets-changed
```

窗口 mount 时主动查询完整状态，事件只作增量唤醒。所有用户文案同时更新 `src/i18n/en.json` 和 `src/i18n/zh.json`；Rust 只返回稳定错误码，前端负责本地化。

## 16. 同步协议设计

### 16.1 schema v7 新事件

```text
book.asset.publish
book.asset.delete
book.preferred_asset.set
```

v1 不增加 `book.asset_position.set`。

`book.asset.publish` 携带完整不可变元数据，并可在同一事务中表达 `make_preferred=true`。validation、merge、snapshot 和 tombstone 必须同批扩展，确保：

```text
本地命令提交后的数据库状态
== 日志 replay 后状态
== snapshot 恢复后状态
```

### 16.2 Phase D 与 Phase G 的硬边界

Phase D 可以实现：

- migration 028；
- asset repository/resolver；
- `book_asset_local_state`；
- source bridge/backfill；
- 本机 `ocr_jobs`；
- fake backend、验证器和 feature flag；
- staging 输出。

Phase D **不得**：

- 修改 `EVENT_SCHEMA_VERSION`；
- 向 v6 outbox 写 v7 asset body；
- 发布 `book.asset.publish`；
- 更新 v7 snapshot；
- 让远端 preferred 指针生效。

Phase G 必须一次性落地：

- event schema v7；
- asset events；
- validation；
- merge/tombstone；
- snapshot v7；
- `SyncWriter.with_tx()` 发布；
- peer capability gating；
- mixed v6/v7 和乱序 blob 测试。

这样避免产生“数据库已经有资产，但事件协议只做了一半”的不可回放状态。

### 16.2.1 gating 期间本机结果的存放仍需定稿

当本机已完成 OCR、但仍有活跃 v6 peer 阻止发送 v7 时，产品仍希望本机可以使用结果；同时不能把没有元数据事件的派生 PDF 直接放进 iCloud `books/` 形成长期 orphan。需要在实现前明确一种策略：

- 结果和 local-only asset metadata 留在本机 staging/本地数据目录，重启可恢复；待 gating 打开后再原子晋升到同步 `books/` 并写 v7 outbox；或
- 在 gate 阻塞时允许本机数据库保存明确标记为 local-only 的资产，但物理文件仍不能进入共享目录，且后续晋升必须有幂等 publisher。

无论选哪一种，都不能让 `preferred_asset_id` 或旧客户端看到一个无法解释的资产路径。该问题是当前设计的开放实现点，审核者应检查空间占用、重启、同步目录切换和用户卸载行为。

### 16.3 peer capability gating

现有 v6 客户端收到 v7 事件会拒绝发送者整份日志；这会让该 peer 后续进度、高亮等事件也停止同步。因此在首次发送 v7 资产事件前必须检查所有活跃 peer。

规则：

- peer manifest 通告 `max_event_schema`；缺字段按 6；
- 所有活跃、未忽略 peer 都必须 `>= 7` 才允许首次发 v7；
- 超过 30 天未见的 peer 可按 stale 规则不计入，阈值应可调整；
- 提供非破坏性的“手动忽略此旧设备”入口；
- 手动忽略只影响本机 gating，不删除对端 manifest/log/snapshot；
- 一旦发送过 v7，记录持久化单向 ratchet，不能再假装回到 v6；
- UI 列出阻塞升级的设备名和最后在线时间。

gating 不只约束 JSONL event，也约束含 `book_assets` 的 v7 snapshot：在所有活跃 peer 达到 v7 前，不能写会被 v6 设备拒绝的 v7 snapshot；要么继续使用 v6-compatible snapshot，要么把 snapshot 发布与同一 gating/ratchet 一起处理。

`max_event_schema` 当前假定为“设备可解析和写出的最高完整 schema 版本”。如果未来协议不是严格单调兼容，单一数字不足以表达“能读哪些事件/能写哪些事件”，需要升级为 feature bits 或读写能力字段；这属于 v7 设计评审项。

能力发现必须 **fail closed**。不能简单写：

```rust
list_peers(...).iter().all(|peer| peer.max_event_schema >= 7)
```

因为当前 `list_peers()` 可能跳过：

- 损坏 manifest；
- 暂时不可读 manifest；
- `.icloud` placeholder；
- 暂时不可见但历史已知的设备。

未知能力默认阻塞，除非 peer 已满足 stale 规则或被用户显式忽略。必须测试 malformed、placeholder、stale、ignored、mixed v6/v7 和 ratchet 重启持久化。

### 16.4 事件与 blob 乱序

iCloud 不保证 JSONL 和 PDF 同时到达。

接收端规则：

| 状态 | 行为 |
|---|---|
| 元数据先到，文件缺失 | `remote_only`，继续读 source，后续重试 |
| 发现 `.icloud` placeholder | 触发下载，标 `downloading` |
| 文件先到，事件未到 | 作为暂时孤儿，不自动激活 |
| 文件大小和 SHA-256 通过 | `available_verified`，resolver 可选择 |
| 哈希错误/PDF 无法解析 | `corrupt`，保持 source，允许重试 |

“文件存在”不是可用性证明；preferred 指针也不能绕过本机校验。

### 16.5 同步和手机行为

- source PDF 与 searchable PDF 都存入同步根并保留；
- 生成设备只同步验证完成的最终字节；
- 其他设备无需 OCR runtime；
- 手机先收到元数据时可显示“已识别版本正在下载”；当前 Lantern 仓库尚未交付手机 Reader，以下是未来消费者应满足的协议行为，不是本次桌面发布已完成的功能；
- 下载/校验完成后可默认使用 searchable PDF；
- 下载失败或文件被驱逐时继续打开 source；
- 手机从不自行执行 OCR；
- 卸载生成设备的 OCR runtime 不影响任何已同步资产。

本机生成的 OCR 资产在验证后可以自动切换并显示 Toast；另一设备收到 searchable PDF 时，如果用户正在阅读 source，不能静默重载、跳页或改变缩放。建议先显示“已识别版本可用”，由用户确认切换，或明确产品选择只在下次打开时切换。

### 16.6 删除语义

必须区分：

1. **卸载 OCR 组件**：删除 runtime/model，不删除书籍资产。
2. **释放本机副本**：允许 iCloud 驱逐或移除本机下载，不删同步元数据和共享文件。
3. **从所有设备删除派生资产**：事务中写 `book.asset.delete` 和 tombstone；若是 preferred 同事务回退 source；提交后 best-effort 删除共享 PDF。

不能先删共享文件再发删除事件，否则所有设备会暂时持有指向缺失文件的元数据。

### 16.7 多设备冲突

- 两台设备对同一 source 同时做 fast/best：两个 asset 可共存；
- 同一配置并发生成：唯一 `asset_id` 和文件名防覆盖；
- preferred 指针按现有 `(updated_at, updated_by_device)` LWW 规则收敛；
- delete tombstone 阻止延迟 publish 复活已删除资产；
- 删除整本书后的延迟 asset publish 不能复活书籍；
- 收到同逻辑去重键 ready asset 时，其他设备不自动重复 OCR。

## 17. AI grounding 衔接

OCR 完成后，仅让 Reader 切换不够。grounding 必须：

1. 调用与 Reader 相同的 active-asset resolver；
2. 使用 resolved asset 的 `content_sha256` 作为索引 key 的一部分；
3. searchable PDF ready 后使旧 `PDF_TEXT_LAYER_UNAVAILABLE` 状态可自愈；
4. 安排对活动资产重新建索引；
5. 若资产后来不可用，回退 source，并清楚区分“无文字层”和“文件暂未下载”。

否则可能出现 Reader 已能选词，但 AI grounding 仍固定读取 legacy `source_file_path`，再次判定无文字层。

## 18. 实施阶段、当前进度与发布顺序

### Phase A — 路径契约修复

**状态：完成，commit `75139ff`。**

- event/snapshot/replay 统一书籍路径验证；
- `books/` 与 `sources/` 白名单；
- 原生书、文本书、双设备 replay 回归。

应随最近 patch 发布，不等待 OCR。

### Phase B — peer schema 能力通告

**状态：完成，commit `1faf519`。**

- manifest `max_event_schema`；
- 缺失按 v6；
- Rust/前端 DTO；
- 无行为变化。

应尽早发布，缩短所有设备可观测升级的等待窗口。

### Phase C — backend / packaging / Reader PoC

**状态：进行中，尚未 final go。**

已完成：

- OCRmyPDF 无 Ghostscript 单页 smoke；
- fast/best/语言顺序控制测试；
- Vision Rev2/Rev3 控制页识别；
- Vision + PDFium 文字层写回；
- PDF.js 提取、跨行 Range 和像素不变验证；
- macOS 最低版本事实修正。

未完成：见 §9.5。

退出条件：合法样本报告、最终 macOS/Windows 后端、固定参数、包清单、真实体积、Reader 全链路与 go/no-go 记录。

### Phase D — 安全数据基础与本地管线

- migration 028；
- source asset 幂等 bridge；
- `book_assets` repository；
- `book_asset_local_state`；
- resolver；
- `ocr_jobs` 与 fake backend；
- 统一验证器；
- feature flag 默认关闭；
- cascade 防假绿测试。

Phase D 不改 event/snapshot/outbox/schema 版本。

### Phase E — 扩展管理与设置

- manifest 下载、校验、self-test、原子安装/更新/卸载；
- fast/best 独立管理；
- 设置子视图；
- 依赖 Gatekeeper/Authenticode 分发方案。

若 macOS 最终采用 Vision，该平台的 runtime 下载器范围会显著缩小；Windows 仍可能需要完整实现。

### Phase F — Reader 与 grounding

- 当前页检测与状态 controller；
- 可交互底部提示；
- 开始/取消/重试；
- 完成后的安全切换和 Toast；
- active resolver 接入 Reader 与 grounding；
- 中英文 i18n。

### Phase G — 同步资产与 schema v7

- asset events、snapshot、merge、tombstone；
- blob 发布/下载/校验；
- fail-closed peer gating；
- 单向 ratchet；
- mixed client 与 iCloud 乱序测试。

这是首版跨设备需求的发布门槛，不能推迟成“以后再同步”。

### Phase H — 硬化

- 自动并发常数定稿；
- 1000 页、低内存、彩色高 DPI 压测；
- 混合 PDF、旋转、复杂版面；
- 下载中断、磁盘满、睡眠/唤醒；
- 长期 orphan/asset 管理；
- 评估 OSD、更多语言和竖排模型。

### 18.1 阶段依赖

```text
Phase A ── 独立发布
Phase B ── 独立发布并提前收集 peer 能力
Phase C ── 决定真实 backend 和包边界
Phase D ── feature flag 内的数据基础
Phase E ── 依赖 Phase C；签名依赖 Gatekeeper 工作
Phase F ── 依赖 D 和可用 backend
Phase G ── 依赖 A/B/D；一次性升级协议
Phase H ── 发布前硬化
```

早期工程量估算：OCRmyPDF + 完整同步资产 MVP 约 6–10 周，硬化约 8–14 周；若 macOS Vision 路线成立，macOS 扩展管理工作可减少，整体可能下调 2–4 周。以上只是范围估算，不是承诺排期。

## 19. 验证与验收矩阵

### 19.1 合法样本矩阵

至少 10–20 本有权测试/分发的样本，覆盖：

- 简体中文、英文、中英混合；
- 纯扫描与混合 PDF；
- 150/180/300/600 DPI；
- 黑白、灰度、彩色；
- 轻微倾斜、90° 旋转；
- 单栏、双栏、脚注、表格、图片说明；
- 低清、噪声、透印；
- 竖排或明确标为不支持的样本；
- 加密、损坏、数字签名 PDF；
- 注释、表单、书签和不同 CropBox/MediaBox。

### 19.2 OCR 后端

- 无 Ghostscript、veraPDF、unpaper、pngquant、jbig2enc 环境；
- fast/best 与语言顺序；
- 页数和页面几何不变；
- 已有文字页保持；
- OCR 页有 Unicode 文本；
- 超时页、失败页和跳过页计数正确；
- 取消杀死完整进程树；
- 进程崩溃不影响 Lantern；
- signed app 与最低系统真机；
- Windows 安装包和升级/卸载。

### 19.3 Reader 与 AI

- 单击、双击、中文字符级选择；
- 跨行、跨栏拖选；
- 复制、查词、翻译、AI 解释；
- 高亮创建、重启恢复、另一设备恢复；
- 分页/滚动/缩放/旋转/双页；
- OCR 期间继续阅读；
- 完成后保持页码、缩放和布局；
- Toast 只在文字层可用后出现；
- grounding 自动从无文字层状态恢复并读取 active asset。

### 19.4 扩展管理

- 下载中断与断电；
- 哈希不符、签名不符；
- 解压失败、磁盘满；
- self-test 失败；
- 更新失败回滚旧版本；
- 转换中卸载；
- fast/best 独立安装和删除；
- 卸载 runtime 后已生成 PDF 仍可读；
- 空间统计区分 runtime/model/assets。

### 19.5 同步

- v6/v7 混合 peer 不发送 v7；
- 全部活跃 peer >= 7 后允许发送；
- missing/malformed/placeholder manifest 阻塞；
- stale peer 和手动忽略；
- ratchet 重启后仍保持；
- event 先到、blob 后到；
- blob 先到、event 后到；
- `.icloud` placeholder；
- size/hash 错误；
- 两设备同时生成；
- fast 与 best 共存；
- preferred LWW；
- 删除与延迟 publish；
- 删除整书后资产不能复活；
- OCR 期间开启/关闭同步；
- copy-back 失败时保持同步开启和旧 data_dir 不变。

### 19.6 崩溃注入

在以下位置强制终止并重启：

- OCR 进行中；
- `.partial.pdf` 写入中；
- 最终 rename 前；
- 最终文件后、DB 事务前；
- DB/outbox 提交后、JSONL flush 前；
- 对端收到事件但文件尚未下载。

每种情况必须满足：源 PDF 可打开、无半成品 ready、可安全重试、不覆盖已发布资产。

### 19.7 Definition of Done

首版发布前至少满足：

- Phase C 正式 go/no-go 为 go；
- macOS 12+ signed app 和 Windows 11 x64 目标后端通过；
- 完整 SBOM/notices；
- 原始 PDF 永不覆盖；
- searchable PDF 的视觉和页数验证通过；
- Reader 选择、高亮和 AI 全链路通过；
- 卸载 runtime 不影响已有资产；
- 双设备不安装 runtime 也能消费同步资产；
- v7 gating fail-closed 测试全绿；
- iCloud 事件/blob 乱序和 placeholder 不导致白屏；
- 崩溃与磁盘错误不产生不可恢复的 active 指针；
- 中英文 UI 和可访问性状态通知完成。

## 20. 主要风险、缓解措施与开放决定

| 优先级 | 风险 | 当前缓解 | 仍需审核/验证 |
|---|---|---|---|
| P0 | v7 事件让 v6 peer 停止处理整份日志 | manifest 能力 + fail-closed gating + ratchet | 能力发现是否覆盖所有历史 peer 和 iCloud placeholder |
| P0 | preferred 指向未下载/损坏文件 | local availability + SHA-256 + resolver 回退 | 重启和 watcher 竞态 |
| P0 | Phase D 提前写不完整协议事件 | 明确禁止 D 修改 outbox/event；G 同批落地 | 实现 review 必须逐 commit 检查 |
| P0 | OCR 覆盖或损坏原 PDF | source 只读、staging、不可变 asset、原子发布 | 数字签名 PDF 和异常路径 |
| P1 | Vision 在 signed app/macOS 12 失败 | Phase C 真机 gate；保留 OCRmyPDF 回退 | entitlement、线程和 sandbox 行为 |
| P1 | 复杂 PDF 写回破坏 CropBox/旋转/注释/表单 | 统一验证器和样本矩阵 | PDFium graft 是否完整保留结构 |
| P1 | CJK 字体/ToUnicode 许可或兼容性 | 子集字体 PoC；计划 SBOM | 最终字体选择和复制兼容性 |
| P1 | Windows 包体和中文质量不可接受 | 保留多个 backend 候选 | Windows.Media.Ocr 与自包含 OCRmyPDF 对比 |
| P1 | 自动并发导致 OOM或阅读卡顿 | 单书队列、jobs 上限、OMP=1、历史降级 | 目标设备 per-worker 预算 |
| P1 | fast/best 重新识别使高亮漂移 | 不自动覆盖，创建新 asset，保留旧 asset | v1 UI 如何提示手动切换 |
| P2 | 派生 PDF 增加 iCloud 和磁盘占用 | 空间统计、仅本机释放、全设备删除分离 | 大书/多版本保留策略 |
| P2 | OCRmyPDF 依赖许可复杂 | 排除 Ghostscript、版本化目录、SBOM/notices | LGPL 分发形式审查 |
| P2 | 当前扫描启发式漏判混合 PDF | 以当前页文字层交互触发 | 页面检测缓存和节流 |

### 20.1 当前仍需做出的最终决定

1. macOS 是否正式采用 Vision + PDFium，还是双平台统一 OCRmyPDF；
2. Windows.Media.Ocr 是否值得保留为原生候选；
3. Windows runtime 的实际包格式、下载体积和最低系统；
4. production CJK 字体或 glyphless ToUnicode 策略；
5. 对部分页面超时/失败时，什么阈值允许“部分完成”；
6. stale peer 默认 30 天是否合适，以及忽略入口文案；
7. 用户主动重新 OCR 后，是否自动把新资产设为 preferred，还是先预览/确认；
8. 多个大体积旧资产的默认保留和清理策略。
9. `book_assets` 与 legacy `books.file_path/source_file_path` 的单向兼容规则：Reader 查询是否只返回 resolver 解析后的 active path，还是在某些场景更新 legacy 字段；必须避免旧 `BookMetadataSet`/snapshot 把活动资产覆盖回 source，也不能让旧客户端把 OCR 路径误当成唯一源。
10. `geometry_fingerprint`、source/derived 外键和 immutable/update 约束由 SQLite 还是 Rust publisher 保证；当前 SQL 只是草案，显式 cascade 与 orphan reconciliation 尚未实现。
11. gating 阻塞期间的 local-only 资产具体目录、重启恢复和晋升流程。
12. Vision 系统模型随 OS revision 变化导致同一 source/profile 产生不同字节时，`pipeline_version` 应记录的 OS build、Vision revision、PDFium build、字体策略和去重规则。
13. 输出存在 timed-out/failed 页时，是允许 `ready` 并明确提示未识别页，还是整体 `failed`；v1 是否只支持整书重跑。
14. macOS 的 Vision proprietary dependency 是否符合“始终开源”的产品定义；若不符合，统一 Tesseract 路线的包体和性能代价是否值得。
15. v1 是用户显式启动后整书生成一份不可变资产，还是允许按页增量生成；当前推荐整书一次性生成，因为增量 PDF 合并、部分资产同步和高亮稳定性会显著扩大范围。
16. migration 028 的 source asset 覆盖范围和 `format` 枚举：仅 PDF/EPUB，还是把 text/HTML/markdown/MOBI/AZW/AZW3 也纳入；Phase D 必须为既有格式和 Phase A 修复过的文本书补迁移/删除/replay 回归。

## 21. 给独立审核 AI 的具体问题

请逐条给出“可行 / 有条件可行 / 不可行”，并说明证据和建议修改：

1. searchable PDF 作为第一产物是否是 Lantern 当前 PDF.js/AI 架构下的最优折中？
2. `book_assets + book_asset_local_state + preferred_asset_id` 是否足以避免远端元数据和本机文件可用性混淆？
3. legacy source hash/size 暂可为空、derived 必须完整的迁移策略是否安全？
4. v1 不建 `book_asset_positions` 是否会破坏已有 PDF 高亮，还是可以合理推迟到 EPUB/多位置空间？
5. Phase D 完全不写资产 outbox、Phase G 同批升级 v7 的边界是否正确？
6. fail-closed gating、stale peer、手动忽略和单向 ratchet 是否足以保护 v6 客户端？
7. 发布顺序能否在 OCR 期间切换同步目录、iCloud 乱序和崩溃条件下维持不变量？
8. Vision observation → PDFium invisible text 的坐标、字体、阅读顺序和 PDF 结构风险是否被充分覆盖？
9. macOS/Windows 使用不同 OCR 引擎、但同步同一标准 PDF 资产，是否会带来不可接受的一致性问题？
10. 自动并发模型是否需要更保守的默认值或系统内存压力 API？
11. 扩展安装、更新、卸载和许可策略是否适合 MIT 开源项目？
12. Reader 的状态和触发规则是否会误触发、阻塞阅读或制造重复任务？
13. grounding 与 Reader 共用 resolver、按 asset hash 重建索引是否充分？
14. 验收矩阵还缺哪些会在真实扫描书中造成严重回归的样本？
15. 能否在不牺牲手机消费、同步和标准 Selection 的前提下进一步缩小首发范围？

希望审核输出至少包含：

- 致命阻塞项；
- 应在编码前修正的架构问题；
- 可在实现阶段处理的问题；
- 可以推迟的优化；
- 推荐的最终 backend 选择条件；
- 建议调整后的阶段依赖和验收门槛。

## 22. 代码与文档证据索引

### 22.1 已有文档

- `docs/impls/scanned-pdf-ocr-pipeline.md`：当前可执行实施计划与 Phase A–H 边界。
- `docs/reviews/scanned-pdf-ocr-phase-c-poc-2026-07-18.md`：Phase C 可复现实测证据与开放门槛。
- `docs/reviews/scanned-pdf-ocr-upgrade-proposal-2026-07-17.md`：早期完整背景、候选比较、风险与验收表。
- `docs/arch/overview.md`：Lantern 书库、Reader、SQLite 和 iCloud 同步架构。
- `docs/impls/macos-distribution-gatekeeper-fix.md`：macOS 签名、公证与当前 ad-hoc 分发限制。

### 22.2 关键代码位置

| 主题 | 位置 |
|---|---|
| 当前页文字交互和 Selection | `src/pages/reader/useReaderInteractions.ts` |
| Reader 无文字层提示与布局 | `src/pages/Reader.tsx` |
| AI PDF 扫描启发式 | `src-tauri/src/ai/grounding/extract.rs` |
| 现有本地 prepared 转换状态机 | `src-tauri/src/commands/books/convert_prepare.rs` |
| 活动书籍查询与文件可用性 | `src-tauri/src/commands/books/query.rs` |
| 书籍导入和源路径 | `src-tauri/src/commands/books/import.rs` |
| 同步事件 schema | `src-tauri/src/sync/events.rs` |
| 事件路径验证 | `src-tauri/src/sync/validation.rs` |
| replay 和未知事件行为 | `src-tauri/src/sync/replay.rs`、`src-tauri/src/sync/log.rs` |
| 原子 outbox | `src-tauri/src/sync/writer.rs` |
| peer manifest | `src-tauri/src/sync/peer.rs` |
| snapshot | `src-tauri/src/sync/snapshot/` |
| iCloud 存储切换和 copy-back | `src-tauri/src/commands/sync.rs` |
| vendored PDF Reader | `public/foliate-js/` |

### 22.3 已完成 commit

- `75139ff` — 修复书籍路径契约。
- `1faf519` — peer manifest 通告最大事件 schema。

### 22.4 临时 PoC 产物

本机临时证据位于：

```text
/private/tmp/lantern-ocr-poc/
/private/tmp/lantern-vision-graft-poc/
```

它们不是仓库交付物，不应提交到 Git。可复现步骤和结果摘要以 Phase C 报告为准。

## 23. 最终审查基线

在独立审核开始时，请把下列内容视为当前基线：

- 不重新讨论“是否覆盖原 PDF”：答案固定为不覆盖；
- 不把 searchable PDF 降级为仅本机缓存：它必须同步；
- 不要求手机执行 OCR；
- 不把 EPUB 纳入第一期；
- 不把 Tesseract.js 覆盖层当默认备选；
- 不假设 Vision 已最终胜出；
- 不把单页 PoC 数字外推为整书发布性能；
- 不把“Lantern 是 MIT”误读为“所有第三方组件都是 MIT”；
- 不允许 Phase D 偷渡 v7 事件；
- 不允许 capability 检查对未知 peer fail open；
- 不允许 preferred 指针绕过本机文件校验；
- 不允许在 Phase C 开放门槛通过前宣布功能已具备发布条件。

如果审核者建议改变这些基线，应明确指出触发变化的致命证据、替代设计如何满足跨设备和 AI 需求，以及迁移/兼容成本。
