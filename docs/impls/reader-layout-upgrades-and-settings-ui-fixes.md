# 阅读器排版、设置 UI 与凭据 P1 实施记录

> 状态：代码实现完成，自动化检查通过，人工验收待执行
>
> 基线：`9e018c2`（v1.4.1）
>
> 更新日期：2026-07-15

本文固化原分析文档中的推荐方案、后续确认项和实施边界。最终真机、视觉、跨格式及跨版本矩阵只编写测试方案，不由 Codex 执行；对应方案见 [阅读器排版、选区交互与凭据 P1 全量测试方案](../testing/reader-layout-and-credential-p1-test-plan.md)。

## 1. 已确认范围

| 范围 | 决策 | 实施状态 |
|---|---|---|
| 设置行控件 | 同类选择控件使用统一宽度常量 | 已实现 |
| Toggle 越界 | 删除平行实现，统一共享 `ui/Toggle`，并强制可访问名称 | 已实现 |
| 卡片预览 | 只在“卡片设计 / 操作菜单”打开；按可用高度计算卡片高度 | 已实现 |
| Portal 主题 | 下拉菜单复制触发器继承到的局部阅读主题变量 | 已实现 |
| 单词与选区 | 单击显示完整系统选区后开菜单；拖选松手后补全词边界 | 已实现并完成静态审查 |
| Force Click | 保留 macOS 系统词典，仅真实 `webkitmouseforcedown` 抑制 Quill 菜单 | 已实现 |
| 章节分页 | 分页模式下语义章节另起页，滚动模式保持连续 | 已实现并完成静态审查 |
| 翻页输入 | 补 PageUp、PageDown、Space、Shift+Space；保留自定义键 | 已实现 |
| 翻页动画 | 滑动、渐隐、覆盖、无动画四档，含降级和减少动态效果 | 已实现并完成静态审查 |
| 正文宽度 | 最大宽度以上层阅读容器为上限，不再使用固定像素上限 | 已实现 |
| 凭据 P1 | 日常 API Key/OAuth 只读本地 `secrets.db`；旧保险库显式迁移 | 已实现并加固 |
| 凭据 P2 | iCloud 中仅同步密文，主密钥经 iCloud Keychain 分发 | 延后，等待正式签名与 access group |
| M5 拆分 | 拆分 `books.rs`、`snapshot.rs`、`Reader.tsx` | 已完成；`Reader.tsx` 为 1,472 行 |

## 2. 阅读交互

### 2.1 触发规则

- 单击正文单词时，先把完整单词写入浏览器 `Selection`，让用户明确看到操作对象，再在约 240ms 后显示操作菜单。
- 删除“单击单词行为”设置；单击固定打开菜单，不直接请求 AI。
- 双击快速查词保留为可关闭选项。双击发生时取消待处理单击，避免一次手势触发两次。
- macOS Force Click 不被应用接管；Windows 只提供双击快速查词。
- 链接、按钮、输入控件、纯空白和纯标点不触发查词。

### 2.2 拖选补全

- 指针按下期间不打开菜单；仅在 `pointerup` 后处理最终选区。
- 只补全首尾词边界，中间已覆盖文本保持不变。
- 使用 `Intl.Segmenter` 作为首选分词器，并保留 Unicode fallback。
- 缩写、所有格和连字符词把 `'`、`’`、`-`、`‐`、`‑` 视作词内连接符。
- 支持被行内标签拆开的连续词元；规范化采用 NFC，组合音标不丢失。
- 拖出阅读容器或 Foliate iframe 后松手也必须结束指针状态。
- 键盘选择、右键、双击和 Force Click 不进入自动扩选路径。

### 2.3 视觉与菜单

- 原始、阅读纸、灰色和深色主题分别定义高对比度 `::selection`。
- 系统选区必须和手动标记、自动查词标记明显区分。
- 菜单按补全后的最终选区重新定位，优先放在选区侧面，其次上下避让，不覆盖选区且不越出视口。
- 菜单键盘导航跳过 disabled 项；Escape 关闭菜单。

## 3. 章节、宽度与翻页

### 3.1 文本书章节

TXT/Markdown/HTML 的标题解析器已经知道 `Volume / Book / Division / Chapter / Section` 语义等级。分页不能使用目录 `depth` 推断，因为同一个 Chapter 在嵌套目录中可能是 depth 3，而 Section 也可能是 depth 1。

实施模型：

- `TextBookBlock` 增加 `starts_page`。
- `Volume / Book / Division / Chapter`、顶层标题、`ACT / SCENE` 等阅读单元标记为 true。
- `Section` 等小节标记为 false。
- prepared document 版本从 v2 升为 v3；旧书首次打开时自动重新准备，稳定源偏移位置协议不变。
- 成功生成 v3 后清理旧 prepared cache 和 sidecar。
- 只有分页模式应用 `break-before: column`；文档首 block 不制造空白页。

### 3.2 EPUB 章节

- 优先根据 TOC 目标和章节容器标记真实章节，不把所有 h2/h3 都当章节。
- 支持 `body > h1`、`section > h1`、`section > header > h1` 等常见结构。
- spine item 的首章节标题不制造空白页。
- 滚动模式不注入强制章节断点。

### 3.3 页面布局

- 单页/双页继续共用 `page_columns`；双页仅在横向空间足够时生效，竖向窗口回落单页并在页面布局区说明。
- EPUB/PDF 的列宽和 TXT article 最大宽度都受当前阅读容器约束，而不是固定像素常量。
- 阅读界面页边距和全局阅读偏好共用同一设置键并通过设置事件双向同步。
- 底部进度保留章节百分比；可选追加整书进度和分页模式的当前页/总页数。

### 3.4 翻页与动画

- 默认键：方向键、PageUp/PageDown、Space/Shift+Space。
- 自定义键盘键和鼠标非主键继续通过录制保存；输入框、AI 面板和设置弹层不触发翻页。
- 动画档位：`slide`、`fade`、`cover`、`none`。
- `fade` 和 `cover` 优先使用同文档 View Transitions；调用必须保持正确的 `document` 绑定。
- 无 View Transitions 时，fade 使用受控 opacity 降级，cover 在 EPUB/PDF/TXT 统一降级为 slide。
- `prefers-reduced-motion: reduce` 同时关闭 View Transition、Foliate `animated` 和 TXT smooth scroll，等效于无动画。

## 4. 设置 UI

- 设置行使用 `ROW_CONTROL_WIDTH` 或 `ROW_CONTROL_WIDTH_COMPACT`，不在调用点继续增加任意选择框宽度。
- `ui/Toggle` 是唯一 switch 实现；`label` 为必填属性，ESLint 禁止设置模块另写 `role="switch"`。
- CardPreview 用 `ResizeObserver` 同时测量可用宽高和菜单高度；静态预览不调用 API，真实预览只由显式按钮触发。
- 工具预览在宽屏作为右侧栏，在较窄窗口作为子对话框覆盖显示。覆盖态让底层设置 `inert`，焦点限制在预览内，Escape 先关闭预览并恢复原焦点。
- Select 继续 portal 到 `body` 以避免裁剪，但复制当前主题 CSS 变量，保证应用主题与阅读主题不一致时仍显示正确配色。

## 5. 凭据存储 P1

### 5.1 日常路径

- API Key 和 OAuth token 存入本地专用 `secrets.db`。
- 日常保存、模型列表、连接测试、查词、学习卡片和 AI 对话不访问 Keychain。
- 前端只得到凭据掩码、状态和计数；秘密不进入日志、普通 settings、MCP、事件日志或快照。
- 数据库在 Unix 使用 `0600`，SQLite 使用 `secure_delete=ON` 与 `journal_mode=DELETE`。这是本地明文的明确产品权衡，不宣称抵御同用户进程、备份、文件系统快照或磁盘取证。

### 5.2 旧版本迁移

- 启动和普通 AI 操作只读取迁移元数据，不自动访问系统凭据。
- AI 设置页显示待迁移数量；用户点击后先出现应用内解释框，确认后才请求 Keychain。
- v1.4 加密行和更早的逐项 Keychain 候选在同一次显式操作中读取。
- 所有可恢复值在单个 SQLite 事务中写入；只删除成功恢复的旧行，本地新值优先。
- 缺失主密钥或单条损坏密文不阻断其他独立候选；坏行保持原样并返回 `VAULT_PARTIAL_MIGRATION`，迁移提示继续存在。
- 拒绝、取消或系统授权错误不形成后台重试循环。

### 5.3 跨数据库一致性

AI profile 元数据和 `secrets.db` 无法共享一个 SQLite 事务。新增、替换、删除和删除 profile 因此保留完整秘密快照并执行补偿；补偿失败必须返回同时包含主操作和补偿结果的组合错误，不得静默吞掉。

### 5.4 P2 延后条件

凭据密文同步必须等待：

1. 正式签名带来稳定应用身份；
2. 正确配置 iCloud Keychain access group；
3. 跨设备主密钥可见性通过真实设备验证。

在此前不创建同步主密钥，不把凭据写入 iCloud。P2 仍采用逐条 AES-256-GCM、版本化 AAD、LWW 合并、原子 bundle 写入，以及“发现远端更新后由用户显式导入”的设计。

## 6. 复杂度拆分

### 6.1 Rust

- `commands/books.rs` 拆为 `books/{mod,format,text_headings,text_prepare,import,pdf,query,mutate,tests}.rs`，公开命令路径不变。
- `sync/snapshot.rs` 拆为 `snapshot/{mod,rows,compact,apply,tests}.rs`，序列化与应用语义不变。

### 6.2 Reader

已建立的边界：

- `reader-theme.ts`
- `chapter-pagination.ts`
- `foliate-types.ts`
- `reading-progress-writer.ts`
- `useReaderSettingsSync.ts`
- `usePageTurnInput.ts`
- `useBookAvailability.ts`
- `useWindowSizePersistence.ts`
- `useReaderInteractions.ts`
- `useSidePanelResize.ts`
- `useFoliateAnnotations.ts`
- `useFoliateView.ts`
- `useReaderNavigation.ts`

Foliate 创建、打开、事件、布局和销毁已迁入 `useFoliateView.ts`，标记/注释与跨窗口导航也已独立。`Reader.tsx` 现在只协调书籍状态、各 hook 和页面布局，共 1,472 行，低于 1,500 行目标；Tauri 命令、CFI、进度和标记语义保持不变。

## 7. 验证边界

实施阶段已执行：

- `npm run lint`
- `npx tsc --noEmit`
- `npm run build`
- `npm run test:unit`
- `cargo fmt --check`
- `cargo clippy --lib --tests -- -D warnings`
- `cargo test --lib`
- `npm audit --audit-level=high`
- `cargo audit`
- `git diff --check`

结果：

- 前端单元测试 10/10 通过；TypeScript、ESLint、生产构建和 `npm audit --audit-level=high` 通过。
- 中英文 i18n 各 911 个叶子键，路径完全一致。
- Rust 库测试 389 项通过、0 失败、1 项需要用户所选 iCloud 文件夹的人工冒烟测试保持忽略；Fmt 与严格 Clippy 通过。
- `cargo audit` 通过，无阻断漏洞；保留仓库允许的 23 条上游维护状态、健全性或撤回版本告警。
- 独立静态审查未发现 P1/P2 级问题；`git diff --check` 通过。

最终人工矩阵保持“待执行”，不得把未执行写成通过。范围至少包括 Force Touch 真机、拖出 iframe、四主题选区、EPUB/PDF/TXT、单页/双页、窄窗口、无 View Transitions、减少动态效果、凭据跨版本迁移和可访问性。
