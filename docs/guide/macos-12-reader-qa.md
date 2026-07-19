# macOS 12 Reader 兼容性真机 QA

> 状态：**任务书已就绪，自动化 fixture 已通过；真实 Monterey 与集成后的 packaged `.app` 证据待执行。**
>
> 实施方案：[`docs/impls/macos-12-reader-webkit-compatibility.md`](../impls/macos-12-reader-webkit-compatibility.md)

本文用于验证同一套 Lantern Reader 在现代 WebKit 选择 ZIP 原生流和 PDF.js modern，在 macOS 12 /
Safari 15.1 选择 ZIP 纯 JavaScript 流式 fallback 和 PDF.js legacy。**开发服务器、Node、当前 macOS
或 Playwright 的结果不能替代 Monterey packaged `.app`。**

## 1. 发布阻断规则

以下任一项缺证据即不得宣称“支持 macOS 12”：

1. 当前 macOS 与真实 Apple Silicon Monterey 12.x 测试的是同一份候选 artifact，About commit 与
   artifact/tag 一致；
2. Monterey 的 Safari/WKWebView 能力快照符合旧路径，并成功打开 EPUB/PDF；
3. modern/legacy 各自只加载匹配的一组 PDF main/worker，关闭、重开不累积 Worker；
4. 《谈美》、Hayek PDF、searchable OCR PDF 以及 §8 扩展样本矩阵通过；
5. Library、Reader、Settings 的布局和交互通过，不以“文件能打开”代替 UI 验收；
6. 性能、安装后体积和 DMG 增量满足实施方案 §8；
7. Console 不出现 `false is not a constructor`、`Unexpected token '{'`、
   `Promise.withResolvers` 或其他兼容错误。

失败项记为 **FAIL**，无法执行或证据缺失记为 **BLOCKED**；二者都不是 PASS。

## 2. 样本与隐私边界

仓库内 synthetic fixtures：

| 样本 | 路径 | 覆盖 |
|---|---|---|
| EPUB | `tests/fixtures/reader-compat/minimal-deflated.epub` | container、OPF、NCX、nav、文本章节、SVG 图片、ZIP method 8 |
| PDF | `tests/fixtures/reader-compat/minimal-text.pdf` | PDF 1.4、单页 Helvetica 文本、无加密/JavaScript |

最终回归在本机使用但**不得提交到 Git**：

- `测试文件/谈美 (中国文化丛书·经典随行) (朱光潜) (Z-Library).epub`；
- `测试文件/The Road to Serfdom - Text and Documents (The Definite Edition, 2010) (Friedrich August Hayek) (z-library.sk, 1lib.sk, z-lib.sk).pdf`。

截图、HAR、Console 和视频可能包含版权内容或用户书库信息，应存到私有 QA artifact，不放进仓库。

## 3. 每次测试必须记录的身份

建议证据目录：`Lantern-QA/<candidate-commit>/<modern|monterey>/`。至少保存：

| 文件 | 内容 |
|---|---|
| `00-environment.txt` | OS、build、arch、Safari、UA、能力快照、测试时间、测试者 |
| `01-artifact.txt` | DMG/app SHA-256、字节数、版本、About commit、tag/下载 URL |
| `02-about.png` | Settings → About，commit 可读 |
| `03-library.png` | Library 布局 |
| `04-settings.png` | Settings 布局和可操作控件 |
| `10-epub.mp4` | EPUB 打开、TOC、翻页/搜索、图片、标注/书签/恢复 |
| `20-pdf-paged.mp4` | PDF paged 功能矩阵 |
| `21-pdf-scroll.mp4` | PDF scrolling 功能矩阵 |
| `22-pdf-resources.png` | PDF main/worker Network 过滤结果 |
| `23-worker-cycles.txt` | 5 次关闭/重开的 Worker/RSS 记录 |
| `30-console.txt` | 完整 Console 导出或复制文本 |
| `40-performance.csv` | 冷开 1 次、热开 5 次、median 和包体 |
| `result.md` | §10 结果表及失败复现 |

环境采集命令：

```bash
sw_vers
uname -m
/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  /Applications/Safari.app/Contents/Info.plist
plutil -p /Applications/Lantern.app/Contents/Info.plist
shasum -a 256 Lantern.dmg
du -sk /Applications/Lantern.app
spctl -a -vv /Applications/Lantern.app
```

在 Lantern Reader 的 Web Inspector Console 记录：

```js
(() => {
  let deflateRaw = false
  try {
    new DecompressionStream('deflate-raw')
    deflateRaw = true
  } catch {}
  return {
    userAgent: navigator.userAgent,
    urlParse: typeof URL.parse,
    abortSignalAny: typeof AbortSignal.any,
    compressionStream: typeof CompressionStream,
    decompressionStream: typeof DecompressionStream,
    deflateRaw,
    promiseWithResolvers: typeof Promise.withResolvers,
    promiseTry: typeof Promise.try,
    structuredClone: typeof structuredClone,
    uint8ArrayFromBase64: typeof Uint8Array.fromBase64,
    uint8ArrayToBase64: typeof Uint8Array.prototype.toBase64,
    uint8ArrayToHex: typeof Uint8Array.prototype.toHex,
    setIntersection: typeof Set.prototype.intersection,
  }
})()
```

预期边界：

| 环境 | `deflateRaw` | PDF modern 必需能力 | PDF |
|---|---:|---|---|
| 当前 macOS / 新 WebKit | `true` | 上述 URL/Abort/Promise/TypedArray/Set 项均为 `function` | modern |
| macOS 12 / Safari 15.1 | `false` | 至少一项非 `function` | legacy |

不要根据 `sw_vers` 推断分支；必须保留实际能力快照。

## 4. 自动化前置门禁

在候选 commit 的干净工作树执行：

```bash
node --experimental-strip-types --test tests/reader-compat.integration.test.ts
npm run test:unit
npm run build
npm run package
```

W4 集成测试证明：

- synthetic EPUB 在 `CompressionStream` / `DecompressionStream` 被移除后，所有 method 8 条目仍可逐项读取；
- PDF selector 覆盖三个 capability 缺口；
- modern 和 legacy main 各自配对对应 Worker，并能从同一 fixture 读取同一页文本；
- compatibility module/runtime 错误保持 generic Reader error，不误报为 PDF 损坏。

它不证明 WKWebView CSS、真实 Worker 生命周期、用户交互和 Tauri custom scheme，后续真机步骤不可省略。

## 5. Artifact 安装前检查

1. 从拟发布位置下载 artifact，不使用另一次本地构建替代；记录 URL、SHA-256 和字节数。
2. 核对 `CFBundleShortVersionString`、`CFBundleVersion` 和 `LSMinimumSystemVersion = 12.0`。
3. Settings → About 核对 commit；与 tag commit 不同立即停止。
4. 对主二进制和所有 bundled dylib 检查 `minos <= 12.0`。
5. 保留首次 Gatekeeper 结果。当前 ad-hoc 包被 quarantine 拦截时，记录原始错误后按 release notes 执行
   `xattr` workaround，再继续 Reader QA。
6. QA 设备使用专门测试库或完整备份；关闭 iCloud Sync，避免测试高亮、书签和删除动作进入真实书库。

## 6. 当前 macOS modern 基线与候选回归

### 6.1 已采集的 v2.0.0 基线身份

2026-07-19 在当前机器读取到：

| 字段 | 值 |
|---|---|
| OS | macOS 26.5.2 (25F84), Apple Silicon |
| Safari | 26.5.2 |
| 已安装 app | Lantern 2.0.0 |
| About/build commit | `6dd1a2626e7106504e1a97649b25d3342ecf050a` |
| 安装后大小 | 43,892 KiB (`du -sk`) |
| 旧 `LSMinimumSystemVersion` | 10.13 |

该 app 当时正在承载用户会话，未退出、清库或注入性能探针，因此 **`view.open`、首屏和 Worker 数量尚无受控
baseline，不能用当前进程的 RSS 冒充**。由主代理在候选包完成后，按下述同设备协议补采 v2.0.0 与候选值。

### 6.2 同设备性能协议

EPUB 与 PDF 分别执行：

1. 退出 Lantern，确认进程结束；首次启动并打开样本，记为 cold；
2. 保持 app 运行，关闭 Reader 后重开同一样本 5 次，记为 warm 1-5；
3. 用屏幕录像统一测“点击书封到首个可读文字/页面出现”；Web Inspector 若能获取 `view.open` promise
   时间，同时记录但不得用另一台设备的绝对值横比；
4. 取 5 次 warm median。候选 median 必须 `<= v2.0.0 baseline * 110%`；
5. 每次关闭后记录 Worker 数和 RSS，确认没有单调累积；
6. v2.0.0 和候选使用同一本地文件、窗口尺寸、布局模式、电源状态和冷/热定义。

```csv
environment,artifact_commit,format,sample,run_kind,run_index,view_open_ms,first_screen_ms,workers_open,workers_after_close,rss_kib,notes
modern,,epub,tanmei,cold,1,,,,,,
modern,,epub,tanmei,warm,1,,,,,,
modern,,pdf,hayek,cold,1,,,,,,
modern,,pdf,hayek,warm,1,,,,,,
```

### 6.3 modern 路径硬断言

- EPUB 能力快照 `deflateRaw: true`，打开《谈美》成功；
- Network 只有 `vendor/pdfjs/pdf.mjs` 和 `vendor/pdfjs/pdf.worker.mjs`；
- Network **没有**任何 `vendor/pdfjs/legacy/` 请求；
- PDF 打开时只有 1 个 Worker，关闭后归零；连续 5 次关闭/重开不累积；
- 双构建只增加磁盘体积，未使用 legacy 不进入运行内存；
- EPUB/PDF warm median 不超过 v2.0.0 的 110%。

## 7. macOS 12 / Safari 15.1 发布阻断流程

测试机必须是 Apple Silicon Monterey，优先 12.0-12.1 与 Safari 15.1；若只能取得较新的 Monterey/Safari，
记录精确版本并保持状态 **BLOCKED**，不能外推到初始 WebKit。

1. 全新安装候选 packaged `.app`，完成 §3、§5 身份采集；截图首次启动和 About。
2. 在 Library 导入 synthetic EPUB/PDF，确认封面、标题和打开均正常。
3. 记录能力快照；确认 ZIP 原生 `deflate-raw` 不可用且 PDF selector 条件不足。
4. 打开《谈美》，完成 T1；打开 Hayek PDF，分别完成 T2/T3。
5. Web Inspector Network 只允许：
   `vendor/pdfjs/legacy/pdf.mjs` + `vendor/pdfjs/legacy/pdf.worker.mjs`；不得请求或解析 modern 两件套。
6. 打开 PDF 时记录 Worker=1；关闭 Reader 后记录 Worker=0。重复 5 次并记录 RSS，不得累积。
7. 执行 searchable OCR、Library/Settings、Console 和其他格式边界检查。
8. 任一步超过 45 秒 Reader timeout、持续卡住 UI 或出现兼容错误均为 FAIL。

旧系统 ZIP fallback 的证据组合必须同时包含：能力快照 `deflateRaw: false`、synthetic method 8 EPUB 成功、
《谈美》成功、Console 无构造器错误，以及自动化 fallback 测试通过。单独“书打开了”证据不足。

## 8. 手工功能矩阵

### T1 EPUB（synthetic +《谈美》+ 大书/图片书）

- [ ] metadata、封面、TOC、首章和含图片章节可读；
- [ ] paged 翻页和 scrolling 均可操作；
- [ ] 搜索定位正确，文本选择/复制正常；
- [ ] 添加、编辑、删除高亮；添加、删除书签；
- [ ] 关闭 Reader 后恢复 CFI/阅读位置；
- [ ] AI lookup/翻译入口出现并能完成一次请求；
- [ ] 大 EPUB 打开低于 45 秒，交互无持续卡死；
- [ ] fallback 按 entry 工作，无整本 `unzipSync` 峰值。

### T2 PDF paged（synthetic + Hayek）

- [ ] 首屏、页码、fit-width、缩放正常；
- [ ] 单页/双页切换正常；
- [ ] 搜索、拖选、复制、高亮正常；
- [ ] 书签和页码恢复正常；
- [ ] AI lookup/翻译入口正常；
- [ ] 关闭/重开/Retry 无 orphan Worker。

### T3 PDF scrolling（Hayek）

- [ ] 切换 scrolling 后连续滚动、页码和缩放正常；
- [ ] 文本层对齐，搜索定位和跨行选择正常；
- [ ] 返回 paged 后当前位置合理；
- [ ] 关闭后 Worker 被终止。

### T4 searchable OCR PDF

- [ ] 页面图像正常；
- [ ] 隐藏文字层仍可选择、复制和搜索；
- [ ] 高亮与 AI lookup 使用正确文本；
- [ ] 普通文本 PDF 行为不受 OCR 分支影响。

### T5 Library / Reader / Settings UI

- [ ] Library grid/list、导入按钮、搜索、collection 控件无错位或不可点击；
- [ ] Reader 工具栏、侧栏、TOC、搜索、设置、错误页文本可见且不重叠；
- [ ] Settings 分区、下拉框、toggle、输入框和滚动均可操作；
- [ ] 100% 和 125% 显示缩放各检查一次；
- [ ] Console 无 CSS parse 导致的关键控件缺失或 JS exception。

### T6 其他格式静态/打开边界

| 格式 | 样本 | Monterey 结果 | Console/API 审计 | 证据 |
|---|---|---|---|---|
| MOBI/AZW/AZW3 | 任选可公开测试样本 |  |  |  |
| FB2/FBZ | 任选可公开测试样本 |  |  |  |
| CBZ | 任选可公开测试样本 |  |  |  |

### 扩展样本登记

| 类别 | 文件名/私有 ID | SHA-256 | 页/章节/大小 | modern | Monterey |
|---|---|---|---|---|---|
| 大 EPUB |  |  |  |  |  |
| 图片型 EPUB |  |  |  |  |  |
| 普通文本 PDF |  |  |  |  |  |
| searchable OCR PDF |  |  |  |  |  |
| 长 PDF | Hayek |  | 314 页 |  |  |

## 9. 资源、Worker、内存和体积

每次打开 PDF 前清空 Network，打开后按 `pdf.mjs` 过滤并截图。保存完整 URL、initiator、status 和 transfer
size；只看文件名不足以排除路径混配。

| 环境 | cycle | main URL | worker URL | 打开时 Worker | 关闭后 Worker | RSS KiB |
|---|---:|---|---|---:|---:|---:|
| modern | 1 |  |  |  |  |  |
| modern | 2-5 |  |  |  |  |  |
| Monterey | 1 |  |  |  |  |  |
| Monterey | 2-5 |  |  |  |  |  |

体积结果：

| 指标 | v2.0.0 | 候选 | 增量 | 预算 | 结果 |
|---|---:|---:|---:|---:|---|
| DMG bytes |  |  |  | <= 1.5 MB |  |
| `.app` KiB | 43,892 |  |  | legacy <= 3.8 MB 安装后增量 |  |
| legacy main+worker gzip | N/A |  |  | <= 0.9 MB |  |
| ZIP fallback gzip | N/A |  |  | <= 100 KB |  |

## 10. 最终结果表

| Gate | modern | Monterey 12 / Safari 15.1 | 证据 | 结论 |
|---|---|---|---|---|
| Artifact/About 身份一致 |  |  |  |  |
| EPUB synthetic +《谈美》 |  |  |  |  |
| PDF synthetic + Hayek paged/scroll |  |  |  |  |
| searchable OCR PDF |  |  |  |  |
| PDF main/worker 成对且单变体 |  |  |  |  |
| 5 次 Worker cleanup |  |  |  |  |
| Library/Reader/Settings |  |  |  |  |
| Console 兼容错误为零 |  |  |  |  |
| 性能 <= baseline 110% |  |  |  |  |
| 包体/原生最低版本 |  |  |  |  |
| 其他格式边界 |  |  |  |  |

总结果只能填写 **PASS / FAIL / BLOCKED**。只有两列均 PASS 且全部证据可追溯时，W4 才完成发布门禁。

## 11. 失败回报模板

```text
标题: [Reader compat][modern|Monterey] <最短症状>
Artifact: <version> <commit> <SHA-256>
环境: macOS <version/build>, Safari <version>, UA <full string>
样本: <private ID or fixture path>, SHA-256 <hash>
布局: paged|scrolling, single|double, zoom <value>
前置状态: cold|warm, 第 <n> 次打开, Worker before=<n>
复现步骤:
1. ...
2. ...
实际结果: ...
预期结果: ...
耗时: view.open=<ms>, first screen=<ms>
资源: main=<URL>, worker=<URL>, Worker after close=<n>
Console: <first exception + stack>
证据: <video/screenshot/HAR/log paths>
复现率: <n>/<n>
```

兼容加载失败必须保留原始 module/runtime error；不要先改名为“PDF 损坏”。W4 只提交稳定复现和证据，修复回流
W1（EPUB）、W2（PDF）或 W3（build/CSS），不得在 QA 工作流直接改 Reader 实现。
