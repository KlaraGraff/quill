# 个性化阅读器当前改动静态审查报告

> 审查日期：2026-07-12  
> 审查快照：`feat/personalized-reader`，`HEAD 5595d37` 加 2026-07-12 23:27（Africa/Casablanca）时的未提交改动  
> 对照依据：`docs/roadmap/personalized-english-reader-master-plan.md`、`docs/impls/personalized-english-reader.md`、`docs/reviews/personalized-reader-static-code-review-2026-07-12.md`  
> 审查方式：只阅读源码、配置、迁移和 Git 差异  
> 明确未执行：应用启动、构建、测试、类型检查、Lint、真实 AI 请求、iCloud 实机同步、签名、公证和视觉验收

## 1. 总结论

当前改动已经完成主要产品骨架，但不能认定为“全部按照原文档执行完成”。暖纸阅读视觉、查词到 AI 侧栏、查询历史与词汇状态、正文标记、多 API Key、常见格式入口、当前版本/原项目双展示，以及原审查中大部分安全问题均已有实现。

仍有 1 项 P0、5 项 P1、7 项 P2 和 3 项 P3 问题或缺口。最需要先处理的是会阻止全新数据库初始化及旧数据库升级的 migration 嵌套事务，其次是同步水位、中文 AI 流处理，以及新安装的默认 OpenAI 地址。

本轮复核同时确认，下列旧结论已经被当前最新代码处理，不应再作为现存问题：

- 系统凭据后端已按 macOS/Windows/Linux 启用持久实现，并在迁移前检查持久性。
- CSP 已允许 Tauri asset 来源；用户选定的 iCloud 目录会动态加入 asset scope。
- 同步事件、snapshot、实体 ID 和 blob 路径已增加统一语法校验，删除书籍不再接受任意绝对路径或 `..` 路径。
- 同步离线启动会保持 queue-only，并在恢复时迁移本地 blob。
- 同步日志和 snapshot 已有文件、单行和事件数上限。
- 同步启停与普通 mutation 已加入 transition gate；导入失败也已有文件清理 guard。
- API Key 删除已经改为先删凭据、数据库失败时补偿恢复。
- 查词、解释、翻译和聊天请求已保存 request ID，并在关闭或切换时调用后端取消。
- 已有阅读窗口导航已加入 pending queue、navigation ID 和 ack。
- Reader annotation 已统一为顺序差量更新；临时导航效果结束后会恢复永久标注，侧栏高亮操作也会触发刷新。
- 查询历史分页响应已包含全量按书聚合，修改保留期后也会向当前窗口和 Reader 窗口广播刷新。

## 2. 问题清单

### P0-1：migration runner 的外层事务与 migration 009 自带事务冲突，会阻止全新数据库初始化

证据：`src-tauri/src/db.rs:228-245` 现在为每个 migration 创建 `unchecked_transaction()`，并在事务内执行整份 SQL；但 `src-tauri/migrations/009_normalize_timestamps.sql:35-37,266` 自己执行 `PRAGMA foreign_keys = OFF; BEGIN; ... COMMIT;`。SQLite 不允许在已经开始的 transaction 内再次执行 `BEGIN`，而且 `foreign_keys` pragma 在事务内也不会按脚本设计生效。相同 runner 还用于 `run_migrations_up_to`。

影响：全新数据库从 1 开始执行时会在 migration 009 报 `cannot start a transaction within a transaction`；版本低于 9 的现有数据库也无法升级。外层事务会回滚 009 本身，但应用会持续启动失败。这是静态可确定的发布阻断问题。

推荐措施：统一事务所有权。较稳妥的方式是移除 009 脚本内的 `BEGIN/COMMIT`，并把必须在事务外执行的 `PRAGMA foreign_keys = OFF` 交给 runner，在 migration 后按明确策略恢复；或把 009 标记为自管事务的特殊 migration，runner 不再外包 transaction。必须覆盖全新数据库、v8 升级、009 中途失败回滚和重复启动恢复。

### P1-1：失败事件可被同一设备的后续事件越过水位，宣称的“下次重试”实际不会发生

证据：`src-tauri/src/sync/replay.rs:515-555` 对单个事件应用失败后回滚并继续处理全局列表；后续更大 ID 的成功事件会在 `533-536` 推进该设备水位。下一轮又在 `485-493` 跳过所有不大于水位的事件。

影响：若一个事件因父实体尚未到达、类型不兼容或暂时数据库错误失败，而同设备后续事件成功，失败事件将永久被跳过。代码注释与日志声称会重试，与真实行为不符。

推荐措施：水位必须按 peer 保持连续。某个 peer 首次失败后，本轮不再应用该 peer 的后续事件；或明确把失败事件放入 quarantine/dead-letter，再以可审计策略推进水位。增加“坏事件后仍有好事件”的回归用例。

### P1-2：中文或 emoji 聊天内容可使标题生成任务 panic

证据：`src-tauri/src/commands/ai.rs:345-353` 用字节长度判断后直接取 `&text[..200]`。Rust 字符串索引必须落在 UTF-8 字符边界。

影响：第 200 个字节落在多字节字符中间时，后台任务 panic，标题生成失败。

推荐措施：按 `chars()` 或 grapheme cluster 截断；若必须限制字节数，先向下寻找合法 UTF-8 边界，并覆盖中英文混排、emoji 和 200 字节临界用例。

### P1-3：AI SSE 按网络 chunk 独立做 lossy UTF-8 解码，会损坏中文和 JSON

证据：`src-tauri/src/ai/openai_compat.rs:57-98`、`openai_responses.rs:73-119`、`anthropic.rs:74-120` 都把每个网络 chunk 单独传给 `String::from_utf8_lossy`，并只接受 `data: ` 前缀。

影响：UTF-8 字符跨 chunk 时会变成 `�`，JSON 可能解析失败；合法的 `data:`、CRLF、多行 data 和尾部残留也可能丢失，表现为缺字、流中断或错误 failover。

推荐措施：保留字节缓冲，按完整 SSE 行/事件边界切分后再严格 UTF-8 解码；使用符合 SSE 规范的解析器，并让 adapter 返回结构化事件与解析错误。

### P1-4：全新安装只添加 OpenAI Key 时，默认请求可能发往本机 Ollama

证据：`src-tauri/src/ai/router.rs:199-207` 创建默认 `provider=openai` profile 时，旧设置缺失则 `base_url=NULL`；`router.rs:416-423` 对非 Anthropic/OAuth 分支的空 URL 统一回退到 `http://localhost:11434`。设置页只有用户主动切换 Provider 时才填 `https://api.openai.com`。

影响：新用户直接添加 OpenAI Key 后通常得到本机连接失败，并可能误以为 Key 无效。

推荐措施：按 Provider 解析默认 URL：OpenAI 和 Anthropic 使用各自官方地址，Ollama 才回退本机，custom 必须显式填写；迁移创建 profile 时直接持久化最终默认值。

### P1-5：同步日志只有语法身份校验，未来 ULID/时间戳仍可污染水位或 LWW

证据：`src-tauri/src/sync/validation.rs:27-45` 已校验 UUID、文件名/payload device、schema 和 ULID 语法，但没有校验 ULID 时间与 `event.ts` 的合理范围，也没有设备签名。`src-tauri/src/sync/replay.rs:485-536` 以事件 ID 推进水位；snapshot 在 `src-tauri/src/sync/snapshot.rs:508-514` 也以合法 ULID 单调推进。

影响：同一共享目录中的损坏客户端、严重时钟异常或可写入者，仍可用匹配设备 UUID 的极未来 ULID/时间戳让合法事件被永久跳过，或让异常行长期赢得 LWW。

推荐措施：至少限制 ULID/event timestamp 与当前时间和彼此的偏差，并隔离未来事件；更强方案是为每台设备建立密钥并签名日志 envelope。若只把 iCloud 目录视为可信输入，也应记录该信任边界。

### P2-1：EventLog 重启后未以现有日志末尾 ID 初始化单调状态

`src-tauri/src/sync/log.rs:53-66` 每次创建全新 ULID Generator；若系统时钟回拨，新追加 ID 可能小于旧日志末尾，随后 `src-tauri/src/sync/replay.rs:439-447` 会拒绝整份日志。建议启动时读取/验证最后 ID，并保证后续 ID 严格递增；遇到时钟回拨应等待、逻辑递增或换段记录。

### P2-2：格式能力模型已经建立，但粒度不足且没有完整驱动阅读器行为

`src/components/reader-settings.ts:22-45` 已新增统一 `ReaderCapabilities`，导航、文本标记、词汇/书签侧栏和设置入口也开始使用它；原先“完全没有能力模型”的结论已不再成立。但模型只有 `supportsTextAnchors / supportsReflowSettings / supportsSpread` 三项，`supportsSpread` 在代码中没有消费者；`src/pages/Reader.tsx:985-1000` 仍对所有格式设置 flow、column 和 margin，只对 PDF 设置 spread；`1505-1512` 又把所有不可重排格式伪装成 `bookFormat=pdf` 传给设置面板。

此外，PDF 被标记为 `supportsTextAnchors=true`，同一个布尔值同时开放选词、自动查询/词汇标记、书签、词汇侧栏和 CFI 导航；这与总方针“PDF 保留查词和手动高亮，首期不实现全文自动词汇标记”的能力边界不一致。CBZ 的 `supportsSpread=true` 也无法真正控制 spread 设置。

建议把能力拆为 `hasTextLayer / supportsSelection / supportsAnnotations / supportsWordMarkers / supportsCfiNavigation / supportsFontStyles / supportsZoom / supportsSpread / supportsContinuousScroll`，由能力值直接控制设置与 renderer 属性，不再把非 EPUB/PDF 格式伪装成二者之一。

### P2-3：格式扩展实现仍低于原方案的数据与解析要求

`migrations/016_book_source_formats.sql` 只有 `source_format/render_format/conversion_version`，缺少来源文件路径与 SHA-256；文本导入不保留原文件。`src-tauri/src/commands/books.rs:214-274` 仍用手写规则处理 Markdown/HTML，未使用成熟 parser/sanitizer；生成 EPUB 的 identifier 每次随机，不能确定性重建；conversion version 也没有进入同步事件。

建议引入 importer registry、CommonMark/HTML parser 与 sanitizer，补齐来源哈希、原文件策略、确定性 identifier 和同步元数据；当前能力应表述为“基础导入/转换”，不应承诺完整格式兼容。

### P2-4：SRS 有四档评分，但仍是自制固定倍率算法

`src-tauri/src/commands/vocab.rs:111-149` 使用固定 10 分钟、`1.2/2.5/3.5` 倍率和 21 天 mastered 阈值，没有稳定性、难度、遗忘概率或算法版本。建议采用 FSRS 等成熟实现，并设计现有 interval/due 状态迁移。

### P2-5：关于页双仓库展示完成，但版本身份信息不完整

`src/components/settings/AboutSettings.tsx:47-143` 已正确区分当前版本与上游，链接和 MIT 署名也正确；但没有上游基线、构建 commit、build date、channel、维护者和“复制版本信息”。建议由 CI 注入不可变 build metadata。

### P2-6：所有 Key 冷却、失效或禁用时统一误报为“AI 未配置”

`src-tauri/src/ai/router.rs:274-301` 先过滤状态，`522-525` 对空候选统一返回 `AI_NOT_CONFIGURED`。建议区分未配置、全部禁用、全部 invalid 和全部 cooling down，并向 UI 返回最近恢复时间。

### P2-7：Provider 错误分类仍依赖字符串和 HTTP status

`src-tauri/src/ai/mod.rs:40-56` 丢弃响应体，`src-tauri/src/ai/router.rs:103-135` 从字符串识别状态；`343-365` 把所有 401/403 永久标为 invalid。模型权限、区域/组织策略或内容策略可能误伤有效 Key。建议 adapter 解析脱敏结构化错误码，只有明确 revoked/invalid key 才永久失效。

### P3-1：可恢复备份、数据导入和批量管理尚未完成

总方针要求 CSV/JSON 导入导出、批量管理和备份验证；当前只有 CSV 导出，没有事务导入、dry-run、冲突策略、恢复验证或批量命令。这属于明确未完成范围，不是单个运行 Bug。

### P3-2：Bundle ID 与旧版数据迁移策略仍未确定

当前标识为 `com.klagragraff.quill`，与 `KlaraGraff` 拼写看起来不一致；代码没有从上游 `com.wycstudios.quill` 应用目录和旧 Keychain service 迁移。发布前必须确认最终 ID；若希望继承原版数据，需要幂等、可回滚迁移，否则应在 UI 明确两版数据隔离。

### P3-3：若未来启用 macOS App Sandbox，iCloud 文件夹授权不能跨重启恢复

当前只保存绝对路径，没有 security-scoped bookmark；`tauri.conf.json` 又明确未启用 entitlements，因此这不是当前非沙盒构建的现行 Bug。但将来启用 App Sandbox 时必须持久化、解析和更新 bookmark，并管理 start/stop access 生命周期。

## 3. 原文档执行对照

| 领域 | 静态结论 | 说明 |
|---|---|---|
| 暖纸视觉、字体与阅读层级 | 基本完成 | 代码已有 paper theme、Palatino 和克制的阅读设置；本轮未做截图验收。 |
| 查词悬浮框与 AI 侧栏追问 | 基本完成 | 引用进入按书聊天、请求取消和已有窗口导航 ack 已实现；仍需运行验证。 |
| API Key 安全存储 | 基本完成 | WebView 禁读明文，系统凭据后端和迁移校验已补齐；真实持久化仍需运行验证。 |
| 多 API Key 自动切换 | 主要完成但不可靠 | 列表、优先级、健康状态、测试和首 token 前切换已有；默认 URL、错误分类和状态提示有缺陷。 |
| iCloud 私有 container 替换 | 主要完成 | 已改为用户选择 iCloud Drive 目录、动态 asset scope、queue-only 与 blob reconcile；同步水位、迁移事务和时间信任仍有风险。 |
| 查询历史、搜索、删除、导出、保留期 | 基本完成 | 分页、全量聚合、核心操作和 prune 后跨窗口刷新均已实现；仍需运行验证。 |
| EPUB 正文标记 | 基本完成 | 统一差量快照、状态开关、点击入口、临时标注恢复和侧栏刷新已有；仍需运行与视觉验证。 |
| SRS 复习 | 部分完成 | Again/Hard/Good/Easy、due、interval 和同步字段已有；不是成熟 SRS。 |
| TXT 与常见格式 | 部分完成 | 已接入 TXT/MD/HTML/MOBI/AZW/FB2/FBZ/CBZ，并新增初步能力模型；来源模型、解析器、确定性和能力接线仍不完整。 |
| 当前版本/原项目双展示 | 主要完成 | 当前版为主、上游与 MIT 署名保留；缺 build metadata。 |
| 更新通道隔离 | 基本完成 | 上游 updater 已移除/禁用；自有签名发布通道尚未建立。 |
| 数据导入、备份恢复、批量管理 | 未完成 | 只有单向 CSV 导出。 |
| 分阶段验收与完整验证矩阵 | 未按原文档完成 | 当前改动跨越多个阶段且仍未完成本轮要求之外的运行/视觉/双设备验收。 |

## 4. 建议处理顺序

1. 先修复 migration 009 的嵌套事务阻断，再处理同步水位连续性。
2. 修复 UTF-8 标题截断、SSE 解析、OpenAI 默认 URL 和错误分类。
3. 完善格式能力模型与 importer 数据边界。
4. 最后补齐 importer 数据模型、成熟 SRS、About build metadata 和可恢复备份。

## 5. 仍须运行验证的风险

本报告没有运行任何验证，因此以下事项只能列为待验收，不能由静态阅读证明：

- 系统 Keychain 在开发包、签名包、升级和重启后的持久化。
- iCloud 目录权限、文件驱逐/下载、离线恢复、双设备冲突和大规模日志回放。
- OpenAI、Anthropic、兼容接口、OAuth 和 Ollama 的真实 SSE、取消和多 Key failover。
- 所有宣称格式的真实导入、渲染、进度恢复与能力限制。
- 标注在分页、切章、resize、删除、变色和跨窗口导航后的视觉状态。
- Bundle ID、旧版升级、DMG/NSIS、签名、公证和自有更新通道。

## 6. 本轮边界

本轮只新增本 Markdown 报告，没有修改任何实现源码，也没有运行应用、构建、测试、Lint、类型检查或实机验证。审查期间工作区代码仍在发生更新，因此本报告以顶部记录的快照时间为准。
