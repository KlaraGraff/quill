# 个性化阅读器整改状态

> 更新日期：2026-07-12
> 分支：`feat/personalized-reader`
> 对照：`personalized-reader-current-changes-static-audit-2026-07-12.md`、`personalized-reader-implementation-static-review-2026-07-12.md`
> 说明：原审查报告记录的是当时的静态快照，保留不改。本文件记录其后已实施的整改与验证状态。

## 已完成整改

- 数据库 migration 009 由 runner 统一管理事务，并在事务外临时关闭外键检查；全新数据库、v8 升级、失败回滚和幂等迁移均有回归测试。
- 同步事件使用连续 peer watermark；同一 peer 的首个失败事件会阻止其后续事件越过水位。日志重开后会续接单调 ULID，并限制远端事件与 ULID 时间不能超出允许的未来时钟偏差。
- 同步输入已校验设备身份、事件 schema、实体 ID、blob 路径和文件扩展名；日志与 snapshot 读取也具备大小、单行和事件数量限制。
- AI 流改为增量 SSE 字节解析，兼容 UTF-8 跨分块、CRLF、`data:`/`data: `、多行 data 与 EOF 残留；标题截断改为 UTF-8 安全边界。
- AI 多密钥状态区分未配置、已停用、全部失效、冷却中和无可用密钥；流式请求会把这些安全状态码传到界面，聊天、查词、解释和翻译均显示双语可读文案及设置入口。
- 默认 Base URL 按 Provider 决定：OpenAI、Anthropic、Ollama 与 custom 的行为彼此隔离；custom 未配置 URL 会明确报错。
- 阅读器格式能力模型已区分选词、手动标注、自动词汇标记、CFI 导航、重排、双页、连续滚动和缩放。PDF 支持选词与手动高亮但关闭自动词汇标记；不支持标注的格式不显示高亮菜单。
- TXT/Markdown/HTML 导入保留原文件、来源格式、SHA-256 和转换版本，使用成熟 Markdown parser 与 HTML sanitizer；转换 EPUB 使用确定性来源 hash 标识，导入失败会清理临时和最终文件。
- 词汇复习已使用 FSRS，并把完整复习状态纳入导入、导出、同步事件和 snapshot。词汇支持 JSON/CSV 预览导入、冲突策略、dry-run 与批量管理。
- 关于页展示当前版本、上游基线、构建 commit、构建时间、发布通道和可复制版本信息；应用数据与 Keychain 均可从上游及早期拼写错误的 bundle ID 迁移。
- 跨进程同步切换使用共享/排他文件锁，MCP 写入与桌面端同步启停不会竞争。

## 本轮补充修复

- AI 错误状态在所有入口统一解析，避免内部错误码直接呈现给用户。
- 流式弹窗在启动调用立即失败时会释放自身事件监听器，且不会误解绑后续新请求。
- 手动高亮入口仅在当前格式具备标注能力时启用。
- 严格 Rust lint 中发现的无用常量、复杂元组状态和文件锁打开语义已收敛；必要的 Tauri/provider 参数边界保留局部 lint 注解。

## 已执行验证

`npm run lint`、`npm run build`、`./node_modules/.bin/tsc --noEmit`、`cargo fmt --check`、`cargo check`、`cargo test`、`cargo clippy -- -D warnings` 和 `git diff --check` 均已通过。

Rust 测试为 `273 passed, 1 ignored`，MCP 集成测试为 `2 passed`。被忽略的测试是需要用户选定 iCloud 目录的手动烟雾测试。

## 仍需实机验收

下列不是静态代码缺陷，不能由本轮检查替代：真实 Keychain 持久化、iCloud 双设备/离线恢复、各 AI Provider 的真实流与故障切换、所有书籍格式的 Foliate 渲染、签名安装包/公证，以及 App Sandbox 场景下的 security-scoped bookmark。
