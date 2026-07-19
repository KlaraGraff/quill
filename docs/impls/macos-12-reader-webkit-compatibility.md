# macOS 12 Reader WebKit 兼容性升级实施方案

> 状态：**代码与自动化已实施；真实 Monterey packaged `.app` 发布验收待执行**
>
> 定稿日期：2026-07-19
>
> 读者：执行本方案的 GPT-5.6 主代理及其最多 4 个子代理。W1-W4 均是可独立派发的任务书，
> 不依赖本次讨论上下文。

## 0. 结论

Lantern 保留一套 Foliate Reader 上层逻辑，在底层按 **WebKit 能力** 选择实现：

- EPUB 始终使用本地 `groupBy` helper；ZIP 解压优先系统 `DecompressionStream`，缺失时使用
  zip.js 同版本自带的纯 JavaScript 流式 codec；
- PDF.js 5.5.207 同时保留 modern 与 legacy 构建，打开 PDF 前选择，主模块和 Worker 必须成对；
- React/Vite 主应用显式以 Safari 15 为目标；`public/foliate-js` 不再作为未经检查的原样资产，
  生产构建必须转译 Reader 源模块并通过 Safari 15 兼容门禁；
- 不按 `sw_vers`、UA 或 macOS 大版本分支。同一个 macOS 12 可能运行 Safari 15 或更新后的
  Safari 17，能力检测才是实际运行边界；
- 不维护两套完整 Foliate，不引入全局 polyfill 包，不用旧 PDF.js 版本作为 fallback。

最终兼容目标是 **Apple Silicon、macOS 12 Monterey、初始 WKWebView/Safari 15.1**。发布前必须在
真实 Monterey 设备的打包 `.app` 中完成 EPUB/PDF 全链路验收；Playwright WebKit、Node 测试和
当前 macOS 只能作为前置门禁，不能替代真机结论。

## 1. 已确认事实

### 1.1 两个故障不是文件损坏

- `测试文件/谈美 (中国文化丛书·经典随行) (朱光潜) (Z-Library).epub` 已通过 `unzip -t`；
- `测试文件/The Road to Serfdom - Text and Documents (The Definite Edition, 2010) (Friedrich August Hayek) (z-library.sk, 1lib.sk, z-lib.sk).pdf`
  是正常 PDF 1.4、314 页、未加密、无嵌入 JavaScript；
- 故障发生在 `src/pages/reader/useFoliateView.ts` 的 `view.open(file)`，导入、文件复制和数据库记录
  已完成，属于 Reader 打开失败。

### 1.2 v2.0.0 的直接兼容缺口

| 链路 | 当前实现 | macOS 12 初始 WebKit 的结果 |
|---|---|---|
| EPUB ZIP | `@zip.js/zip.js` 2.8.22 的精简 `zip-core`，`useWebWorkers: false` | 缺少 `DecompressionStream` 时 codec 为 `false`，随后执行 `new false(...)` |
| EPUB metadata | `Object.groupBy` / `Map.groupBy` | Safari 17.4 前不存在 |
| PDF main | `pdfjs-dist/build/pdf.mjs` 5.5.207 modern build | `static {}` 在 Safari 16.4 前解析失败 |
| PDF runtime | `Promise.withResolvers` 等现代 API | Safari 17.4 前运行失败 |
| PDF 部分路径 | `Uint8Array.fromBase64` | Safari 18.2 前不存在 |

Safari 支持边界：

| 能力 | Safari 首个支持版本 |
|---|---:|
| `CompressionStream` / `DecompressionStream` | 16.4 |
| class static initialization block | 16.4 |
| `Object.groupBy` / `Map.groupBy` | 17.4 |
| `Promise.withResolvers` | 17.4 |
| `Uint8Array.fromBase64` | 18.2 |

### 1.3 为什么构建和当前系统没有暴露问题

- Tauri 使用系统 `WKWebView`，不捆绑 Chromium。原生 Mach-O 和 PDFium 的 `minos <= 12.0` 只证明
  应用可以被 macOS 12 加载，不证明 Reader JavaScript 可执行；
- 当前 `vite.config.ts` 未指定 `build.target`，Vite 7 默认目标包含 `safari16`；
- `public/foliate-js` 由 Vite 原样复制，完全绕过 Vite 的语法转译；
- CI 在 Ubuntu/Node 22 构建，release 在 `macos-latest` 打包，没有 macOS 12 WKWebView smoke；
- 当前开发机为 macOS 26.5.2 / Safari 26.5.2，天然具备上述全部能力。

### 1.4 v2.0.0 精确依赖基线

`v2.0.0` 指向 Foliate submodule commit `112eb278e4fc04f48f494dd71b213b2536bf4062`；当前 vendored
目录由同一 commit 转换而来。依赖锁定为：

- `@zip.js/zip.js` 2.8.22；
- `pdfjs-dist` 5.5.207；
- `fflate` 0.8.2（当前只用于 MOBI zlib，不作为首选 EPUB ZIP fallback）。

## 2. 目标与非目标

### 2.1 必须实现

1. macOS 12 / Safari 15.1 能打开上述 EPUB 和 PDF；
2. 当前 macOS 仍走 ZIP 原生解压和 PDF modern build；
3. EPUB/PDF 的 TOC、分页/滚动、搜索、文本选择、高亮、书签、CFI/页码恢复和 AI 入口不回归；
4. modern/legacy 选择对用户透明，不增加设置项；
5. 未使用的 PDF.js 构建不被下载、解析或创建 Worker，运行内存不翻倍；
6. 生产构建和 CI 能阻止再次引入 Safari 15 不可解析语法及未隔离的新 API；
7. 打包 `Info.plist` 明确 `LSMinimumSystemVersion = 12.0`，原生依赖继续满足该下限；
8. README 的 macOS 12 声明与真实安装包行为一致。

### 2.2 明确不做

- 不支持 Intel Mac；
- 不支持 macOS 11；
- 不引入旧数据 migration 或旧协议兼容代码；
- 不维护两套 Foliate 源码；
- 不用 PDF.js 3.x/4.x 作为低版本分支；
- 不全局加载 core-js 或修改所有页面的内建对象；
- 不以一次性 `unzipSync` 解压整本 EPUB；大书不能用峰值内存换兼容性；
- 不新增用户可见的“兼容模式”开关；
- 不承诺 ZIP 密码、DRM 或当前产品本就没有入口的加密格式；
- 不借本项目重构 Reader、PDF layout、annotation 或 OCR 资产系统。

## 3. 技术决策

### 3.1 能力选择，不看系统版本

所有选择函数都必须是可注入、可单测的纯函数。生产调用传 `globalThis` 的能力快照，测试传显式对象。

```js
export const selectPdfJsVariant = capabilities =>
    capabilities.urlParse
    && capabilities.abortSignalAny
    && capabilities.promiseTry
    && capabilities.promiseWithResolvers
    && capabilities.structuredClone
    && capabilities.uint8ArrayFromBase64
    && capabilities.uint8ArrayToBase64
    && capabilities.uint8ArrayToHex
    && capabilities.setIntersection
        ? 'modern'
        : 'legacy'
```

这是实施审计后确认的最小集合，覆盖 `URL.parse`、`AbortSignal.any`、`Promise.try`、
`Promise.withResolvers`、`structuredClone`、TypedArray Base64/Hex 和 `Set.prototype.intersection`。
后续依赖更新若发现 modern PDF.js 还有无条件使用的新 API，必须把它加入能力快照；不能靠 UA 白名单补洞。
modern 动态导入本身若拒绝，可在**尚未创建 Worker、尚未调用 `getDocument`** 时回退 legacy；无效 PDF、
密码错误和文档解析错误不得触发构建切换。

ZIP 不只检查构造器是否存在，还要验证格式：

```js
const supportsNativeDeflateRaw = () => {
    try {
        new DecompressionStream('deflate-raw')
        return true
    } catch {
        return false
    }
}
```

实际 zip.js 配置应始终带纯 JS codec，让库在原生构造失败时自动 fallback；上面的 probe 用于测试、诊断和
性能断言，不用于复制一套 ZIP loader。

### 3.2 EPUB：同一 zip.js，原生流与纯 JS 流自动回退

当前 `rollup/zip.js` 导入 `@zip.js/zip.js/lib/zip-core.js`，该精简入口没有可供旧 WebKit 使用的 codec。
改为同版本公开导出的 `@zip.js/zip.js/lib/zip-core-native.js`。zip.js 的 `native` 命名在这里指内嵌纯
JavaScript Compression Streams 实现，不是要求更高系统版本。

要求：

- 保持 `ZipReader`、`BlobReader`、`TextWriter`、`BlobWriter` 和 `view.js` loader 接口不变；
- 保持 `useWebWorkers: false`，继续避开 Tauri 自定义 scheme 下额外 Worker/WASM URL 风险；
- 系统支持 `DecompressionStream('deflate-raw')` 时仍优先原生实现；
- 旧系统按 entry 流式解压，不把整本 EPUB 展开到 JS heap；
- 若 `zip-core-native` 在 Monterey 真机失败，第二选择才是一个基于既有 `fflate.Inflate` 的
  `TransformStream` adapter；禁止直接使用 `unzipSync` 全量展开。

`Object.groupBy` / `Map.groupBy` 由 `epub.js` 内两个局部 helper 替换。helper 要保持当前调用语义：

- object 版本返回 null-prototype object，键转为 property key；
- map 版本保留 `null`、字符串等键的身份；
- 每组保持输入顺序；
- 只替换当前 5 个 metadata 调用点，不抽成全仓库 polyfill。

### 3.3 PDF：同版本 modern/legacy 双构建

保留 `pdfjs-dist` 5.5.207，一次依赖升级同时产生两组代码：

```text
public/foliate-js/vendor/pdfjs/
  pdf.mjs                    # modern main，保留当前 URL
  pdf.worker.mjs             # modern worker
  legacy/pdf.mjs             # legacy main
  legacy/pdf.worker.mjs      # legacy worker
  cmaps/                     # 共用
  standard_fonts/            # 共用
  *.css                      # 共用
```

约束：

- modern 和 legacy 必须来自同一个 lockfile 版本；
- 主模块与 Worker 只能成对，禁止 modern main + legacy worker 或反向组合；
- `pdf.js` 删除顶部 static import，改为缓存的 literal dynamic import；Safari 15 在选择前不得解析 modern 文件；
- 使用动态 import 返回的 module exports，不再依赖 `globalThis.pdfjsLib`；
- loader 返回 `{ pdfjsLib, workerUrl, variant }`，`makePDF()` 其余实现共用；
- import Promise 失败后清空缓存，Reader 的 Retry 才能重新尝试；
- 继续使用显式 `workerPort`，保持当前 Tauri custom-scheme 修复和 Worker cleanup；
- 不把 legacy source map 放进生产包。其 npm source map 约 7.8 MB，调试时可从 lockfile 对应 tarball取得；
- CMap、standard fonts、CSS 不复制第二份。

PDF.js 5.5.207 的 official legacy build 仍残留一个 class static block，生成时使用 Rollup AST 位置将其降为
Safari 15 可解析的静态私有字段初始化器。该 legacy build 也没有提供同步 `structuredClone` polyfill；loader
只在选择 legacy 后安装局部实现，覆盖 Reader 可达的 ArrayBuffer、TypedArray、Map、Set、Blob、File、Error
等类型。`transfer` 在旧路径采用复制而非 detach，以少量峰值内存换取正确性；modern 路径不加载这段实现。

当前测量值：

| PDF.js 代码 | 未压缩 | gzip 近似 |
|---|---:|---:|
| modern main + worker | 2.98 MB | 628 KB |
| legacy main + worker | 3.34 MB | 702 KB |
| 双构建合计 | 6.31 MB | 1.33 MB |

相对当前安装包，预计增加约 3.34 MB 安装后空间、0.7 MB 左右 DMG 压缩体积。只有被选中的主模块和 Worker
进入内存。

### 3.4 Reader 纳入 Safari 15 构建

分两层处理：

1. `vite.config.ts` 显式设置 `build.target: 'safari15'` 与对应 CSS target，React 应用不再依赖 Vite
   未来默认值；
2. 新增 `scripts/build-reader-assets.mjs`。Vite 将 `public/` 复制到 `dist/` 后，脚本用仓库已安装的
   esbuild 将 `dist/foliate-js/**/*.js` 独立转译到 Safari 15，保持 ESM 相对 URL、dynamic import 和
   `import.meta.url`。`vendor/pdfjs/**/*.mjs` 不转译：modern 由能力隔离，legacy 已由 PDF.js 官方构建。

新增 `scripts/check-reader-compat.mjs` 作为构建门禁：

- 检查最终产物存在 modern/legacy main-worker 四件套；
- 从文件 header 断言四件套版本一致；
- 断言 `pdf.js` 没有 static import modern PDF.js；
- 检查 Safari 15 路径不直接使用已知新 API；
- 对 Reader `.js` 做 Safari 15 syntax parse/transform；
- 检查 legacy source map 未进入 `dist`；
- 输出各变体字节数并执行 §8 预算；
- 对最终 CSS 做兼容审计，实际使用的 Tailwind 4 新语法若在 Safari 15 被忽略，补最小 fallback。

static import 和受限 API 检查使用仓库已有 TypeScript compiler API 建 AST，不用正则猜 JavaScript 语义；
version header 和文件大小等纯文本元数据才允许直接读取。

生产 `npm run build` 必须包含 Reader transform 和 compatibility check。开发服务器仍可读取源码，但真机结论
一律基于 packaged `.app`，不能用 `npm run dev` 代替。

### 3.5 原生最低版本显式化

`src-tauri/tauri.conf.json` 的 `bundle.macOS.minimumSystemVersion` 设为 `12.0`，使
`LSMinimumSystemVersion` 与 `MACOSX_DEPLOYMENT_TARGET` 不再依赖 Tauri 默认 `10.13`。发布校验继续检查：

- 主可执行文件和所有捆绑 dylib 的 `minos <= 12.0`；
- `Info.plist` 的 `LSMinimumSystemVersion = 12.0`；
- 当前 ad-hoc / 后续 Developer ID 签名流程均不改变该值。

## 4. 并行契约

### 4.1 GPT-5.6 编排规则

主代理最多同时派发 **4 个子代理**，对应 W1-W4。四者可以在同一基线并发，必须遵守：

1. 子代理先读 `AGENTS.md` 和本文，只改分配文件；
2. 子代理不 fetch、不 rebase、不 stash、不 push，不处理其他代理的 dirty changes；
3. 子代理不得提交共享 generated vendor 全量重建结果；最终 vendor regeneration 由主代理在集成后执行一次；
4. 子代理返回：改动摘要、精确文件列表、已运行命令、失败/未运行项、风险；
5. 接口若需变化，先通知主代理，不能单方改写 §4.2；
6. 主代理负责逐工作流审查、集成、全量测试、focused commits 和 push；
7. 若工具支持隔离 worktree，可使用；否则依靠下表的文件所有权避免冲突。

### 4.2 冻结接口

W1-W4 以这些名字开发，避免并行期互相等待：

```js
// public/foliate-js/pdf-compat.js，W2 所有
snapshotPdfCapabilities(globalObject) -> PdfCapabilities
selectPdfJsVariant(capabilities) -> 'modern' | 'legacy'
loadPdfJs() -> Promise<{ pdfjsLib, workerUrl, variant }>

// public/foliate-js/epub.js，W1 所有，仅测试导出
groupByObject(items, keyFn) -> null-prototype object
groupByMap(items, keyFn) -> Map
```

产物路径固定为 §3.3；W3 的 build/check 脚本据此开发。W4 的测试可以直接 import 两组纯函数，不需要等待
浏览器环境。

### 4.3 文件所有权

| 工作流 | 独占文件 | 禁止改动 |
|---|---|---|
| W1 EPUB | `public/foliate-js/epub.js`、`view.js`、`rollup/zip.js`、`vendor/zip.js`、W1 新测试 | `pdf*`、root build/CI |
| W2 PDF | `public/foliate-js/pdf.js`、新 `pdf-compat.js`、`rollup.config.js`、`vendor/pdfjs/legacy/**`、W2 新测试 | EPUB/view、root build/CI |
| W3 Build | `vite.config.ts`、根目录 `package.json` / `package-lock.json`、`src-tauri/tauri.conf.json`、`.github/workflows/ci.yaml`、`scripts/*reader*` | Reader 行为代码、OCR 文件 |
| W4 QA | `tests/fixtures/reader-compat/**`、W4 新集成测试、`docs/guide/macos-12-reader-qa.md` | W1-W3 实现文件、OCR 文件 |

`public/foliate-js/rollup.config.js` 由 W2 独占；W1 只改已有 `rollup/zip.js` 入口。主代理集成 W1/W2 后运行一次
`npm --prefix public/foliate-js ci` 和 `npm --prefix public/foliate-js run build`，按路径分别审查生成结果。

## 5. 四路并行工作流

### W1 - EPUB metadata 与 ZIP fallback

**目标**：同一 EPUB loader 在现代系统使用原生 DEFLATE，在 Safari 15 使用 zip.js 纯 JS 流式 codec。

任务：

1. 在 `epub.js` 添加 §4.2 的两个局部 helper，替换全部 `Object.groupBy` / `Map.groupBy`；
2. 将 `rollup/zip.js` 的入口从 `lib/zip-core.js` 改为 `lib/zip-core-native.js`；
3. 保持 `view.js` 的 `useWebWorkers: false` 和公开 loader 形状；仅增加必要的 codec 配置/probe；
4. 重新生成 `vendor/zip.js`，记录 raw/gzip 大小；
5. 在 Node 22 测试中把 `globalThis.CompressionStream` / `DecompressionStream` 设为 `undefined`，读取一个
   method 8 的最小 ZIP entry，必须成功；
6. 测试原生构造器存在、原生构造器存在但拒绝 `deflate-raw`、构造器完全缺失三种情况；
7. 用《谈美》执行本地 smoke：metadata、TOC、首章、含图片章节均可读取；
8. 确认 fallback 按 entry 解压，没有 `arrayBuffer()` + `unzipSync()` 全书展开；
9. 审计 zip.js 在产品可达路径中的 Safari 15 API；不可达的 ZIP 密码路径记录为非目标，不加全局 polyfill。

完成标准：

- 缺失 Compression Streams 的自动化测试复现旧环境并通过；
- 《谈美》在当前系统可正常打开，现代路径仍选择原生解压；
- ZIP vendor 增量在 §8 预算内；
- 没有 PDF、React、Rust 或 OCR diff。

建议验证命令：

```bash
npm --prefix public/foliate-js ci
npm --prefix public/foliate-js run build
npm run test:unit
```

### W2 - PDF.js modern/legacy 双构建与 loader

**目标**：Safari 15 在解析 modern 文件前选择 legacy；现代 WebKit 保持当前 PDF.js 代码和性能。

任务：

1. 扩展 `rollup.config.js`，从同一 `pdfjs-dist` 复制 modern 与 legacy main/worker；共享 CMap、字体、CSS；
2. 不复制 legacy `.map`，必要时移除 legacy 文件尾部 `sourceMappingURL`；
3. 新建 `pdf-compat.js`，实现 §4.2 的 capability snapshot、纯 selector 和 cached dynamic loader；
4. modern/legacy 两个 import 必须是 literal dynamic import，避免 Safari 15 提前解析 modern；
5. modern import 在模块初始化阶段拒绝时回退 legacy；一旦创建 Worker 或调用 `getDocument`，不再切换；
6. `pdf.js` 改用 loader 返回的 module exports 和匹配 worker URL，保留显式 `workerPort`、range transport、
   CMap/font 路径和现有 cleanup；
7. 单测 capability 矩阵、import retry cache reset、main-worker 配对、现代系统不选择 legacy；
8. 对 modern/legacy 分别跑最小 PDF `getMetadata` / 首页 render smoke；
9. 使用 Hayek PDF 测试 paginated 与 scrolling 两种模式、文本选择、搜索、关闭后 Worker 终止；
10. 记录四个代码文件的 raw/gzip 大小和 PDF.js version header。

完成标准：

- Safari 15 能解析 `pdf.js` 和 `pdf-compat.js`，且 selector 返回 legacy；
- 当前系统 selector 返回 modern，Network/资源记录中不加载 legacy；
- 两个 variant 的 API 输出对上层一致；
- 失败时无 orphan Worker，Retry 可重新加载；
- 没有 EPUB、root build/CI、Rust 或 OCR diff。

建议验证命令：

```bash
npm --prefix public/foliate-js ci
npm --prefix public/foliate-js run build
npm run test:unit
```

### W3 - Safari 15 构建、CSS 审计与 CI 门禁

**目标**：把“支持 macOS 12”变成构建契约，而不是依赖开发机恰好足够新。

任务：

1. `vite.config.ts` 显式设置 JS/CSS Safari 15 target；
2. 新增 `scripts/build-reader-assets.mjs`，按 §3.4 转译 Vite 已复制的 Reader `.js`；
3. 新增 `scripts/check-reader-compat.mjs`，实现 §3.4 全部门禁；
4. 在 root `package.json` 注册 `build:reader-assets`、`check:reader-compat`，并接入现有 `npm run build`；
5. 如脚本直接 import esbuild，则把现有 transitive esbuild 固定为 direct devDependency，避免 npm 布局变化；
6. `ci.yaml` 增加清晰命名的 Reader compatibility step；
7. `tauri.conf.json` 设置 `minimumSystemVersion: "12.0"`；
8. 对最终 Tailwind CSS 做 Safari 15 审计和页面 smoke。只补实际使用规则的最小 fallback；不得顺手降级
   Tailwind 或重写全站样式；
9. 验证 `npm run dev` 不受脚本影响，`npm run build` 的 `dist/foliate-js` 已转译且路径不变；
10. 生成一份机器可读 size report，供 W4 和 release gate 使用。

完成标准：

- `npm run build` 缺任一 legacy 文件、版本不匹配、static import modern、Reader syntax 超线或超 size budget
  都会失败；
- `dist` 中 app shell 与 Reader compatibility path 均以 Safari 15 为目标；
- Library、Reader、Settings 在 Safari 15 没有因 CSS 新语法出现不可操作控件；
- 没有 W1/W2 Reader 行为、Rust OCR 或同步 diff。

建议验证命令：

```bash
npx tsc --noEmit
npm run lint
npm run test:unit
npm run build
```

### W4 - Fixtures、集成回归与 Monterey 真机任务书

**目标**：建立可重复的兼容验收，不让本次修复只覆盖两条截图路径。

任务：

1. 创建最小 deflated EPUB fixture，覆盖 `container.xml`、OPF metadata、NCX/nav、一个文本章节和一张图片；
2. 创建最小文本 PDF fixture；fixture 应小且可审查，不提交用户的 Z-Library 文件；
3. 增加集成测试，验证 selector、ZIP fallback、PDF variant 配对和 Reader error 不把兼容加载失败误报为“PDF 损坏”；
4. 新建 `docs/guide/macos-12-reader-qa.md`，列出 §7 真机步骤、证据格式和结果表；
5. 在当前 macOS 跑 modern baseline，记录 EPUB/PDF `view.open`、首屏时间、包体和 Worker 数量；
6. W1-W3 合入后，在 macOS 12 packaged `.app` 跑 legacy/fallback；记录 OS、Safari、WebKit UA、应用 commit；
7. 检查 Library、Reader、Settings 的 CSS/交互，不只验证“文件能打开”；
8. 使用用户的《谈美》和 Hayek PDF 做最终回归，但不把样本加入 git；
9. 增加一份大 EPUB、图片型 EPUB、普通文本 PDF、searchable OCR PDF 的手工矩阵；
10. 对失败项给出稳定复现步骤，不在 W4 越权修改 W1-W3 实现。

完成标准：

- 自动化 fixture 可在 CI 运行；
- Monterey 和当前 macOS 的结果都有 commit、系统/WebKit 版本和截图/日志；
- modern/legacy 各自只加载一个 PDF main 和一个 Worker；
- 关闭/重开 PDF 不累积 Worker；
- QA 文档足以让另一名代理或维护者复测。

建议验证命令：

```bash
npm run test:unit
npm run build
npm run package
```

## 6. 主代理集成顺序

```text
M0  主代理：确认 clean ownership，记录 baseline，冻结 §4.2
      |
      +--> W1 EPUB -------------------+
      +--> W2 PDF --------------------+--> M1 source integration
      +--> W3 Build/CI ---------------+
      +--> W4 Fixtures/QA skeleton ----+
                                      |
M1  主代理：审查 W1-W3，最终 regenerate vendor，运行自动化
                                      |
M2  W4：当前 macOS modern 回归 + packaged app smoke
                                      |
M3  真实 macOS 12：legacy/fallback 全链路，失败则回对应 owner
                                      |
M4  主代理：release artifact 验证，patch version 发布
```

集成细则：

1. 先审 W1 source，再审 W2 source；主代理随后执行一次 nested `npm ci` + build，避免两个子代理提交互相覆盖的
   generated vendor；
2. 再接 W3，让最终 build/check 读取已确定的产物路径；
3. 接 W4 fixture/test/guide；
4. 主代理更新 `public/foliate-js/LANTERN.md`，记录 ZIP/PDF 双路径、生成命令和 Safari 15 维护边界；
5. 运行 §9 全部命令；
6. 按 §10 真机结果回流。macOS 12 失败不能用“当前系统正常”关闭；
7. 每个工作流形成 focused commit，最后再做一个仅包含必要生成资产的 vendor commit；
8. 直接推 `main`，除非 CI 必须 gate 风险或维护者明确要求 PR。

## 7. 运行验收矩阵

### 7.1 macOS 12 / Safari 15.1（发布阻断）

- [ ] 安装并首次启动 packaged `.app`；
- [ ] Library、Reader、Settings 无错位、不可点击或文本不可见；
- [ ] 导入并打开《谈美》，TOC、首章、图片、翻页、搜索正常；
- [ ] EPUB 添加/删除高亮、书签，关闭后恢复位置；
- [ ] 导入并打开 Hayek PDF，paged/scrolling 均正常；
- [ ] PDF fit-width、缩放、单双页、搜索、拖选、复制、高亮、AI lookup 正常；
- [ ] PDF 关闭、重开、Retry 不遗留 Worker；
- [ ] 打开 searchable OCR PDF，文字层仍可选择；
- [ ] Console 无 `false is not a constructor`、`Unexpected token '{'`、`Promise.withResolvers` 错误；
- [ ] 资源记录只出现 legacy PDF main/worker；
- [ ] ZIP 记录/测试证明使用纯 JS fallback，未全书解压；
- [ ] About commit 与测试 artifact/tag 一致。

### 7.2 当前 macOS（不得回归）

- [ ] 相同 EPUB/PDF 功能矩阵通过；
- [ ] PDF 只加载 modern main/worker；
- [ ] EPUB 使用系统 `DecompressionStream('deflate-raw')`；
- [ ] modern `view.open` 中位数不超过改动前 baseline 的 110%；
- [ ] 未加载 legacy 代码，运行内存不因双构建增加；
- [ ] OCR searchable PDF、普通 PDF、长 PDF 无文本层回归。

### 7.3 其他格式静态边界

MOBI/AZW/AZW3、FB2/FBZ、CBZ 至少进入 Safari 15 syntax/API audit。若发现产品可达路径依赖更高 API，
必须在本版本修复或明确阻断“macOS 12 全格式支持”发布，不得只因本次两个样本是 EPUB/PDF 而忽略。

## 8. 性能和体积预算

| 指标 | 预算 | 超线处理 |
|---|---:|---|
| legacy PDF main + worker 压缩增量 | <= 0.9 MB | 检查是否误带 source map/重复资源 |
| legacy PDF 安装后增量 | <= 3.8 MB | 检查 CMap/font/CSS 是否重复 |
| ZIP fallback gzip 增量 | <= 100 KB | 优先 tree-shake 到 reader-only API |
| 整体 DMG 增量 | <= 1.5 MB | 输出资产明细，禁止无解释放宽 |
| 当前系统 EPUB/PDF 打开中位数 | <= baseline 110% | profile import、Worker、首屏 |
| 当前系统运行内存 | 只加载一个 PDF variant | 资源/Worker 检查为硬门禁 |
| macOS 12 打开 | 低于现有 45 s Reader timeout，且无持续 UI 卡死 | 优化流式 codec；禁止全量 unzip |

性能测试每个样本至少冷开 1 次、热开 5 次，记录 median；不同设备不做绝对横比，只比较同设备改动前后。

## 9. 自动化验证

主代理集成后运行：

```bash
npx tsc --noEmit
npm run lint
npm run test:unit
npm run build
```

Reader vendor 重新生成时另跑：

```bash
npm --prefix public/foliate-js ci
npm --prefix public/foliate-js run build
```

打包验证：

```bash
npm run package
plutil -p src-tauri/target/release/bundle/macos/Lantern.app/Contents/Info.plist
otool -l src-tauri/target/release/bundle/macos/Lantern.app/Contents/MacOS/quill
spctl -a -vv src-tauri/target/release/bundle/macos/Lantern.app
```

若现有 OCR/同步在途改动导致全仓检查失败，必须区分“本项目引入”与“基线已失败”，记录精确命令和首个错误；
不能修改或提交不属于本项目的 dirty files 来换绿。

## 10. 风险与处理

| 风险 | 处理 |
|---|---|
| PDF modern main/worker 混配 | loader 返回成对路径；build check 比对 version header |
| Safari 15 在 import 前解析 modern | 禁止 static import；只用 literal dynamic branch；真机 Network 验证 |
| legacy core-js 影响全局 | 仅低能力 PDF 打开时懒加载；现代路径永不 import legacy |
| ZIP JS codec 阻塞主线程 | 使用 zip.js 流式 codec；大 EPUB profile；禁止 `unzipSync` 全量展开 |
| Tailwind 4 输出高于 Safari 15 | 最终 CSS audit + 三页真机 smoke；补实际规则 fallback |
| dev 与 packaged build 不一致 | release gate 只承认 packaged `.app` |
| 依赖升级再次抬高能力线 | lockfile 同版本双构建 + compatibility script + 真机门禁 |
| macOS 12 机器无法获得 | 状态标记 blocked，不发布“已支持”；现代模拟测试不能替代 |
| 两路生成资产互相覆盖 | 子代理不提交全量 regeneration；主代理集成后统一生成 |
| 兼容错误被误报成坏 PDF | W4 覆盖 `toReaderOpenError`，module/runtime 失败保持 generic |

Rollback 以 commit 为单位：PDF loader、EPUB fallback、build gate 可分别回退。任何 rollback 都不得删除原书、
修改数据库或迁移用户数据；本项目只有静态资产和 Reader 加载逻辑。

## 11. 发布门禁

全部满足才可发布：

- [ ] W1-W4 完成标准全部满足；
- [ ] 自动化和 packaged build 通过；
- [ ] 真实 macOS 12 与当前 macOS 证据齐全；
- [ ] `LSMinimumSystemVersion = 12.0`，所有原生库 `minos <= 12.0`；
- [ ] DMG/安装后体积在预算内；
- [ ] modern 系统未加载 legacy，旧系统未解析 modern；
- [ ] EPUB/PDF 及 OCR searchable PDF 功能矩阵通过；
- [ ] release asset 下载后验证体积、About commit、`spctl -a -vv`；
- [ ] release notes 继续说明 ad-hoc 签名的 `xattr` workaround（直到 Developer ID 项目完成）。

`v2.0.0` 与 `v2.0.1` 均已发布且版本号已烧毁。修复发布必须使用新的 patch，例如 **v2.0.2**，禁止用
不同内容重新上传既有版本号的同名资产。

## 12. 四个子代理的直接派发摘要

主代理可把对应 §5 连同以下一行直接发给子代理：

| 子代理 | 派发摘要 |
|---|---|
| W1 | “只负责 EPUB：替换 groupBy，改用 zip-core-native 流式 fallback，覆盖缺失 Compression Streams；严格遵守 §4 文件边界和 §5 W1 完成标准。” |
| W2 | “只负责 PDF：同版本 modern/legacy、能力 loader、main-worker 配对和 cleanup；严格遵守 §4 文件边界和 §5 W2 完成标准。” |
| W3 | “只负责 build/CI：Safari 15 target、Reader 产物转译、兼容/体积门禁和 minimumSystemVersion；严格遵守 §4 文件边界和 §5 W3 完成标准。” |
| W4 | “只负责 fixture/测试/QA：建立自动化与 macOS 12 真机任务书，不越权修改 W1-W3；严格遵守 §4 文件边界和 §5 W4 完成标准。” |

主代理不得再派第五个子代理。评审、冲突解决、最终 vendor regeneration、全量验证、commit、push 和 release
均由主代理自己完成。
