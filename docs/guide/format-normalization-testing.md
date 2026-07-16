# 格式规整管线 — 交接测试文档

> **状态：** Phase 1（管线骨架）与 Phase 2 路线 A（Calibre 转换器）的**代码实现已完成**，`cargo test`（466 通过）/ `cargo clippy` / `npx tsc` / `npm run lint` 全绿。**运行时/GUI 验收未做**（实现会话无图形环境），本文档是给测试者的完整验收清单。
>
> 计划与设计背景见 [`docs/impls/format-normalization-pipeline.md`](../impls/format-normalization-pipeline.md)。

## 1. 被测行为一句话总结

导入 MOBI / AZW / AZW3 时，若本机检测到 Calibre 的 `ebook-convert`，书籍会以 `render_format=epub` 入库并在后台转换为 EPUB（本地产物，不进 iCloud）；转换完成后阅读器直接读本地 EPUB，从而获得选中/查词/标注等完整能力。检测不到 Calibre 则维持原有的 foliate 原生只读路径，行为无任何变化。

## 2. 前置条件

- macOS 真机，能构建并运行 app（`npm run tauri dev` 或打包产物）。
- **Calibre**（含命令行工具 `ebook-convert`）。标准安装到 `/Applications/calibre.app` 即可，探测顺序：`PATH` → `/opt/homebrew/bin` → `/usr/local/bin` → `/Applications/calibre.app/Contents/MacOS` → `~/Applications/calibre.app/Contents/MacOS`。
- 测试文件（见原始审计 `docs/reviews/test-files-format-compatibility-audit-2026-07-16.md`）：
  - `西学三书.azw3`（KF8 v8，HUFF/CDIC 压缩 — 较难样本）
  - `重读20世纪中国小说.mobi`（MOBI6 v6）
  - 任一普通 EPUB / PDF / TXT（回归用）
- 观察后台状态的两个位置：
  - 库页书封覆盖层（转圈 = pending/preparing，感叹号 = failed，点击失败书 = 重试）。
  - 产物目录：`~/Library/Application Support/<bundle-id>/prepared/{book_id}.converted.v1.epub`。

## 3. 基线（已通过，改动后回归时重跑）

```bash
cd src-tauri && cargo test --lib && cargo clippy --all-targets
cd .. && npx tsc --noEmit && npm run lint
```

## 4. 验收用例

### T1 · 有 Calibre：AZW3 导入并转换（核心路径）
1. 确认 Calibre 已安装，导入 `西学三书.azw3`。
2. **预期：** 书立即出现在库中，封面覆盖"准备中"转圈；`prepared/` 下先出现 `.{id}.converted.v1.epub.tmp.epub` 临时件，完成后变成 `{id}.converted.v1.epub`，覆盖层消失。
3. 打开书。**预期：** 以 EPUB 渲染；能**划词选中**，AI 查词/翻译/解释可用；能创建手动标注与生词标记；进度/CFI 正常保存。
4. 检查 DB（可选）：`render_format='epub'`、`source_format='azw3'`、`preparation_state='ready'`、`file_path` 仍指向 `books/...azw3`（同步列不变，重定向发生在读取层）。

### T2 · 有 Calibre：MOBI6 导入并转换
同 T1，用 `重读20世纪中国小说.mobi`。KF8 与 MOBI6 两代格式都要过。

### T3 · 无 Calibre：优雅降级（不回归）
1. 临时让探测失败：`sudo mv /Applications/calibre.app /Applications/calibre.app.bak`（并确认 PATH 里无 `ebook-convert`），重启 app。
2. 导入任一 mobi/azw3。**预期：** 无覆盖层，行为与改动前完全一致——原生 foliate 只读阅读，`render_format` 保持 `mobi`/`azw3`，不出现 pending 状态、不转圈。
3. 恢复 Calibre。注意：**已按原生导入的书不会追溯转换**（设计如此，重新导入才走管线）。

### T4 · 转换失败 → 重试闭环
1. 构造失败：装有 Calibre 时导入一本 azw3（入库为 pending），**在转换完成前**退出 app，把 Calibre 改名（模拟卸载），再启动 app。
2. **预期：** 启动恢复（resume）自动重跑转换 → 失败，覆盖层变感叹号"准备失败"；DB `preparation_error='CALIBRE_MISSING'`。
3. 恢复 Calibre，**点击失败的书**。**预期：** 触发 `retry_book_conversion`，重新探测到 Calibre，转换成功变 ready（探测是每次任务重新做的，无需重启）。
4. 另一条失败路径（可选）：把一个损坏文件改后缀为 `.azw3` 导入。**预期：** 转换失败，`preparation_error` 为 `CONVERSION_TOOL_FAILED:<ebook-convert stderr 末行>`，UI 显示失败可重试，**不 crash、不无限转圈**。

### T5 · 转换中杀进程：崩溃恢复
1. 导入大文件 azw3，在覆盖层还在转圈时强杀 app（`kill -9`）。
2. 重启。**预期：** 启动时 `resume_interrupted_book_conversions` 把 `preparing` 复位为 `pending` 并自动重跑，最终 ready；`prepared/` 不残留 `.tmp.epub` 半成品被当成品用（临时件先写、原子 rename 后才发布）。

### T6 · Reader 端重试分派
1. 用 T4 方法制造一本 failed 的转换书，直接通过 URL/历史打开它的阅读器窗口（或在它 failed 时点开——正常入口被覆盖层挡住，可跳过此用例若无法直达）。
2. **预期：** 阅读器错误页的"重试"按钮触发的是 `retry_book_conversion`（转换书）而非 `retry_text_book_preparation`；text 书的重试行为不回归。

### T7 · 双设备 iCloud 同步（最重要的架构验收）
1. 设备 A（有 Calibre）导入 azw3，等 ready，确认可读。
2. 等 iCloud 同步到设备 B。**预期（B 有 Calibre）：** 书到达时状态为 **pending**（不是 ready——`preparation_state` 是本地推导列），B 自行转换后可读；A、B 的产物各自独立生成。
3. **预期（B 无 Calibre）：** B 上该书 pending → 转换失败 `CALIBRE_MISSING` → 显示失败+可重试；**不 crash**。装上 Calibre 后点击重试即可读。
4. **产物不进 iCloud：** 检查 iCloud 目录（`~/Library/Mobile Documents/.../quill/books|covers`）**不存在**任何 `*.converted*.epub`；只有源 `.azw3` 同步。

### T8 · ready 但产物丢失：自愈
1. 一本 ready 的转换书，退出 app，手动删除 `prepared/{id}.converted.v1.epub`。
2. 启动 app 进入库页。**预期：** 该书自动回到"准备中"覆盖层（`resolve_book_paths` 守卫式把 ready 翻回 pending 并当场重新调度），转换完成后恢复可读。绝不出现"点开后 foliate 把 azw3 当 EPUB 解析报错"。

### T9 · 回归清单
- EPUB / PDF / TXT / Markdown / HTML 导入与阅读完全不变。
- TXT 管线（text_prepare）的 pending→ready、失败重试不变。
- FB2 / FBZ / CBZ 仍走原生路径（不进转换管线）。
- 改动前已入库的原生 mobi/azw3 书照常可读，不被误标 pending。
- 书籍删除：删除一本转换书后，`prepared/` 的产物残留与否（当前**不清理**，属已知小遗留，见 §5）。

### T10 · Phase 0 遗留验收（顺带）
Layer A/B（阅读器 init 超时自愈）当时也没条件真机复现：拿一本带失效 saved CFI 的 EPUB 验证打开不再永久卡死（`useFoliateView` 清位置重试 + paginator 15s iframe 超时 fallback）。

## 5. 已知限制（设计内，测试时勿当 bug 报）

1. **MCP 导入不即时调度**：MCP 是独立进程无 AppHandle，经 MCP 导入的转换书（与 text 书一致）要等下次 app 启动或 sync initial tick 才开始转换。
2. **无追溯转换**：装 Calibre 之前导入的原生 mobi/azw3 书不会自动转换，需删除重导。
3. **live sync 事件的调度延迟**：app 运行中经 sync 到达的 pending 转换书，等下次启动/initial tick 才转换（与 text 管线行为一致）。
4. **删除书籍不清理 `prepared/` 产物**（text 管线同样如此）；产物按 `CONVERSION_VERSION` 版本化，旧版本会在下次转换时清扫。
5. **超时上限 600s**：单本转换超过 10 分钟按失败处理（`CONVERSION_TIMEOUT`），可重试。

## 6. 实现位置速查

| 内容 | 位置 |
|---|---|
| 状态机 + Calibre 转换器 | `src-tauri/src/commands/books/convert_prepare.rs` |
| 导入分支（探测 + 降级） | `src-tauri/src/commands/books/import.rs` (`do_import_native`) |
| 读取层重定向 + 自愈 | `src-tauri/src/commands/books/query.rs` (`resolve_book_paths`) |
| sync 状态推导（跨设备 pending） | `src-tauri/src/sync/merge.rs`、`src-tauri/src/sync/snapshot/apply.rs` |
| 启动恢复/调度接线 | `src-tauri/src/lib.rs` |
| 前端共享判断/重试 | `src/hooks/useBooks.ts`、`BookGrid.tsx`、`BookList.tsx`、`Reader.tsx` |
