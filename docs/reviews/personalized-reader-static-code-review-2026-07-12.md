# 个性化英语阅读器静态代码审查与后续改进方案

> 审查日期：2026-07-12  
> 仓库：`KlaraGraff/quill`  
> 分支：`feat/personalized-reader`  
> 当前提交：`5595d37`  
> 对比基线：`b28668f`（上游 `v1.2.10`）  
> 原始审查方式：只阅读源码、配置和提交差异  
> 执行更新：2026-07-12，已按本文件实施可在代码层面完成的改进。未启动应用、未进行 iCloud 实机同步、未调用真实 AI API、未创建安装包或签名。

## 1. 结论摘要

本次静态审查和后续代码执行不能证明应用已经可以正常运行，也不能替代实机验收。首期功能现已覆盖暖色阅读主题、查词历史、生词收藏、学习状态、正文词汇标记开关，以及从查词进入 AI 侧栏等主要界面；本轮已处理可由静态实现确认的安全、同步、格式与核心交互问题。

原始建议优先级如下：

1. 先封闭 WebView 读取 API Key 的通路，并为电子书内容建立 CSP 安全边界。
2. 修复 iCloud 身份、容器解析和错误分类，再判断同步是否可用。
3. 修复正文标记的增量刷新、已有阅读窗口导航、查询失败入库和删除书籍后的数据残留。
4. 统一 AI 请求路由和流式事件协议后，再实现多 API Key 自动切换。
5. 先建立严格的格式识别与导入分发，再以 TXT 转内部 EPUB 的方式扩展格式。

截图中的 iCloud 报错来自上游私有 container 约定。当前实现已改为用户选择 iCloud Drive 文件夹，不再使用或访问上游私有 container；当前 Mac 的登录、iCloud Drive 与网络状态仍需要实机确认。

## 执行状态（2026-07-12）

### 已完成

- P0-1：通用设置命令会拒绝敏感键的读写；凭据已迁移到系统凭据存储；Tauri CSP 限制电子书脚本来源。
- P1-1、P1-2：同步改为用户选择 iCloud Drive 文件夹，移除上游私有 container、entitlement 与 updater 依赖；设置页使用结构化错误码和本地化文案。
- P1-3、P1-4：正文标记使用差量 add/delete，并在查询、收藏、学习状态和跨窗口事件后刷新。
- P1-5：已有阅读窗口接收 `reader:navigate`，可跳到 CFI 并打开词汇或 AI 面板。
- P1-6、P1-7：Chat 事件按 request ID 隔离；失败流以结构化失败结束，不写入查询历史或聊天记录。
- P1-8：导入使用 EPUB/PDF/TXT allowlist、容器检查和结构化错误；TXT 转稳定内部 EPUB。
- P1-9：本地删除和同步回放删除书籍时均清理查询历史。
- P1-10：产品名、版本、仓库链接和更新策略已改为 `Quill Personal`；自动更新未配置时保持禁用。
- P2-2、P2-4：自动词汇标记可打开既有释义或词汇详情；词汇列表移除了嵌套 button。
- P2-6：API Key 与 OAuth token 由系统 Keychain 保存，SQLite 不保存明文。
- P2-8、P2-9：启动和 MCP 只会使用仍位于 iCloud Drive 且可写的记录目录；失效目录不会被创建或作为 blob 根目录。同步错误已映射为中英文文案，只有可恢复的目录错误显示重试。
- P3-1、P3-2：词汇与查询历史搜索覆盖单词、释义、上下文和书名；删除虚假的 `p. 1` CFI 显示。
- “当前版本 / 原始项目”展示方案：关于页以前者为主，保留上游仓库与 MIT 署名。

### 本轮补充修复

- 手动测试某个已标记为 `invalid` 的 API Key 时，会绕过运行时 failover 过滤；测试成功后可恢复为 `active`。
- 改变词汇状态不再增加 `review_count`，并拒绝无效状态值；已实现 `Again / Hard / Good / Easy` 的显式复习命令、间隔、到期时间、复习次数和最后评分。手动改变学习状态时，sync event 会保留现有的 SRS 元数据，不会覆盖另一台设备的复习间隔。
- Reader 对整本书的标记数据建立快照：只有数据、显示设置或窗口焦点变化时读取数据库；Foliate 重建 overlay 时仅重用快照，不再每章重复 IPC 查询。
- AI Router 现在在启动异步任务前注册取消令牌；取消不会使当前 Key 进入冷却或触发故障切换。共享 `reqwest` Client 具备连接、首字节与流空闲超时，Provider 错误会保留秒数或 HTTP-date 形式的 `Retry-After`，用于精确冷却受限 Key。
- 查询历史已提供游标分页、搜索、按书过滤、逐条删除、二次确认清空和 CSV 导出。保留期可在通用设置选择永久、30 天、90 天或 1 年；启用期限后会立即清理，并在每次新查询时继续维护。
- 导入已扩展 Markdown、HTML、MOBI/AZW/AZW3、FB2/FBZ 和 CBZ。文本格式转为内部 EPUB；原生格式保留原文件和扩展名，并以 magic/container 检测拦截伪造文件。CBZ、MOBI/AZW/AZW3、FB2/FBZ 不开放依赖稳定 CFI 的标记、查词、生词、书签或高亮入口；AI 侧栏仍可用于整书讨论。

### 仍需后续实现或产品决策

- P2-5：查询历史与“已查询”标记明确保持本机数据，未跨 iCloud 同步；隐私说明已写明此边界。若未来要同步，仍需完整事件、LWW、tombstone、snapshot 和旧客户端兼容设计。
- P3-4：正文标记显示偏好仍是设备本地 `localStorage` 设置。
- Provider adapter 仍直接向 Tauri event 输出 token，尚未抽象为完全独立、可单测的流协议；这不阻碍首 token 前的多 Key 切换，但后续如增加 Provider 或断点重试，应继续解耦。
- 常见格式当前支持 EPUB、PDF、TXT、Markdown、HTML、MOBI/AZW/AZW3、FB2/FBZ 和 CBZ。尚不支持需要额外解析器或 OCR 的 CBR、KFX、DJVU、扫描型 PDF OCR 与 DRM 内容；不要仅通过扩展名宣称支持。
- iCloud Drive folder picker 的跨重启 security-scoped bookmark 持久化依赖 Tauri dialog plugin 的平台管理；在真实签名与沙盒发布前仍需实机验证授权持续性。

### 本轮静态检查

本轮已通过：

```text
npm exec tsc -- --noEmit
npm run lint
cd src-tauri && cargo check
cd src-tauri && cargo fmt -- --check
git diff --check
```

未执行：应用启动、前端生产构建、Rust 单元测试、iCloud 真机同步、真实 AI Provider 请求、DMG/签名/公证与视觉截图验收。

## 2. 问题清单

### P0：必须在继续处理真实 API Key 前修复

#### P0-1：API Key 仍可被 WebView 读取，且电子书内容缺少 CSP 隔离

证据：

- `src-tauri/src/commands/settings.rs:33-36` 的 `get_setting` 对敏感键直接返回 `secrets.get(&key)`，即明文密钥。
- `src-tauri/src/lib.rs:627-631` 将 `get_setting` 注册为前端可调用的 Tauri command。
- `get_all_settings` 虽然不主动加入密钥，但先无条件读取整张 `settings` 表，安全性依赖旧密钥迁移已经完全成功，也无法覆盖未来的动态密钥名称。
- `src-tauri/tauri.conf.json:23-28` 将 CSP 设置为 `null`。
- `public/foliate-js/paginator.js:242-245` 的电子书 iframe 同时允许 `allow-same-origin` 和 `allow-scripts`。
- `public/foliate-js/README.md:74-84` 明确警告：该渲染方式必须使用 CSP 阻止电子书脚本。

影响：

无 CSP 的不可信 EPUB 内容与可读取敏感设置的 IPC 命令组合后，恶意书籍脚本可能访问父窗口和 Tauri 调用面，读取 API Key、修改设置或调用其他已注册命令。设置页“不回显密钥”并没有真正建立安全边界。

推荐措施：

1. 通用 `get_setting` 对所有敏感键和敏感前缀返回稳定的 `SECRET_READ_FORBIDDEN`，绝不返回明文。
2. `get_all_settings` 在命令边界显式过滤敏感键，不能只依赖迁移逻辑。
3. 前端只可读取 `configured: boolean`、掩码末四位和凭据健康状态。
4. 密钥读取仅允许在 Rust 内部 AI Router 中发生，不提供任何“读取密钥”IPC。
5. 为 Tauri 页面配置严格 CSP，至少限制 `script-src`、`connect-src`、`frame-src`、`img-src` 和 `object-src`；在确认 EPUB 脚本被阻止前，不导入任意 HTML。
6. 对敏感写命令使用专用 command，并校验调用窗口和参数，不继续用任意键名的通用设置命令承载凭据。

### P1：会阻断功能或造成明显错误

#### P1-1：fork 没有资格使用上游作者的 iCloud container

证据：

- `src-tauri/Entitlements.plist:6-21` 固定为 Team ID `49B2V2W538`、应用 ID `49B2V2W538.com.wycstudios.quill` 和容器 `iCloud.com.wycstudios.quill`。
- `src-tauri/tauri.conf.json:5,27,62-67` 继续使用 `com.wycstudios.quill`、上游容器路径和 `embedded.provisionprofile`。
- `src-tauri/src/icloud.rs:16` 再次硬编码相同容器。
- `.github/workflows/release.yml:47-85` 依赖上游仓库 Secrets 中的证书、profile 和 Team ID；fork 不会继承这些 Secrets。
- `.gitignore:29` 排除了本地 `embedded.provisionprofile`。
- `com.apple.developer.icloud-services` 当前写成字符串 `*`，与常见的 entitlement 数组结构不一致，也应纳入签名配置修正。

影响：

即使当前 Mac 已登录 iCloud，fork 或本地开发版也通常无权解析、创建或访问上游作者的私有 ubiquity container。继续复用上游 Bundle ID 还会造成应用数据、安装身份和更新通道冲突。

推荐措施：见第 3 节 iCloud 专项方案。

#### P1-2：iCloud “可用”判断与真正启用条件互相矛盾

证据：

- `src-tauri/src/icloud.rs:25-33` 通过字符串拼接构造固定目录，没有调用 macOS ubiquity-container API。
- `src-tauri/src/icloud.rs:45-48` 只检查通用目录 `~/Library/Mobile Documents` 是否存在。
- `src-tauri/src/commands/sync.rs:174-180` 因此可能向 UI 返回 `available=true`。
- `src-tauri/src/commands/sync.rs:247-249` 在真正启用时却要求上游专属 container 目录已经存在，否则返回截图中的英文错误。

影响：

UI 可能允许用户打开同步开关，但第一次启用立即失败。未登录、iCloud Drive 未启用、entitlement 不匹配、容器未注册、profile 错误和暂时不可用都会被合并为同一句“请登录 iCloud”。新容器通常还需要由带正确 entitlement 的已签名进程调用系统 API 后才会被解析，当前代码在此之前就因为目录不存在而退出。

推荐措施：

- 使用 `NSFileManager.url(forUbiquityContainerIdentifier:)` 返回的 URL 作为唯一容器来源。
- 账号状态、Drive 状态、容器授权、可写性和暂时不可用使用结构化错误码区分。
- `sync_status` 和 `sync_enable` 共用同一个 `ICloudAvailability` 解析结果。
- 不以目录存在作为 entitlement 或同步有效性的证明。

#### P1-3：关闭正文词汇标记后，当前章节的旧标记可能残留

证据：

- `src/pages/Reader.tsx:897-910` 在开关变化时只调用 `renderer.render()`。
- `public/foliate-js/paginator.js:777-784` 的普通 `render()` 主要重新布局。
- overlay 的创建由 `public/foliate-js/paginator.js:994-1015` 和 `public/foliate-js/view.js:419-440` 的另一条生命周期触发。
- Reader 没有保存当前词汇 annotation 清单，也没有在关闭开关时逐项调用 `deleteAnnotation`。

影响：

关闭“已查询、已收藏、学习中、已掌握”开关后，当前可见章节的下划线可能继续显示，直到切章或重新打开书籍。

推荐措施：

维护当前已应用 annotation 的 `Map<CFI, MarkerType>`；设置或数据变化时计算差量，对移除项调用 `deleteAnnotation`，对新增或变色项调用 `addAnnotation`。不要把重排版当作 annotation 状态更新机制。

#### P1-4：刚完成的查词、收藏或学习状态变化不会立即更新正文标记

证据：

- `src/components/LookupPopover.tsx:164-175` 保存查询历史后没有通知 Reader。
- `src/components/LookupPopover.tsx:209-219` 收藏成功后也只更新弹窗本地状态。
- `src/pages/Reader.tsx:710-740` 只在 `create-overlay` 时重新读取数据库。

影响：

用户查完一个词或收藏后，正文通常不会立即出现相应标记；学习状态变化也可能继续显示旧颜色。

推荐措施：

保存成功后发出带 `book_id`、`cfi`、新状态的 `lookup-record-changed` / `vocab-changed` 事件。Reader 只对当前书籍和当前 overlay 做增量 add/delete/update，并在窗口重新聚焦时做一次轻量校准。

#### P1-5：已有阅读窗口忽略“回到原文”和目标侧栏参数

证据：

- `src/utils/openReaderWindow.ts:37-42` 检测到窗口已存在时只执行 `setFocus()` 后返回。
- `cfi`、`openVocab`、`openChat` 和 `chatId` 只在新建窗口 URL 中使用。
- 查询历史、生词详情和聊天详情都会调用该方法打开目标位置。

影响：

阅读窗口已打开时，从查询历史或词汇页点击“在阅读器中打开”不会跳转到对应 CFI，也不会打开指定侧栏或会话。

推荐措施：

为每本书的 reader window 发送 `reader:navigate` 事件，载荷包含唯一 navigation ID、CFI、目标面板和 chat ID；Reader 收到后先导航，再打开面板并回传完成或失败状态。新建窗口仍可用 URL 参数完成首次导航。

#### P1-6：AI Chat 使用全局流事件，多窗口或并发请求会串流

证据：

- `src/hooks/useAiChat.ts:333-369` 固定监听 `ai-stream-chunk`。
- `src-tauri/src/commands/ai.rs:528-548` 所有 Chat 都向相同事件名广播。
- 查词、解释、翻译和标题已经使用 request ID，Chat 尚未采用相同策略。

影响：

两个阅读窗口或两个并发 Chat 可能接收彼此的 token，造成内容混合、错误持久化和隐私泄漏。多 API Key 重试还会进一步放大该问题。

推荐措施：

先为 Chat 增加 `request_id`，使用 `ai-stream-chunk-{request_id}` 或面向指定窗口的 `emit_to`；同时增加取消令牌和 `ai_cancel(request_id)`。完成请求隔离后再做自动切 Key。

#### P1-7：查询流失败内容可能作为正常历史保存

证据：

- AI 后端将普通流错误作为 `delta: "Error: ..."` 发出，然后再发送 `done=true`。
- `src/components/LookupPopover.tsx:160-175` 只排除“未配置”类错误；只要两个流结束且任一有内容就保存。
- 如果一个流已有部分正文后再追加错误，结果仍可能被当作成功内容持久化。

影响：

查询历史或生词释义中可能出现错误字符串、截断内容或“部分正文 + Error”。以后点击词汇标记复用历史时会继续传播错误内容。

推荐措施：

流事件改为 `Delta | Completed | Failed` 的结构化协议。只有收到明确 `Completed` 的结果才能写查询历史或生词；失败结果只保留在当前 UI 中供重试，不进入数据库。

#### P1-8：未知扩展名会被错误地当作 EPUB

证据：

- `src-tauri/src/commands/books.rs:396-405` 只有 PDF 分支，其他所有扩展都进入 `do_import_epub`。
- 当前 TXT、MOBI、FB2 或任意未知文件不会得到准确的“不支持”错误，而是交给 EPUB 解析器。

影响：

错误信息误导，导入器无法可靠扩展，恶意或损坏容器也缺少明确的格式边界。

推荐措施：

先实现严格 allowlist、magic sniff 和统一 importer registry；不识别的文件返回 `UNSUPPORTED_FORMAT`，不得默认 EPUB。具体方案见第 5 节。

#### P1-9：删除书籍会遗留新增的查询历史

证据：

- `src-tauri/migrations/014_lookup_history.sql:4-17` 为 `lookup_records.book_id` 声明了外键级联。
- `src-tauri/src/db.rs:145-161` 生产数据库明确关闭 `PRAGMA foreign_keys`。
- `src-tauri/src/commands/books.rs:767-779` 的本地删除路径没有删除 `lookup_records`。
- `src-tauri/src/sync/merge.rs:196-235` 的远端 `book.delete` 回放路径也没有删除该表。

影响：

本机删除书籍或从其他设备同步删除后，查询记录会成为孤儿数据并持续占用数据库，列表中还可能显示未知书籍。

推荐措施：

两条删除路径都显式执行 `DELETE FROM lookup_records WHERE book_id = ?`。长期应把所有 book-child 表收敛到共享 cascade helper，并要求每次新增子表同步更新本地删除、远端 merge 和重置流程。

#### P1-10：fork 仍指向上游更新通道，可能被官方版本覆盖

证据：

- `src-tauri/tauri.conf.json:32-36` 继续使用 `yicheng47/quill` 的更新地址和上游签名公钥。
- `src/components/settings/AboutSettings.tsx`、README 和隐私文档仍指向上游项目。
- Bundle ID 也仍为上游值。

影响：

个性化安装包可能检查并安装上游官方更新，从而覆盖 fork 功能；相同 Bundle ID 还可能与原版共享或争用系统级身份。

推荐措施：

在发布任何个性化安装包前更换 Bundle ID、产品名、更新地址和更新签名密钥。若暂时没有自己的发布流程，应禁用自动更新，而不是继续消费上游更新清单。

### P2：应在下一轮稳定性改造中处理

#### P2-1：每次 overlay 创建都会读取该书全部标记数据

`src/pages/Reader.tsx:710-738` 忽略事件中的 section index，每次都读取整本书的 highlights、lookup history 和 vocab，再尝试解析所有 CFI。书籍和历史增大后，会增加数据库、CFI 解析和重绘成本。

推荐保存 section/index 信息，按当前 section 查询并缓存；数据变更使用增量事件，不反复加载整本书。

#### P2-2：点击查询词或收藏词标记没有学习交互

`src/pages/Reader.tsx:787-813` 的 `show-annotation` 只查找手动高亮。点击自动词汇标记不会打开已有释义、词汇详情或重新查询入口，与总方针中“点击标记优先复用历史释义”不一致。

推荐为 annotation 保存类型与 record ID；点击后打开轻量释义弹窗，并提供“查看详情、重新查询、进入 AI 追问”。

#### P2-3：当前复习逻辑不是真正的间隔复习

**执行状态：已完成。** 本轮已加入 SRS migration、`record_vocab_review` 命令、Again/Hard/Good/Easy UI、复习次数/间隔/到期时间/最后评分同步字段；手动改变学习状态时也会保留已有复习元数据。

- `src/components/DictionaryContent.tsx:127-128` 的“开始学习”固定安排 24 小时后复习。
- `src-tauri/src/commands/vocab.rs:215-243` 对任何 mastery 变化都增加 `review_count`。
- 后端接受任意字符串 mastery，没有枚举校验。

推荐拆分 `start_learning`、`set_mastery` 和 `record_review(rating)`；只有真实复习才增加 review count。采用成熟 SRS 算法和明确的 Again/Hard/Good/Easy 评分，不自行用固定天数模拟。

#### P2-4：词汇列表存在嵌套 button

`src/components/DictionaryContent.tsx:345-405,430-468` 和 `src/components/DictionaryPanel.tsx:76-116` 在外层 `<button>` 内继续放删除、学习等 `<button>`。这是无效 HTML，可能造成键盘焦点、点击冒泡和辅助功能行为不一致。

推荐外层改为普通行容器，单独提供“打开详情”按钮，操作按钮与主按钮保持兄弟关系。

#### P2-5：查询历史与“已查询”标记不会跨设备同步

`src-tauri/migrations/014_lookup_history.sql:1-3` 明确将 `lookup_records` 设为本地数据；收藏词和学习状态则已有 sync event、merge 和 snapshot。结果是同一本书在另一台设备上能看到收藏词，却看不到查询历史和灰色已查询标记。

短期应在 UI 和隐私说明中标注“查询历史与已查询标记仅保存在本机”。长期若需要同步，必须完整增加 lookup add/update/delete event、LWW 字段、tombstone、merge、snapshot、旧客户端兼容和限流，不能只追加日志事件。

#### P2-6：`secrets.db` 是普通 SQLite，不是系统 Keychain

`src-tauri/src/secrets.rs:24-59` 将密钥明文存入本地 SQLite。它与主数据库分离且不参与 iCloud 同步，但不等于系统级安全存储。

推荐使用 macOS Keychain；跨平台可使用成熟 keyring 库。数据库只保存 credential ID、末四位和排序。UI 文案应使用“仅保存在本机”，在完成 Keychain 迁移前避免声称“加密安全保存”。

#### P2-7：AI Provider 适配器无法正确支持可靠故障切换

**执行状态：主要完成。** 已加入共享 HTTP Client、连接/首字节/流空闲超时、请求取消、首 token 前自动换 Key、失败 Key 健康状态与 `Retry-After` 冷却。Provider 仍直接 emit UI event，完整流协议解耦保留为后续可维护性改造。

当前各 Provider 直接向 Tauri event 写 token，HTTP 错误压成字符串；没有共享连接池、明确 connect/header/idle timeout、结构化错误分类或“是否已输出首 token”的状态。自然 EOF 也可能被当作正常完成。

推荐先解耦 Provider 与 UI event，返回结构化 stream 和 `AiErrorKind`，再由统一 Router 决定是否换 Key。详见第 4 节。

#### P2-8：iCloud 启动路径可能制造“看似存在”的普通目录

同步已启用时，`src-tauri/src/lib.rs:493-507` 可直接使用 marker 中的旧绝对路径，`src-tauri/src/db.rs:145-149` 随后对该路径执行 `create_dir_all`。这会把“系统授权的 ubiquity container”与“普通文件 API 能创建的同名目录”混为一谈。

推荐 marker 只保存用户意图和 container ID；每次启动重新调用原生 API 解析，并确认 ubiquitous 和 writable。只允许在系统返回的 container URL 下创建 `logs/books/covers` 子目录。

#### P2-9：iCloud 错误未本地化，重试对配置错误没有帮助

`src/components/settings/LibrarySyncSettings.tsx:394-407` 原样显示 Rust 英文错误，并使用 `truncate`；Retry 只是重复执行同一个命令。中文界面因此出现截图中的英文，详细原因还可能被截断。

推荐后端返回错误码和脱敏详情，前端映射中英文文案并允许多行；对 entitlement/container 错误显示配置指引，对临时网络或下载错误才提供重试。

### P3：产品完整性与维护性问题

1. `src/pages/DictionaryPage.tsx:33-46`、`DictionaryPanel.tsx:21-24` 和 `DictionaryContent.tsx:52-65` 的收藏词搜索只按单词前缀匹配，但文案称可搜索单词、释义或书籍。应统一搜索字段和匹配规则。
2. `DictionaryContent.tsx:456-462` 和 `DictionaryPage.tsx:365-371` 只要存在 CFI 就固定显示 `p. 1`。CFI 不是页码，应显示章节、阅读百分比或通过当前 renderer 解析真实位置。
3. **执行状态：已完成。** 查询历史现在使用游标分页，支持搜索、按书过滤、单条删除、二次确认清空、保留期限和 CSV 导出；历史仍保持本机数据。
4. 四个正文标记开关保存在 `localStorage`，不会跨设备同步。应明确其为设备显示偏好，或移入结构化 book settings 并补齐同步协议。
5. `src/pages/SettingsPage.tsx` 是未挂到 `src/App.tsx:61-64` 的重复设置实现，实际界面使用 `SettingsModal`。后续若只改一处，多 Key 设置会产生漂移。应删除未使用实现或复用同一组 section 组件。

## 3. iCloud 同步为什么不可用

### 3.1 截图能确定什么

截图中的原文来自 `src-tauri/src/commands/sync.rs:247-249`。触发条件不是系统已经判断“用户未登录”，而是代码拼出的目标路径：

```text
~/Library/Mobile Documents/iCloud~com~wycstudios~quill/Documents
```

其 container 父目录不存在。代码把这个结果统一翻译成：

```text
iCloud is not available - sign in to iCloud and try again
```

因此截图只能证明目标 container 目录没有通过当前代码检查，不能证明 Mac 没登录 iCloud。

### 3.2 代码层面可以确定的优先原因

1. 当前 fork 沿用上游 Team ID、Bundle ID 和 container ID。
2. fork 没有上游仓库的签名证书、provisioning profile 和 CI Secrets。
3. 本地开发版只把 app-data 目录手工加了 `-dev`，并没有真正切换为独立的 Debug Bundle ID 和 Debug iCloud container。
4. 代码不调用 Apple 的 container 解析 API，而是手工拼路径并要求目录预先存在。
5. `sync_status` 只检查通用 Mobile Documents 目录，导致 UI 可能误报可用。

### 3.3 仅凭截图无法确定的环境因素

- 当前 macOS 是否登录 Apple Account。
- iCloud Drive 是否启用。
- 网络、企业 MDM 或系统权限是否限制 iCloud Drive。
- profile 是否安装、签名是否包含正确 entitlement。
- container 是否已在 Apple Developer Portal 注册并分配给 App ID。

这些因素以后需要实机诊断，但本轮按要求不执行。

### 3.4 推荐方案 A：使用自己的私有 iCloud container

适用于需要原生 ubiquity container、准备长期签名发布的 fork。

实施步骤：

1. 在自己的 Apple Developer Team 创建唯一 App ID，例如 `com.<your-domain>.reader`。
2. 创建对应 iCloud Documents container，例如 `iCloud.com.<your-domain>.reader`。
3. 为 Development 和 Distribution 分别生成匹配的 provisioning profile。
4. 替换 `tauri.conf.json`、entitlements、Rust 常量、asset scope、capabilities、开发配置和 CI Secrets 中的全部身份。
5. 将 `com.apple.developer.icloud-services` 修正为 Apple 要求的数组，并包含实际使用的 `CloudDocuments`。
6. Debug 使用独立 Bundle ID、container、entitlements 和 profile；Release 使用生产配置。
7. 使用 `URLForUbiquityContainerIdentifier` 获取真实 URL，并以小型可撤销写入探针确认可写。
8. 返回 `ACCOUNT_SIGNED_OUT`、`DRIVE_DISABLED`、`CONTAINER_UNAUTHORIZED`、`CONTAINER_NOT_REGISTERED`、`TEMPORARILY_UNAVAILABLE`、`WRITE_FAILED` 等错误码。

注意：换成自己的 container 后，fork 不能直接访问官方 Quill container 中的数据。若要迁移，只能通过有合法上游签名的版本先导出，再导入 fork。

### 3.5 推荐方案 B：用户选择 iCloud Drive 文件夹

对于个人 fork，更推荐把同步根目录改为用户在系统文件选择器中指定的 iCloud Drive 子目录，并保存 security-scoped bookmark。这样同步引擎仍可使用现有 `logs/devices/books/covers` 目录结构，但不再依赖上游作者的私有 container entitlement。

实施要点：

- 首次启用时让用户选择或创建 `iCloud Drive/Quill Reader`。
- 只保存安全作用域书签或等价授权，不硬编码绝对路径。
- 启动时重新解析书签并确认目录可写。
- UI 显示当前同步目录，并提供“更换目录”和“重新授权”。
- 目录暂时离线时进入 queue-only，不创建伪 container，也不丢本地写入。

该方案更适合个人自用和 fork 分发；若以后进入 App Store，再评估方案 A。

## 4. 多 API Key 自动切换的具体实现方案

### 4.1 设计边界

- “多 Key”首先定义为同一 Provider Profile 下的多个凭据，按优先级故障切换。
- 跨 Provider 或跨模型 fallback 会改变费用、隐私和输出行为，应作为用户显式配置的下一层路由，不默认发生。
- OAuth 与 API Key 也是不同的计费/身份通道，不能未经用户设置自动互换。
- 所有查词、段落解释、Chat、标题和翻译必须走同一个 Router，不能只改查词。

### 4.2 数据模型

非敏感配置和凭据元数据可使用以下结构：

```sql
CREATE TABLE ai_profiles (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  provider TEXT NOT NULL,
  auth_mode TEXT NOT NULL,
  base_url TEXT,
  model TEXT NOT NULL,
  temperature REAL NOT NULL,
  keep_alive TEXT,
  enabled INTEGER NOT NULL,
  priority INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE ai_credentials (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL,
  label TEXT NOT NULL,
  secret_ref TEXT NOT NULL UNIQUE,
  masked_suffix TEXT NOT NULL,
  enabled INTEGER NOT NULL,
  priority INTEGER NOT NULL,
  state TEXT NOT NULL DEFAULT 'active',
  cooldown_until INTEGER,
  last_error_kind TEXT,
  last_used_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
```

明文密钥存入 macOS Keychain，建议 account 名为 `quill.ai.<credential-id>`。SQLite 只保存 credential ID、Keychain reference、末四位、排序和健康状态。

若首期暂时继续使用 `secrets.db`，只能通过专用命令保存 `ai_api_key/<credential-id>`，并把敏感键识别改为前缀判断。此方案仍是明文 SQLite，只能作为过渡，不能宣称等同 Keychain。

### 4.3 统一后端 Router

新增 `src-tauri/src/ai/router.rs`，由 Tauri 管理一个共享 `AiRouter`：

```rust
enum AiErrorKind {
    Auth,
    Permission,
    RateLimit,
    Quota,
    Timeout,
    Network,
    Provider5xx,
    BadRequest,
    ModelNotFound,
    ContentPolicy,
    Protocol,
    Cancelled,
}

enum AiEvent {
    Delta(String),
    Retrying { credential_id: String, reason: AiErrorKind },
    Completed,
    Failed(AiError),
}
```

Router 职责：

1. 每次请求拍摄一份按 profile priority、credential priority 排序的候选快照。
2. 维护 attempted set，每个 Key 一次请求最多尝试一次，整次请求最多遍历一轮。
3. 复用共享 `reqwest::Client`，设置 connect、响应头和流 idle timeout。
4. 记录该请求是否已向 UI 发出首个 token。
5. 仅在零 token 时自动切 Key；已有输出后报“部分响应中断”，不静默重启，避免重复内容。
6. 成功后清除暂态错误；冷却到期后原第一优先级 Key 自动恢复。
7. 所有日志只记录 credential ID 和错误类别，不记录密钥、Authorization header 或完整响应体。

Provider adapter 不再直接接收 `AppHandle` 和 event name，而是返回结构化 stream；由 Router 统一向对应 request/window 发事件。

### 4.4 自动切换规则

| 情况 | 是否自动换 Key | 处理方式 |
|---|---:|---|
| 401 / key revoked | 是 | 标记当前凭据无效，切下一个 |
| 403 | 视错误码 | Key 权限/区域限制可切；内容策略或请求级禁止不切 |
| 402 / insufficient quota | 是 | 标记配额问题，切下一个 |
| 429 rate limit | 是 | 读取 `Retry-After`，当前 Key 进入冷却 |
| DNS、TLS、连接失败、超时 | 零 token 时是 | 记录暂态失败，最多遍历一轮 |
| 500/502/503/504 | 零 token 时是 | 短冷却后切换，避免无限轰击故障端点 |
| 400/404/413/422 | 否 | 多为请求、模型或上下文配置错误，换 Key 无意义 |
| 正常 refusal/content policy | 否 | 这是模型响应，不是凭据故障 |
| SSE/JSON 协议错误或缺少完成标记 | 零 token 时是 | 已有 token 时返回部分流中断 |
| 用户取消 | 否 | 立即停止，不尝试下一 Key |

建议默认 connect timeout 10 秒、响应头 30 秒、流 idle 45 秒；可配置但不要对长对话设置过短的总时长。

### 4.5 请求隔离与取消

- Chat 增加 `request_id`，事件使用 `ai-stream-chunk-{request_id}`。
- 优先使用 `emit_to(window_label, ...)`，避免全局广播。
- 后端保存 `request_id -> CancellationToken`，提供 `ai_cancel(request_id)`。
- UI 关闭弹窗、切书或点击停止时主动取消。
- Retry event 可以显示“第 1 个凭据限流，正在切换”，但不暴露完整 Key。

### 4.6 设置界面

`AiSettings` 改为 Provider Profile 与密钥列表：

- 每个 Key 显示标签、末四位、启用状态、优先级、冷却或无效状态。
- 支持新增、替换、删除、启用、禁用、上移、下移和单独连接测试。
- 新增时明文只提交一次，列表 API 永不返回明文。
- “已配置”按启用且非空的 credential 数量计算，不能只判断数据库中是否存在空字符串。
- 连接测试显示目标 Provider、Base URL、Model 和脱敏错误。

建议使用专用命令：

```text
ai_list_profiles
ai_upsert_profile
ai_list_credentials
ai_add_credential
ai_replace_credential
ai_set_credential_enabled
ai_reorder_credentials
ai_delete_credential
ai_test_credential
```

### 4.7 旧配置迁移

1. 用当前 `ai_provider/model/base_url/temperature/keep_alive/auth_mode` 创建默认 profile。
2. 旧 `ai_api_key` 非空时，写入 Keychain 并创建 priority 0 credential。
3. Keychain 写入和 metadata transaction 都成功后，才删除旧键。
4. 迁移失败时保留旧配置，并显示可恢复错误，不能静默丢 Key。
5. 迁移完成后永久禁止通用设置接口读取敏感键。

### 4.8 计划中的验收场景

本轮没有运行这些验证，后续实现时至少覆盖：

- 第一把 Key 返回 401，第二把成功。
- 第一把返回 429 且包含 `Retry-After`，第二把成功，冷却到期后第一把恢复。
- 400 或模型不存在时不切 Key。
- 首 token 后断流不自动切换，UI 标记部分响应。
- 两个阅读窗口并发 Chat 不串流。
- 一次请求不会重复尝试同一 Key，也不会无限循环。
- 失败查询和失败 Chat 不进入持久化历史。
- 前端任何列表或通用设置接口都无法取回明文密钥。

## 5. TXT 及其他常见格式支持的具体实现方案

### 5.1 当前链路为什么不能只改文件选择器

当前支持格式被分散在多层：

- `src/hooks/useBooks.ts:5-22,87-95`：TypeScript 类型与文件选择器只允许 EPUB/PDF。
- `src/pages/Home.tsx:210-230`：拖放只接收 `.epub` 和 `.pdf`。
- `src-tauri/tauri.conf.json:50-60`：系统文件关联只有 EPUB/PDF。
- `src-tauri/src/commands/books.rs:390-407`：后端只有 PDF 和默认 EPUB 两条 importer。
- `src/pages/Reader.tsx:502-507`：PDF 外所有文件都被重新命名成 `book.epub`。
- MCP、同步事件、快照、删除、i18n 和导入错误也依赖当前两种格式。

因此只给 picker 增加 `txt` 会让 TXT 继续进入 EPUB parser，并不会得到可用阅读体验。

### 5.2 推荐总体策略

第一阶段将 TXT、Markdown 和经过清洗的 HTML 在导入时转换为稳定的内部 EPUB，而不是在 Reader 临时拼 DOM。这样可以直接复用现有：

- Foliate 分页和滚动排版。
- CFI 位置、恢复进度和回到原文。
- 选词、查词、手动高亮和词汇标记。
- 目录、字体、主题和 iCloud 文件同步。

生成 EPUB 必须持久化，不能每次打开或在另一设备重新生成，否则章节切分变化会使历史 CFI 失效。

### 5.3 数据模型

保留现有 `format` 作为兼容字段，并新增来源与渲染格式：

```sql
ALTER TABLE books ADD COLUMN source_format TEXT;
ALTER TABLE books ADD COLUMN render_format TEXT;
ALTER TABLE books ADD COLUMN source_file_path TEXT;
ALTER TABLE books ADD COLUMN source_sha256 TEXT;
ALTER TABLE books ADD COLUMN conversion_version INTEGER;
```

示例：

```text
TXT 导入：source_format=txt, render_format=epub, format=epub
原文件：sources/<book-id>.txt
生成物：books/<book-id>.epub
```

老数据回填 `source_format=render_format=format`。`conversion_version` 固化章节切分和 HTML 生成规则；转换器升级不得自动重写已有书籍，以免破坏 CFI。

若原文件也跨设备保留，必须让同步的目录移动、删除书籍、snapshot 和 event 一起处理 `sources/`。若原文件仅本机保存，则使用 local-only metadata，不能把其他设备无法解析的 source path 写进同步 Book row。

### 5.4 统一格式识别与 importer registry

建议新增：

```text
src-tauri/src/import/mod.rs
src-tauri/src/import/detect.rs
src-tauri/src/import/text.rs
src-tauri/src/import/epub_writer.rs
```

Tauri `import_book` 和 MCP `add_book` 共用同一个 dispatcher。识别顺序以 magic/container 内容为主，扩展名只作提示：

| 格式 | 识别方式 |
|---|---|
| PDF | `%PDF-` magic |
| EPUB | ZIP + `mimetype=application/epub+zip` + container.xml |
| TXT/Markdown | 非二进制文本 + 编码检测 + 扩展提示 |
| HTML | HTML root/doctype + 扩展提示，进入 sanitizer |
| MOBI/AZW3 | `BOOKMOBI` magic |
| FB2 | XML root / FictionBook namespace |
| FBZ | ZIP 中的 FB2 文档 |
| CBZ | ZIP 中以图片为主的有序条目 |

不支持或损坏文件返回结构化错误：`UNSUPPORTED_FORMAT`、`ENCODING_UNCERTAIN`、`INVALID_CONTAINER`、`CONVERSION_FAILED`，不再默认 EPUB。

导入使用临时目录：检测、解析、生成、元数据回读全部成功后，再 atomic rename 到 `books/` 并写数据库和 sync event。失败时清理临时文件，避免数据库记录或孤儿文件只成功一半。

### 5.5 TXT 转 EPUB 的具体步骤

1. BOM 优先，再使用成熟的 `chardetng + encoding_rs` 检测 UTF-8、GB18030/GBK、Windows-1252 等常见编码。
2. 置信度不足时让用户选择编码，不能用 lossy decode 静默替换字符。
3. 将 CRLF/CR 统一为 LF，保留空行与段落边界。
4. 使用成熟 EPUB writer 生成 XHTML、nav、TOC、metadata 和唯一 identifier；正文必须 HTML/XML escape。
5. 章节优先识别 `Chapter`、`CHAPTER`、`第...章` 等独立标题行；无标题时按段落边界和稳定字符阈值分块。
6. 输出文件固定命名 `chapter-0001.xhtml`、`chapter-0002.xhtml`，确保同一 conversion version 下结果确定。
7. 标题默认取文件名，作者为 Unknown，导入后允许编辑。
8. 大文件使用采样检测与流式解码，并限制文件大小、章节数和单章大小，避免一次性内存膨胀。
9. 生成完成后用现有 EPUB parser 回读一次元数据，成功后再提交数据库事务。

### 5.6 Markdown 与 HTML

- Markdown 使用成熟 CommonMark parser，标题生成 TOC；默认禁用 raw HTML，或对其进行严格清洗。
- HTML 使用成熟 parser/sanitizer，移除 script、iframe、object、事件属性、`javascript:` URL 和未许可远程资源。
- 本地图片需要解析相对路径、检查 MIME/大小并打包进 EPUB。
- 在 CSP 和 sanitizer 完成前只上线纯文本 TXT，不开放任意 HTML 导入。

### 5.7 Foliate 已有格式能力如何利用

当前 `public/foliate-js/view.js:79-122` 已包含 MOBI/KF8/AZW3、FB2/FBZ 和 CBZ parser，但 Quill 尚未完整打通：

- Reader 把所有非 PDF 文件命名为 `book.epub`，会破坏依赖文件名判断的 FBZ/CBZ 分发。
- 前端 `Book.format` 仍锁死为 EPUB/PDF。
- 当前 UI 只有 `format === pdf` 分支，无法表达 CBZ 这类 fixed-layout 但无文本层的格式。

建议引入能力模型，而不是继续堆格式判断：

```ts
interface ReaderCapabilities {
  reflowable: boolean;
  fixedLayout: boolean;
  hasTextLayer: boolean;
  supportsSelection: boolean;
  supportsAnnotations: boolean;
  supportsFontStyles: boolean;
  supportsZoom: boolean;
  supportsContinuousScroll: boolean;
}
```

CBZ 可以阅读图片，但没有文本层时不能选词、查词或自动高亮；OCR 应作为独立功能。CBR 需要成熟 RAR/libarchive 能力，后置处理。MOBI/AZW3/FB2 可先复用 Foliate parser，但必须保留真实文件名/MIME，并在后续验证位置标识跨版本稳定性；不要自行实现 MOBI 解压算法。

### 5.8 必须同步修改的入口

1. `Book` TypeScript 类型与后端格式枚举。
2. 文件选择器、拖放过滤、Dock/open-file 事件和 Tauri file associations。
3. 后端 importer、MCP schema 和错误文案。
4. Reader 构造 `File` 时的真实扩展和 MIME map。
5. 数据库迁移、同步 event/merge/snapshot 和旧客户端兼容。
6. `sync_enable/sync_disable` 对 `sources/` 的复制和移动。
7. 删除书籍、重置数据和失败回滚对 source/render artifact 的清理。
8. i18n、README、隐私说明和支持格式说明。

### 5.9 推荐分期

1. 先实现 strict detector、allowlist、统一 importer registry 和结构化错误。
2. 增加 source/render format 与 artifact 生命周期。
3. 完成 TXT 转内部 EPUB，包括编码选择、稳定章节、TOC 和源文件保留。
4. CSP 与 sanitizer 完成后加入 Markdown/HTML。
5. 打通 Foliate 已有 MOBI/AZW3、FB2/FBZ、CBZ 能力，并按 capability 控制 UI。
6. 以后再评估 CBR、DOCX、RTF；办公文档应由成熟转换器转 EPUB，不直接混入 Reader 内核。

### 5.10 计划中的验收场景

本轮没有运行这些验证，后续实现时至少覆盖：

- UTF-8、UTF-8 BOM、GB18030、Windows-1252 TXT。
- 无章节、超长章节、中英文混合和超大文本。
- 转换前后段落、引号、尖括号和特殊字符不丢失。
- 同一 conversion version 重复转换产生稳定章节与 CFI。
- 损坏文件、未知扩展和二进制伪装文本得到准确错误。
- 导入失败没有数据库记录、临时文件或孤儿 source。
- 恶意 HTML 不能执行脚本或访问 Tauri IPC。
- 同步后的设备直接使用同一个生成 EPUB，不重新转换。
- CBZ 不显示选词和学习工具；MOBI/FB2/FBZ 使用真实扩展被正确分发。

## 6. 当前版本与原项目的展示方案

### 6.1 当前问题

当前“关于”页无法帮助用户判断自己正在使用哪个发行版本：

- `src/components/settings/AboutSettings.tsx:8-9` 的 GitHub 和文档链接都固定指向原作者仓库 `yicheng47/quill`。
- 页面只显示 Tauri 版本 `v1.2.10`，它与当前 fork 的上游基线相同，无法区分“原版”与“个性化版本”。
- 页面底部只显示 `© 2026 wyc studios`，没有说明当前 fork 的维护者及其与原项目的关系。
- `src-tauri/tauri.conf.json:32-36` 的自动更新仍指向原作者 Releases。用户即使从当前 fork 安装，也可能被引导或更新回原版本。

因此，当前页面虽然保留了原作者信息，却没有为当前版本建立清晰入口。用户查看源码、下载更新、反馈问题或阅读文档时，容易进入错误仓库。

### 6.2 展示原则

采用“当前版本为主、原项目署名为辅”的信息结构：

1. 用户操作目标默认指向当前版本，包括仓库、Releases、Issues、文档和更新检查。
2. 原项目使用独立区域清晰展示，包括原项目名称、原作者、上游仓库和许可证。
3. 不把两个都叫“GitHub”的链接并列显示；每个链接必须说明它代表当前版本还是原项目。
4. 不暗示原作者负责当前 fork 的功能、问题或发布。
5. 不删除或弱化原作者版权与 MIT 许可证信息。

### 6.3 推荐页面结构

建议将应用展示名暂定为 `Quill Personal`。最终名称可在品牌调整阶段确认，但必须与原版具有可辨识差异。

```text
Quill Personal
面向英语学习的个性化阅读器

当前版本  v1.3.0
基于 Quill v1.2.10
macOS · arm64

当前版本
GitHub 仓库              KlaraGraff/quill
版本发布与更新            Releases
问题反馈                  Issues
当前版本文档              README

原始项目
原项目 Quill             yicheng47/quill
原作者                    wyc studios
开源许可证                MIT

基于 Quill 开源项目开发
原项目版权归原作者所有
```

“当前版本”区应排在前面，因为这是用户寻找下载、文档和问题反馈入口时的默认目标。“原始项目”区紧随其后，以正式署名形式说明项目来源，而不是放在难以发现的页脚。

### 6.4 链接归属

| 页面项目 | 推荐目标 |
|---|---|
| 当前版本 GitHub | `https://github.com/KlaraGraff/quill` |
| 当前版本 Releases | `https://github.com/KlaraGraff/quill/releases` |
| 当前版本问题反馈 | `https://github.com/KlaraGraff/quill/issues` |
| 当前版本文档 | `https://github.com/KlaraGraff/quill#readme`，以后有独立文档站再替换 |
| 原始项目 | `https://github.com/yicheng47/quill` |
| 原项目许可证 | 当前仓库内的 `LICENSE`，并保留原版权声明 |

若当前 fork 尚未开放 Releases 或 Issues，对应入口应隐藏或标记“暂未开放”，不能退回指向上游同名入口。当前版本的问题不应默认提交给原作者。

### 6.5 版本信息设计

关于页至少区分以下三项：

```text
当前版本：1.3.0
上游基线：Quill 1.2.10
构建提交：5595d37
```

推荐规则：

- 当前版本使用 fork 自己的 SemVer，不继续与上游版本共用同一个可见版本号。
- 上游基线作为单独 metadata，例如 `UPSTREAM_VERSION=1.2.10`，用于说明当前版本基于哪个 Quill 版本。
- 构建提交取短 SHA，放在可展开的“构建信息”或“复制诊断信息”中，不占据主视觉层级。
- 发布构建通过 CI 注入 commit、build date 和 channel；开发构建明确显示 `Development`，避免被误认成正式发行版。
- 不在运行时解析本地 Git remote；安装包应使用构建时固化、可复现的版本元数据。

### 6.6 署名与版权文案

推荐中文声明：

> 本版本基于 Quill 开源项目开发，由当前版本维护者独立维护，与原项目作者不存在官方隶属关系。原项目版权归原作者所有，并继续遵循 MIT License。

推荐英文声明：

> This edition is based on the open-source Quill project and is independently maintained. It is not an official release of the original project. Original Quill copyright remains with its authors and is licensed under the MIT License.

页脚可简化为：

```text
Original Quill © 2026 wyc studios
Personal edition maintained by KlaraGraff
MIT License
```

不建议只把原作者版权替换成当前维护者，也不建议只保留原作者版权而隐藏 fork 的维护归属。

### 6.7 更新通道必须与页面身份一致

关于页改为当前仓库后，自动更新也必须同步调整：

1. 更新 endpoint 指向当前 fork 的 Releases。
2. 使用当前 fork 自己的 updater 签名密钥和公钥。
3. 当前版本、Bundle ID、产品名和更新清单保持一致。
4. 在自己的稳定发布通道准备完成前，禁用自动更新；不能继续使用上游更新清单。
5. 用户可从“关于”页看到当前更新通道，例如 `Stable`、`Beta` 或 `Development`。

否则页面虽然显示当前 fork，应用仍可能下载上游官方版本并覆盖个性化功能。

### 6.8 实施建议

1. 将当前仓库、上游仓库、Issues、Releases、文档和版本 metadata 收敛到一个 build metadata 模块，不继续散落硬编码。
2. 修改 `AboutSettings`，增加“当前版本”和“原始项目”两个无歧义的分组。
3. 同步维护 `en.json` 和 `zh.json`，避免只在中文界面加入署名说明。
4. 提供“复制版本信息”，输出当前版本、上游基线、commit、平台、架构和更新通道，不包含用户路径或凭据。
5. 同步更新 README、隐私说明、安装包产品名、Bundle ID、Updater 和发布文档。
6. 保留仓库现有 `LICENSE` 中的上游版权，并在新增源文件或分发材料中遵守 MIT 要求。

### 6.9 计划中的验收场景

本轮不执行这些验证，后续实施时至少确认：

- 用户无需打开外部链接即可判断正在使用当前 fork，而不是原版 Quill。
- 当前仓库、Releases、Issues、文档和原项目链接分别打开正确目标。
- 页面同时清晰显示当前维护者和原作者，不产生官方隶属关系误解。
- 当前版本号、上游基线、commit 和安装包实际 metadata 一致。
- 正式版只接收当前 fork 签名的更新；未配置 fork 更新通道时不会安装上游版本。
- 中英文界面表达一致，长仓库名和版本信息不会溢出。

## 7. 推荐实施顺序

### 阶段 1：安全边界

- 禁止通用设置接口读取任何 secret。
- 配置 CSP，收紧电子书 iframe 内容能力。
- 将凭据从普通 SQLite 迁往 Keychain。
- Chat 使用 request ID、定向事件和取消机制。

### 阶段 2：现有功能正确性

- 修复词汇标记差量刷新和点击交互。
- 修复已有阅读窗口的 CFI/面板导航。
- 只有成功 AI 结果才写历史。
- 删除书籍时清理 lookup history。
- 为查询历史增加分页、删除、清空和本地-only 说明。

### 阶段 3：版本身份与发布入口

- 将关于页改为“当前版本”和“原始项目”双分组。
- 建立 fork 自己的版本号、上游基线和构建 metadata。
- 对齐产品名、Bundle ID、仓库链接、Issues、文档和自动更新通道。
- 保留原作者版权、上游仓库和 MIT License 署名。

### 阶段 4：iCloud

- 在“自己的 container”与“用户选择 iCloud Drive 文件夹”中确定一种身份方案。
- 统一可用性解析、原生 URL 获取、结构化错误和本地化。
- 修复 Debug/Release、Bundle ID、更新通道和签名配置。

### 阶段 5：多 Key

- Provider adapter 与 UI event 解耦。
- 建立 Profile/Credential schema、Keychain、Router 和错误分类。
- 所有五类 AI 功能统一接入，再开放自动切换 UI。

### 阶段 6：格式扩展

- 先 strict detector，再 TXT 转 EPUB。
- CSP/sanitizer 完成后增加 Markdown/HTML。
- 最后打通 MOBI/AZW3、FB2/FBZ 和 CBZ capability。

## 8. 本轮交付边界

本文件保留原始静态审查结论和推荐实施方案，并在“执行状态”中记录后续源码实现。没有启动应用、执行 iCloud 实机同步、调用真实 AI API、构建安装包、签名或公证。真实安装包中的表现、Apple 账号状态、同步授权持久性和 Provider failover 行为仍须按后续验收计划确认。
