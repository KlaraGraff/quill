# 个性化阅读器实施结果静态复审

> 复审日期：2026-07-12  
> 仓库：`KlaraGraff/quill`  
> 分支：`feat/personalized-reader`  
> 审查对象：`HEAD 5595d37` 加当前未提交工作区改动  
> 对照文档：`docs/reviews/personalized-reader-static-code-review-2026-07-12.md`  
> 审查方式：只阅读当前源码、配置、依赖锁文件和 Git 差异  
> 明确未执行：应用启动、测试、构建、类型检查、Lint、真实 AI 请求、iCloud 实机同步、安装包制作、签名、公证和视觉验收

## 1. 结论

当前改动覆盖了原方案的大部分功能面，但不能认定为“已经全部按原文档完成”。安全边界、AI 多凭据界面、查询历史管理、格式入口、当前版本/原项目双分组等已有明显实施；iCloud、系统凭据存储、格式转换、正文标注生命周期、成熟 SRS 和完整版本元数据仍存在关键缺口。

本次静态复审确认：

- 4 项 P0：包括密钥迁移后丢失、生产环境无法读取书籍、同步越界路径和同步水位污染。
- 10 项 P1：可能造成同步数据缺失、崩溃、AI 输出损坏、永久标注被误删、孤儿文件或内存耗尽。
- 13 项 P2 与 3 项 P3：功能不完整、时序不可靠、统计不准确、明显偏离原方案，或属于特定发布方式下的前置条件。
- 原文档中标为“已完成”的 iCloud、格式扩展、SRS、Keychain 和版本身份，应分别修正为“部分完成”或“实现有阻断缺陷”。

以下结论均为静态推断，不代表运行时已经复现，也不能证明未列出的路径没有问题。

## 2. 问题清单

### P0-1：macOS 凭据后端实际是内存 Mock，迁移后会永久删除旧密钥

证据：

- `src-tauri/Cargo.toml:51` 仅声明 `keyring = "3"`，没有启用 macOS 的 `apple-native` feature。
- `src-tauri/Cargo.lock:2433-2440` 中 `keyring 3.6.3` 只有 `log`、`zeroize` 依赖，没有 `security-framework`，可确认 Apple 后端未进入依赖图。
- `keyring 3.6.3` 在 macOS 未启用 `apple-native` 时，默认 credential builder 是进程内 Mock；Mock 不跨进程持久化。
- `src-tauri/src/secrets.rs:43-49` 无条件把 `use_keychain` 设为 `true` 并立即执行旧 SQLite 迁移。
- `src-tauri/src/secrets.rs:149-176` 和 `198-232` 在目标写入返回成功后删除 settings/legacy SQLite 中的旧值。

影响：首次迁移所在进程中密钥可能暂时可用，但应用重启后 Mock 内容消失；旧 API Key 和 OAuth token 已从原存储删除，用户必须重新配置。原文档宣称“凭据已迁移到系统 Keychain”与当前依赖配置不符。

推荐措施：

1. 按平台显式启用持久后端，macOS 至少使用 `apple-native`；Windows/Linux 也应分别指定受支持的持久后端。
2. 初始化时检查 credential builder 的持久化能力，Mock 或非持久后端必须拒绝生产迁移。
3. 改为可恢复的两阶段迁移：写入、读回校验、记录迁移完成，最后才删除旧值。
4. 对已经运行过当前版本的用户提供旧 SQLite/备份恢复路径；不要再次无条件清理。

### P0-2：生产 CSP 会阻止阅读器读取书籍文件

证据：

- `src-tauri/tauri.conf.json:24` 的 `connect-src` 没有 `asset:` 或 `http://asset.localhost`。
- `src/pages/Reader.tsx:576-591` 对 `convertFileSrc(book.file_path)` 返回的 asset URL 执行 `fetch()`，然后才交给 Foliate。

影响：生产 WebView 中 EPUB、PDF、文本转换书和其他原生格式都可能在 `fetch(fileUrl)` 处被 CSP 拦截，导致阅读主路径无法打开书籍。这是新增严格 CSP 与当前读取方式之间的直接配置冲突。

推荐措施：只在 `connect-src` 最小化加入 Tauri asset 协议对应来源；同时核对 PDF worker 所需的 `worker-src`。修复后必须用生产构建分别打开本地和 iCloud 书籍，开发服务器结果不能替代该验证。

### P0-3：同步输入可写入越界路径，后续删书可能删除资料目录外文件

证据：

- `src-tauri/src/sync/events.rs:136-150` 的 `BookImportPayload` 接受任意 `id`、`file_path` 和 `cover_path`。
- `src-tauri/src/sync/merge.rs:288-312` 将远端路径原样写入数据库；snapshot 和 metadata 路径也缺少统一验证。
- `src-tauri/src/db.rs:269-277` 对绝对路径原样返回，对带 `../` 的相对路径直接 join。
- `src-tauri/src/commands/books.rs:1281-1315` 删除书籍时对解析后的路径直接 `remove_file`，远端 book id 也被拼入 `covers/{id}.img`。

影响：损坏、被篡改或由旧/恶意客户端写入的共享日志，可以把绝对路径、父目录跳转或带分隔符的实体 id 注入本地数据库。用户以后删除该书时，应用可能删除资料目录外的任意可写文件；相关封面读取路径还存在本机文件越界读取风险。

推荐措施：在事件、snapshot、merge、读取和删除入口共用严格的 `ValidatedEntityId` 与 `ValidatedBookBlobPath`；拒绝绝对路径、父目录、平台前缀和不在 allowlist 的扩展名。删除前 canonicalize 父目录并再次确认 containment，不能只在导入入口验证。

### P0-4：同步日志身份未验证，可污染其他设备水位并永久跳过合法事件

证据：

- `src-tauri/src/sync/replay.rs:628-666` 把任意日志文件 basename 当作 peer，不要求合法设备 UUID。
- `src-tauri/src/sync/log.rs:249-267` 只反序列化每行 JSON，没有核对 `event.device`、schema 版本、ULID 或文件内顺序。
- `src-tauri/src/sync/replay.rs:464-478` 读取水位时使用文件名对应的 peer，`516-519` 推进水位时却使用 payload 内的 `event.device/event.id`。
- snapshot 入口同样缺少“文件名设备 == payload 设备”的统一验证。

影响：伪造日志可以声明来自另一台合法设备并携带极大的 event id，把该设备水位推进到未来，使它之后的真实事件永久被跳过；payload 设备与文件名不一致时，也可能导致事件反复重放。

推荐措施：只发现合法 UUID 文件名；逐事件校验 device、受支持 schema、合法且严格递增的 ULID。水位只使用验证后的文件身份，异常日志/snapshot 应隔离并显示错误，不能静默参与回放。

### P1-1：iCloud 目录暂时不可达时没有进入离线队列，新修改不会等待后续同步

证据：

- `src-tauri/src/lib.rs:449-451` 只有记录目录本次启动仍可写时才得到 `ubiquity_dir`。
- `src-tauri/src/lib.rs:496-499` 只有 `ubiquity_dir.is_some()` 才执行 `set_should_queue(true)`。
- `src-tauri/src/lib.rs:562-566` 对“同步已启用但目录暂不可达”仅记录警告。
- `src-tauri/src/sync/writer.rs:44-58` 明确设计了 queue-only 模式，目的就是把离线期间事件写入 `_pending_publish`，但启动代码没有按持久化同步意图启用该模式。

影响：用户已经启用同步后，如果某次启动 iCloud Drive 暂不可达，本次会话中的书籍、收藏、高亮、词汇和阅读进度修改可能只落本地数据库，不写 outbox；iCloud 恢复后这些修改没有事件可发布到其他设备。

推荐措施：`should_queue` 应由持久化的 `sync_enabled` 决定，目录是否可用只决定能否挂载日志和立即 flush。补充离线写入、重启恢复、outbox drain 和冲突回放场景。

### P1-2：中文等多字节聊天内容可使标题生成线程 panic

证据：`src-tauri/src/commands/ai.rs:345-353` 先用字节长度判断，再用 `&user_message[..200]` 和 `&assistant_message[..200]` 切片。Rust 字符串切片边界必须位于 UTF-8 字符边界。`src/hooks/useAiChat.ts:285-288` 会直接把用户输入或上下文交给标题生成。

影响：当第 200 个字节位于中文、日文、emoji 等多字节字符中间时，后台任务 panic，标题生成失败，并可能污染相关异步任务状态。

推荐措施：按 `chars()` 或 grapheme cluster 截断；若目标是限制请求体大小，使用安全的 UTF-8 边界函数并覆盖中英文混排、emoji 和恰好跨 200 字节的用例。

### P1-3：AI 流按网络分块独立做 lossy UTF-8 解码，可能损坏中文和 SSE JSON

证据：

- `src-tauri/src/ai/openai_compat.rs:60-72`
- `src-tauri/src/ai/openai_responses.rs:76-87`
- `src-tauri/src/ai/anthropic.rs:77-88`

三处都对每个网络 chunk 单独调用 `String::from_utf8_lossy`。UTF-8 字符可以跨 chunk，前一块末尾和后一块开头会分别变成替换字符。三处还只接受带空格的 `data: `，合法的 `data:` SSE 行会被忽略。

影响：中文输出可能出现 `�`，JSON 可能解析失败，最终表现为缺字、流中断或错误切换凭据。

推荐措施：保留字节缓冲，先按 SSE 行边界切分完整字节，再严格 UTF-8 解码；按 SSE 规范解析字段，兼容 `data:` 与 `data: `、CRLF、多行 data 和尾部残留。Provider adapter 应返回结构化事件，而不是静默忽略 JSON 错误。

### P1-4：正文标注以 CFI 作为唯一覆盖键，临时定位效果会删除永久标注

证据：

- `src/pages/Reader.tsx:1304-1308`
- `src/pages/Reader.tsx:1650-1654`
- `src/pages/Reader.tsx:1682-1686`

三处定位后用真实 CFI 添加紫色 annotation，3 秒后用同一个 CFI 调用 `deleteAnnotation`。`public/foliate-js/view.js:398-406` 显示 overlayer 以该 value 删除并重建 annotation。

影响：目标位置若原本已有生词标记、查询标记或手动高亮，临时紫色效果会覆盖它，计时结束后又把该 CFI 的 annotation 整体删除，永久标记从当前阅读视图消失。

推荐措施：临时导航效果使用独立 ID/独立 overlay，不能复用业务 CFI 作为 annotation key；结束临时效果后恢复该 CFI 的快照状态。

### P1-5：标注变色存在 delete/add 并发竞态，侧栏删除高亮不会清理阅读视图

证据：

- `src/pages/Reader.tsx:382-390` 把同一 CFI 的旧 annotation 删除和新颜色添加放入同一个 `Promise.all`。
- `public/foliate-js/view.js:398-406` 的删除和添加都先异步解析导航位置；完成顺序不确定，晚完成的删除可移除新颜色。
- `src/components/BookmarksPanel.tsx:215-220` 从侧栏删除高亮只调用 hook。
- `src/hooks/useBookmarks.ts:101-104` 只删除数据库记录和 hook 本地状态，没有通知 Reader 删除对应 CFI。
- `src/pages/Reader.tsx:394-410` 的刷新逻辑会补加现有手动高亮，但没有维护“先前手动高亮”集合用于差量删除。

影响：颜色更新可能随机变成无标注；从书签/高亮侧栏删除后，正文旧高亮可一直保留到 overlay 重建或重新打开书籍。

推荐措施：同一 CFI 的操作必须顺序执行，先 await delete 再 add；统一维护所有 annotation 的已应用快照，并让手动高亮新增、变色、删除都走同一差量协调器。

### P1-6：书籍导入先写最终文件再提交数据库，失败会留下孤儿文件

证据：

- EPUB：`src-tauri/src/commands/books.rs:586-615`
- 文本转换：`src-tauri/src/commands/books.rs:653-683`
- MOBI/FB2/CBZ 等原生格式：`src-tauri/src/commands/books.rs:717-741`
- PDF：`src-tauri/src/commands/books.rs:770-801`

这些路径都在 `do_insert_book` 前把文件复制或 rename 到最终 `books/` 路径。数据库事务、同步 outbox 或 cover 处理失败时，没有回滚最终文件；文本 EPUB 在 `write_internal_epub` 或 rename 失败的部分路径也没有统一临时文件清理 guard。

影响：导入向用户报告失败，但磁盘中留下无法从 UI 管理的书籍或 `.tmp`；反复重试持续占用空间。该实现偏离原方案“临时目录完成检测/转换/回读，失败清理，最后原子提交”的要求。

推荐措施：建立 import transaction/cleanup guard；所有解析和生成先在专用临时目录完成，数据库失败时删除生成物，只有文件和数据库均成功才对外返回。还应提供启动时的孤儿文件审计策略。

### P1-7：AI 错误只按状态文本分类，可能把有效凭据永久标成 invalid

证据：

- `src-tauri/src/ai/mod.rs:42-56` 读取并丢弃 Provider 响应体，只保留 HTTP status 和 `Retry-After`。
- `src-tauri/src/ai/router.rs:103-135` 从错误字符串搜索 `401/403/429` 等文本进行分类。
- `src-tauri/src/ai/router.rs:343-369` 把所有 401 和 403 分别归入 Auth/Permission，并永久设置为 `invalid`。

影响：模型权限、区域限制、组织策略、内容策略或临时账号状态导致的 403，都可能让一把本来有效的 Key 永久退出候选列表；之后的请求不会自动恢复第一优先级凭据。

推荐措施：Provider adapter 解析脱敏后的结构化错误码，区分凭据无效、模型权限、请求/内容策略、配额和区域限制；只有明确的 revoked/invalid key 才永久失效，其余使用请求级失败或有期限冷却。

### P1-8：同步启用和停用过程中缺少全局写入门闩，存在丢事件或 blob 的竞态

证据：

- `src-tauri/src/commands/sync.rs:343-370` 启用时先发布 bootstrap snapshot，之后才开启 queue；两者之间的修改既不在 snapshot，也可能不进 outbox。
- `src-tauri/src/commands/sync.rs:449-546` 停用时先停止 watcher 并复制 books/covers，较晚才关闭 writer 和切换 `data_dir`；复制遍历期间的新导入可能只留在 iCloud 一侧。
- 当前没有让其他窗口和 MCP 写命令共同遵守的 sync transition gate。

影响：用户在同步启停进度中继续阅读、编辑或导入时，数据库事件与书籍 blob 可能只存在一侧，造成其他设备缺失或当前设备重启后文件不可见。

推荐措施：增加进程级 transition gate，并让 UI、Tauri command 和 MCP 共用。启用时先可靠排队再生成一致 snapshot；停用时冻结 blob mutation，完成最终 inventory、复制、校验后原子切换目录和 writer 状态，失败需恢复旧状态。

### P1-9：全新安装只添加 OpenAI Key 时，默认请求会发往本机 Ollama

证据：

- `src-tauri/src/ai/router.rs:199-207` 新建默认 profile 为 `provider=openai`，但没有旧设置时 `base_url` 为 NULL。
- `src-tauri/src/ai/router.rs:416-423` 除特殊分支外，空 base URL 一律回退到 `http://localhost:11434`。

影响：新用户按常规流程只添加 OpenAI API Key 后，请求会发往本机 Ollama 而不是 OpenAI，通常表现为连接失败，并可能被误认为 Key 无效。

推荐措施：按 provider 解析后端默认地址：OpenAI/Anthropic 使用各自官方地址，Ollama 才使用本机地址，custom 必须显式填写；迁移创建 profile 时直接持久化解析后的默认值。

### P1-10：同步日志和 snapshot 无尺寸上限并整文件读入内存

证据：

- `src-tauri/src/sync/log.rs:204-266` 在线程中 `fs::read` 整个 JSONL，再构造完整 `Vec<Event>`。
- replay 在 `src-tauri/src/sync/replay.rs:464-483` 又把各 peer 的所有事件合并进一个 Vec。
- snapshot 也使用整文件读取/反序列化；超时只能停止等待，不能取消已开始的 `fs::read`。

影响：共享目录中的超大、损坏或恶意日志会造成高内存和 CPU，最坏可导致 OOM；这也放大了同步输入不可信问题。

推荐措施：读取前设置文件尺寸上限；JSONL 使用 `BufRead` 流式处理并限制单行、单事件和每 tick 数量；snapshot 使用有限 reader。超限 peer 文件应隔离并向 UI 报错。

### P2-1：关闭查词、解释和翻译弹窗不会取消后端 AI 请求

证据：`src/components/LookupPopover.tsx:64-113`、`ExplainPopover.tsx:49-95`、`TranslationPopover.tsx:79-127` 在 cleanup 中只移除事件 listener，没有保存 request ID 并调用 `ai_cancel`。Chat 在 `src/hooks/useAiChat.ts:438-452` 已有取消逻辑，因此不是后端能力缺失。

影响：用户关闭弹窗、切书或重新选词后，旧请求仍消耗配额、占连接，并会更新凭据健康状态或触发 failover。

推荐措施：将 request ID 保存到 ref，组件卸载和显式关闭时调用 `ai_cancel`；取消必须保持“不冷却、不切 Key、不持久化”。

### P2-2：AI 标题超时不会解除监听或取消请求，request ID 也不保证唯一

证据：`src/hooks/useAiChat.ts:63-110` 使用 `title-${Date.now()}`；超时回调只 resolve `null`，未调用 `unlisten()` 或 `ai_cancel`。同一毫秒内的多个窗口可能生成相同 request ID 和事件名。

影响：超时任务继续运行，listener 保留到后端最终完成；并发窗口存在标题 token 串入另一请求的可能。

推荐措施：使用 `crypto.randomUUID()`，在 finally 中统一 unlisten、清 timeout 并取消未完成请求。

### P2-3：已有阅读窗口导航仍可能在 Reader 就绪前丢失

证据：

- `src/utils/openReaderWindow.ts:38-48` 对已有窗口直接 emit 后 focus，没有 navigation ID、ack 或重试。
- `src/pages/Reader.tsx:1025-1035` 收到事件后立即对 `viewRef.current` 调用 `goTo`，没有等待 `bookReady`，也没有保存 pending navigation。

影响：窗口对象已经存在但书籍仍在初始化、重载或 view 尚未创建时，“回到原文”不会跳转；调用方无法判断成功还是失败。原方案要求的“先导航、再打开面板、回传完成/失败”仅部分实现。

推荐措施：Reader 缓存最后一个 pending navigation，在 `bookReady && viewRef.current` 后执行；协议增加 navigation ID 和 ack，调用方在确认完成后再聚焦/提示。

### P2-4：原生格式的首次 URL 导航和阅读设置没有使用能力模型

证据：

- `src/pages/Reader.tsx:1297-1313` 的 URL/location state 初始化没有 `supportsTextAnchors` guard，却会打开词汇侧栏并尝试 CFI 导航。
- `src/pages/Reader.tsx:1483` 把所有非 PDF 格式都传成 `epub` 给 `ReaderSettings`。

影响：MOBI/FB2/CBZ 通过首次 URL 打开时可能出现空词汇面板；CBZ 等格式会显示不适用的字体、行距、边距或流式排版设置。

推荐措施：所有入口统一使用 `ReaderCapabilities`，面板、导航、选词、annotation 和设置项都由 capability 决定，不再用 `pdf ? pdf : epub` 二分法。

### P2-5：查询历史的书籍筛选计数只基于当前已加载页

证据：

- `src/components/DictionaryContent.tsx:133-141` 从当前 `records` 构建书籍 pills 和 count。
- `src/components/DictionaryContent.tsx:301` “全部书籍”显示 `records.length`，而 hook 已提供真实的 `historyTotal`。
- `src/hooks/useDictionary.ts:170-180` 首次只读取 50 条，后续手动 load more。

影响：超过 50 条历史后，计数偏小；只出现在未加载页中的书籍不会成为筛选项，用户误以为该书没有查询记录。

推荐措施：后端分页响应同时返回全量总数和按书聚合计数；UI pills 不应从当前页推导。

### P2-6：修改保留期限后，已打开的历史和正文标记不会立即刷新

证据：`src/components/settings/GeneralSettings.tsx:89-93` 保存设置并执行 `prune_lookup_records`，但没有发出 `lookup-record-changed` 或同步刷新历史视图。Reader 的刷新入口位于 `src/pages/Reader.tsx:1005-1023`，依赖事件或窗口 focus。

影响：数据库已删除记录，当前历史列表和正文“已查询”标记仍显示旧数据，直到用户触发其他刷新。

推荐措施：prune 返回受影响书籍/CFI 或发出全局失效事件；历史 hook 与所有 Reader 窗口收到后重新加载。

### P2-7：格式扩展只是部分执行，转换结果与原方案的数据模型不一致

证据：

- `src-tauri/migrations/016_book_source_formats.sql:1-8` 只增加 `source_format`、`render_format`、`conversion_version`，缺少原方案的 `source_file_path` 和 `source_sha256`。
- 文本导入 `src-tauri/src/commands/books.rs:625-684` 不保留原文件。
- `src-tauri/src/commands/books.rs:824-860` 对所有格式都写固定 `TEXT_CONVERSION_VERSION`，但 `BookImportPayload` 没有同步 conversion version。
- `src-tauri/src/commands/books.rs:214-274` 用手写字符串规则处理 Markdown/HTML，不是原方案要求的成熟 CommonMark parser 和 HTML parser/sanitizer，也不支持本地资源打包。
- `src-tauri/src/commands/books.rs:156-169` 只在文件头中寻找 FB2 root，UTF-16 或 root 较晚的合法 FB2 会被拒绝。
- `src-tauri/src/commands/books.rs:184-203` 编码探测没有置信度或用户选择路径。
- `src-tauri/src/commands/books.rs:367-368` 每次转换生成随机 EPUB identifier，同一来源和 conversion version 不能得到确定性产物。

影响：无法审计来源、去重或稳定重建；设备间 conversion metadata 不完整；复杂 Markdown/HTML 会丢语义，合法编码可能误判。当前只能称为“增加若干格式入口和基础转换”，不能称为完整执行原格式方案。

推荐措施：引入 importer registry 与成熟 parser/sanitizer；补齐来源哈希、原文件策略、conversion version 的事件/快照同步和确定性 identifier；编码不确定时交给用户选择。

### P2-8：SRS 有四档评分，但仍是自制固定倍率算法

证据：`src-tauri/src/commands/vocab.rs:111-149` 对 Again/Hard/Good/Easy 使用固定 10 分钟和 `1.2/2.5/3.5` 倍率，并以 21 天阈值直接标记 mastered。

影响：显式评分、复习次数和同步字段已经完成，但没有实现原文档要求的成熟 SRS 算法；长期记忆难度、稳定性、遗忘概率和历史评分没有进入调度模型。

推荐措施：采用 FSRS 等成熟、可测试的算法和版本化参数；迁移现有 interval/due 状态，并确保跨设备只同步复习事件或确定性状态。

### P2-9：关于页完成双仓库展示，但缺少原方案要求的版本身份元数据

证据：`src/components/settings/AboutSettings.tsx:47-143` 已正确区分当前仓库和上游仓库，但只显示当前 app version 和从 UA 推断的平台；没有上游基线、构建 commit、build date、channel、当前维护者或“复制版本信息”。

影响：用户仍无法准确判断安装包基于哪个上游版本、对应哪个源码提交或属于哪个更新通道。原文档第 6.5、6.8 节只能算部分执行。

推荐措施：由 CI 注入不可变 build metadata，About 页展示当前版本、上游基线、短 SHA 和 channel，并提供不含用户路径/凭据的复制诊断信息。

### P2-10：Bundle ID 改名没有旧数据/Keychain 迁移，且标识疑似拼写错误

证据：`src-tauri/tauri.conf.json:3-5` 使用 `Quill Personal` 和 `com.klagragraff.quill`；`src-tauri/src/lib.rs:47-57` 同样用该标识计算 app-data/log 路径。当前代码没有从上游 `com.wycstudios.quill` 的应用目录或旧 Keychain service 迁移数据。

影响：覆盖安装或从原版迁移的用户可能看到空书库和空设置。`klagragraff` 与仓库维护者 `KlaraGraff` 拼写不一致，一旦发布后再修正会再次改变应用身份和凭据命名空间。

推荐措施：发布前确认最终 Bundle ID；若需要继承旧用户数据，做显式、幂等且可回滚的数据和 Keychain 迁移，并在 UI 中说明原版与 Personal 版的数据边界。

### P2-11：所有 Key 冷却、失效或禁用时被误报为“AI 未配置”

证据：`src-tauri/src/ai/router.rs:274-301` 在查询候选阶段过滤 invalid 和未到期 cooldown；`src-tauri/src/ai/router.rs:522-525` 对空候选统一返回 `AI_NOT_CONFIGURED`。

影响：用户已经配置凭据，却被引导重复配置；UI 无法区分等待冷却、全部失效、全部禁用和真正未配置，也无法显示最近恢复时间。

推荐措施：先统计配置总量与状态，再返回 `AI_KEYS_COOLING_DOWN`、`AI_ALL_KEYS_INVALID`、`AI_KEYS_DISABLED` 或真正的 `AI_NOT_CONFIGURED`。

### P2-12：删除 API Key 先删数据库再删凭据，失败会留下不可定位的孤儿 secret

证据：`src-tauri/src/ai/router.rs:705-716` 先删除 `ai_credentials` 行，随后调用 `secrets.delete(secret_ref)`。

影响：Keychain 删除失败时 command 返回失败，但数据库已经丢失 `secret_ref`；刷新后 UI 看不到该 Key，也没有普通路径再次定位并清理凭据。

推荐措施：使用可恢复的 `pending_delete` 状态或补偿事务，确保重试始终保留 secret_ref；删除顺序必须同时考虑“数据库失败”和“Keychain 失败”两侧恢复。

### P2-13：总方针中的可恢复备份、数据导入与批量管理尚未实现

证据：总方针要求 CSV/JSON 导入导出、批量管理和备份验证；当前 `src/components/DictionaryContent.tsx:160-194` 只有 CSV 下载，没有对应 CSV/JSON 导入、dry-run、冲突策略或词汇批量 command。

影响：当前导出只是单向报表，无法证明可以恢复；“直到全部执行”的范围尚未完成。

推荐措施：定义版本化导出 schema，增加解析预览、事务导入、冲突策略和 dry-run；至少覆盖词汇、SRS 元数据和必要书籍关联。批量删除/改状态必须二次确认并在单事务执行。

### P3-1：README 的格式能力说明落后于实现

`README.md:7-15` 主要列出 EPUB/PDF/TXT，未完整说明 Markdown、HTML、MOBI/AZW/AZW3、FB2/FBZ、CBZ 以及各格式的选词、标注和文本层限制。建议用能力表统一“可导入、是否转换、可选词、可高亮、可同步”的声明。

### P3-2：数据库 migration 与 schema version 更新没有同一显式事务

`src-tauri/src/db.rs:228-246` 对 migration 执行 `execute_batch` 后，再单独更新 `schema_version`；015-017 本身没有显式事务。中途遇到磁盘、锁或损坏错误时，前面 DDL 可能已落盘而版本未推进，下次重试可因“列已存在”持续失败。建议每个 migration 和版本更新放入同一显式事务，并为可能部分应用的 016/017 提供幂等修复。

### P3-3：若未来启用 macOS App Sandbox，当前同步目录授权不能跨重启恢复

证据：`src/components/settings/LibrarySyncSettings.tsx:163-170` 向 Tauri dialog 传入 `fileAccessMode: "scoped"`，但当前锁定的 `tauri-plugin-dialog 2.6.0` 只在 iOS 使用该选项；`src-tauri/src/sync/migration.rs:94-100` 只保存绝对路径，没有持久化 macOS security-scoped bookmark。

当前影响边界：`src-tauri/tauri.conf.json:84-86` 没有配置 entitlement，当前 Developer ID/非沙盒分发不因此直接失效，所以不能把它认定为当前运行 bug。但若以后启用 App Sandbox 或采用要求沙盒的分发方式，重启后可能无法恢复用户选择目录的权限。

推荐措施：把“是否启用 App Sandbox”作为明确发布决策。若启用，使用原生 security-scoped bookmark，处理 bookmark 持久化、解析、stale 更新以及 `start/stopAccessingSecurityScopedResource` 生命周期，并在签名沙盒包中实机验证；若保持非沙盒分发，则删除无效的 `fileAccessMode: "scoped"` 参数并在架构文档中写清边界。

## 3. 原文档执行对照

| 原方案领域 | 静态结论 | 说明 |
|---|---|---|
| 敏感设置 IPC 禁读、CSP | 存在阻断回归 | 通用设置已过滤敏感键；Keychain 后端缺失，且 CSP 会阻止当前书籍 fetch。 |
| iCloud 私有 container 替换 | 部分完成 | 已改为用户选择 iCloud Drive 文件夹并移除上游 entitlement；当前离线队列有缺陷，若未来启用 App Sandbox 还需补齐 bookmark 授权持久化。 |
| 正文标记差量刷新与点击 | 部分完成 | 已有快照、差量和点击入口；变色竞态、临时标注误删、侧栏删除残留尚未解决。 |
| 已有阅读窗口导航 | 部分完成 | 已加入 `reader:navigate`；缺 ready queue、navigation ID 和 ack。 |
| Chat 请求隔离、失败不持久化 | 基本完成 | Chat 使用 request ID 和取消；非 Chat 弹窗未取消，标题请求仍有泄漏/冲突风险。 |
| 多 API Key 自动切换 | 主要完成但不可靠 | Profile/Credential、优先级、健康状态、首 token 前 failover 已实现；凭据不持久和错误误分类会破坏实际可用性。 |
| TXT 与常见格式 | 部分完成 | 已接入 TXT/MD/HTML/MOBI/AZW/FB2/FBZ/CBZ 和严格入口；转换架构、来源模型、parser/sanitizer、确定性和失败清理未达原方案。 |
| 删除书籍清理查询历史 | 完成 | 本地删除和同步回放均已补齐 lookup history 清理。 |
| 查询历史分页/搜索/删除/导出/保留期 | 主要完成 | 核心命令与 UI 已有；按书计数和 prune 后刷新仍不正确。 |
| SRS | 部分完成 | 四档评分、interval/due/review count 和同步字段已实现；算法不是成熟 SRS。 |
| 当前版本/原项目双展示 | 部分完成 | 双分组和链接归属正确；上游基线、commit、channel、维护者和诊断信息缺失。 |
| 更新通道隔离 | 基本完成 | 上游 updater 已移除/禁用；自有发布通道尚未建立。 |
| 查询历史跨设备同步 | 按文档暂缓 | 当前保持 local-only，与原文档的后续产品决策一致，不视为本轮 bug。 |
| 标记显示偏好跨设备同步 | 按文档暂缓 | 当前保持设备本地设置，与原文档记录的后续事项一致。 |
| 数据导入、可恢复备份、批量管理 | 未完成 | 只有 CSV 导出，没有导入、恢复验证和批量命令。 |

## 4. 后续推荐顺序

1. 先修复四项 P0：Keychain 持久后端、生产 CSP、同步路径边界、同步身份与水位验证；在此之前不要发布当前构建。
2. 修复 iCloud queue-only 和启停一致性；若发布决策包含 App Sandbox，再补齐 security-scoped bookmark 并做签名沙盒实机验证。
3. 修复 UTF-8 panic/流解码、标注覆盖模型、导入回滚、OpenAI 默认地址和同步文件上限。
4. 完善 Provider 结构化错误分类、凭据状态/删除补偿和所有 AI 请求取消。
5. 再完成格式 importer、成熟 SRS、导航 ack、历史聚合统计、About build metadata 和可恢复数据备份。

## 5. 必须留到后续运行验证的风险

本次没有执行下列验证，因此静态代码即使修正也不能直接判定功能可发布：

- macOS Keychain 在开发包、签名包、升级安装和重启后的真实持久化。
- iCloud Drive 目录授权跨重启、文件驱逐/下载、离线 outbox、双设备冲突和恢复。
- OpenAI、Anthropic、兼容接口、OAuth 与 Ollama 的真实流协议、取消和多 Key failover。
- EPUB/PDF/TXT/Markdown/HTML/MOBI/AZW/AZW3/FB2/FBZ/CBZ 的真实导入与 Foliate 行为。
- 中文、emoji、超长文本、SSE 分块和 Provider 错误体边界。
- 标注在分页、切章、resize、导航、删除和变色后的视觉状态。
- Bundle ID、升级迁移、DMG/NSIS、签名、公证和发布通道。

## 6. 本轮变更边界

本次复审只新增本 Markdown 报告，没有修改任何实现源码或原审查文档。除 `git diff --check` 这种静态差异完整性检查外，没有运行任何测试或验证命令；所有运行时结论仍待后续专门验收。
