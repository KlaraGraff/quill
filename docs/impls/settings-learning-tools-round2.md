# 设置与学习工具第二轮修复方案(Round 2)

> 状态:**已实施**(2026-07-15;图1 根因修复、预览交互、模块开关与自定义草稿守卫均已完成并通过前后端验证)。
> 范围:AI 服务连接测试(图1)、卡片预览滚动与模块动画(图2)、模块启用开关外置(图3/图4)、自定义模块草稿化保存(图3 附加需求)。
> 基线:main @ v1.5.2(含 PR #2 squash 合并 6fbce2d)。

---

## 0. 确认结论

- **Q1(图1 诊断)已回复**:所见文案即折叠徽标"不可用 · 1ms";"实际可用"指阅读中双击查词等 AI 功能能正常调用该 API;没有见过密钥迁移弹窗。
  → 结论:密钥可读、真实链路正常,"密钥迁移"假设排除。据此重查测试路径与真实路径的差异,**根因已在代码中坐实**:连接测试的取消通道发送端被立即丢弃,导致测试在发出任何网络请求前就被误判为"已取消"(详见 §1.2)。原 §1.C"迁移横幅"取消。
- **Q2 ~ Q4:按推荐执行**(滚动机制重写 + 消失动画 + 自定义模块占位;新增自定义模块改为空字段+占位提示;操作菜单自定义动作同样草稿化)。
- **Q5(手风琴)未明确回复**:按推荐(改为手风琴式)执行;如不需要请在实施前说明。

---

## 1. 图1:AI 服务连接测试立即返回"不可用 · 1 ms"

### 1.1 现状解读

- 徽标文案由 [AiServiceCard.tsx:119](../../src/components/settings/AiServiceCard.tsx) `profileHealth` 根据持久化的 `profile.state` 生成;`1 ms` 是 `last_latency_ms`,**只有连接测试会写入这个值**([router.rs:2267](../../src-tauri/src/ai/router.rs) 传 `Some(total_ms)`;真实对话路径传 `None`)。1ms 完成意味着测试**未发出任何网络请求**。
- 用户确认:真实查词链路正常(密钥可读、服务可用),唯独测试按钮必然失败 → 差异只能在测试独有的代码路径上。

### 1.2 根因(已核实,自 v1.4.0 b37845d 引入)

**连接测试的取消通道一出生就是死的。**

- 测试专用封装 `timed_stream_once`([router.rs:1971](../../src-tauri/src/ai/router.rs)):

  ```rust
  let mut cancel = watch::channel(false).1;   // 只留接收端,发送端当场 drop
  ```

- `stream_once` 内部用 `tokio::select!` 同时等待流结果与 `cancel.changed()`([router.rs:637](../../src-tauri/src/ai/router.rs))。tokio watch 通道的约定是:**发送端被 drop 后,`changed()` 立即返回 `Err`**。于是 select 立刻命中取消分支,请求在发出任何 HTTP 之前就返回 `AI_REQUEST_CANCELLED`(约 0–1ms)。
- `classify_error` 没有"已取消"分类,`ai_request_cancelled` 匹配不到任何模式,**兜底归为 `Network`**([router.rs:214](../../src-tauri/src/ai/router.rs))→ 可重试 → 逐个密钥同样瞬间失败 → 总耗时 1ms,并把 profile 写成 `cooldown`(30 秒)——即 deqiyun 的「暂时冷却 · 1 ms」。DeepSeek 的「不可用」是同一 bug 之上的另一种 state 落点(以 §1.3.A 的逐次明细为准确认)。
- 真实链路(`stream_with_failover`)的取消接收端来自 `cancellation_registry`(发送端存活于注册表,[router.rs:231](../../src-tauri/src/ai/router.rs)),所以完全正常。这正是"查词可用、测试必挂"的原因。
- 同一坏模式的隐患点还有 3 处 fallback:`.unwrap_or_else(|| watch::channel(false).1)`([router.rs:849/917/1062](../../src-tauri/src/ai/router.rs)),仅在调用方不传 `request_id` 时触发,一并修复。
- `ai_test_credential`(单密钥测试)同样经由 `timed_stream_once`,同样中招。

### 1.3 方案

**A0. 根因修复(最优先)**

1. `stream_once` 增加防御性等待助手,把"发送端已消失"视为"不可能被取消"而非"已取消":

   ```rust
   async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
       if cancel.changed().await.is_err() {
           std::future::pending::<()>().await; // 发送端已 drop:永不取消
       }
   }
   // select! 中改等 wait_cancelled(cancel)
   ```

   此改动同时治愈 `timed_stream_once` 与三处 fallback,含未来同类调用。
2. `timed_stream_once` 仍显式持有发送端(`let (_cancel_guard, mut cancel) = watch::channel(false);`),语义自明,不依赖防御逻辑。
3. `classify_error` 显式识别 `AI_REQUEST_CANCELLED`(新增 `Cancelled` 类别):取消不写入健康状态(不进冷却、不记为 network),连接测试与真实链路一致处理。
4. 回归测试:`wait_cancelled` 死通道永挂起 / 活通道发送后立即返回;`classify_error("AI_REQUEST_CANCELLED")` 不再是 `Network`。
5. 修复后无需数据修补:下次测试成功即把 state 写回 `active`,错误的"冷却/1ms"自愈。

**A. 测试结果可诊断化(仍建议做,防止下一次"盲猜")**

- `AiConnectionTestResult` 增加逐次尝试明细:

  ```rust
  pub struct AiConnectionTestAttempt {
      credential_id: Option<String>,
      credential_label: Option<String>,
      error_kind: Option<String>,
      error_detail: Option<String>, // 净化+截断(≤300字符,不含密钥)
      latency_ms: u64,
      request_sent: bool,           // false = 未发出任何 HTTP 请求
  }
  ```

- 展开卡片的失败提示条改为:总结果 + 每个密钥一行(标签、原因、耗时);`request_sent=false` 时明确显示 **「未发出网络请求:密钥无法读取,请重新输入或完成迁移」**。
- 折叠态徽标失败时附带原因短语(复用现有 `CONNECTION_ERROR_KEYS` 文案):`不可用 · 密钥无效`、`不可用 · 未配置密钥`,替代裸的"不可用 · 1 ms"。

**B. 状态语义修正(必做)**

- `update_profile_health`:`CredentialInvalid` 不再写 `cooldown`(现在会打 5 分钟冷却,产生"暂时冷却"误导,且真实路由 5 分钟内直接跳过该服务)。改为 state=`invalid`,不写 `cooldown_until`;i18n 新增 `settings.ai.health.invalid`(zh:密钥无效)。路由端影响:该服务仍会被尝试,读取失败零网络成本即跳到下一服务,可接受且自愈。
- `NotConfigured` 徽标文案从"不可用"改为"未配置密钥"。

**~~C. 迁移入口显性化~~(已取消)** — Q1 确认真实链路正常、密钥可读,迁移假设排除。

**验收**
- deqiyun / DeepSeek 点击测试:发出真实请求,成功时显示可用 + 真实首响/总耗时(明显 > 1ms);失败时给出真实的服务端原因。
- 单密钥测试(`ai_test_credential`)同样恢复。
- 测试成功后徽标从"暂时冷却/不可用 · 1ms"自愈为"可用 · 实测耗时"。
- 人为清空密钥:文案为"密钥无法读取…",徽标"密钥无效",不进入"暂时冷却";无启用密钥:徽标"未配置密钥"。
- `cargo test --lib`:新增 `wait_cancelled`、`classify_error` 取消分类、attempt 序列化与 health 状态映射用例。

---

## 2. 图2:预览滚动不灵敏/不流畅、模块无消失动画、模块"不显示"

### 2.1 根因

1. **同一模块二次操作必不滚动**:滚动由 `lastTouchedId`(字符串)驱动([CardPreview.tsx:200](../../src/components/settings/CardPreview.tsx)),`ToolsSettings` 里 `setLastTouchedId(id)` 传相同 id 时 state 不变、effect 不重跑。典型场景正是图2:先关"句中语法作用"再开 → 第二次不滚动 → 模块停在卡片折叠视野外 → 看起来"没有展示"。开/关"默认展开"、连续点同一模块的上下移动同理。这就是"很不灵敏,有时候不滚动"。
2. **固定 300ms 定时 + `scrollIntoView`**:目标未挂载/流式渲染未到位时直接落空;`scrollIntoView` 会滚动**所有可滚祖先**(包括 `overflow-hidden` 的预览外框和设置弹窗),产生画面被"推一下"的不流畅感;WKWebView 的 smooth 行为本身也不稳定。
3. **关闭模块 = React 直接卸载**,无任何过渡。
4. **同类问题排查结论**(Q2 相关):内置模块在本地样例里内容齐全,不会缺席;真正会"没有对应模块展示"的是 **自定义模块**——本地样例 `getLearningCardFixture` 没有它们的内容,`ModuleSection` 因 `hasContent=false` 直接返回 null([LearningCardModules.tsx:143](../../src/components/learning-card/LearningCardModules.tsx)),新增/启用自定义模块后右侧预览完全无反馈,只有点"测试/生成真实预览"才出现。操作菜单预览侧(`data-menu-id`)也共享根因 1。

### 2.2 方案

**A. 触发机制改为 `{ id, nonce }`**

- `ToolsSettings` 的 `lastTouchedId: string` → `lastTouched: { id: string; nonce: number }`,每次 `onTouched` 递增 nonce;`CardPreview` effect 依赖 nonce。同一模块重复操作每次都触发。

**B. 滚动重写:只滚卡片内部容器 + 自定义缓动**

- `LearningCardView` 的滚动容器(`overflow-y-auto` div,[LearningCardView.tsx:130](../../src/components/learning-card/LearningCardView.tsx))加 `data-card-scroll` 标记。
- 新增 `scrollToModule(container, moduleEl)`:计算目标相对容器的居中偏移,rAF + easeInOutCubic(~350ms)动画容器 `scrollTop`;不再调用 `scrollIntoView`,祖先容器不会被牵动。
- 目标查找由"一次 300ms 定时"改为 **rAF 轮询直至挂载(上限 ~1.2s)**,覆盖启用后重渲染、流式渲染、消失动画进行中等时序。
- 新动画开始前取消上一个;用户在容器上滚轮/触摸时立即中止动画(避免抢方向盘)。
- 高亮闪烁(`highlightedId`)逻辑保留,改为滚动完成后触发。

**C. 出现/消失动画**

- `LearningCardModules` 增加 `animateChanges` 开关(仅 `CardPreview` 传 true,真实阅读卡不受影响)。
- 实现:模块列表按"当前启用集 ∪ 正在离场集"渲染;包一层 `Collapsible`(`display:grid; grid-template-rows` 1fr↔0fr + opacity,~240ms),关闭时先播放收起动画,`transitionend` 后再从离场集中移除;开启时从 0fr→1fr 展开。
- 关闭模块时预览的行为链:收起动画 → 不滚动(目标已消失,不做居中,仅在原地播放消失)。

**D. 自定义模块本地占位内容**

- `CardPreview.localResult` 组装时,为当前 kind 每个启用的自定义模块注入占位内容:`{ summary: t("settings.tools.custom.previewPlaceholder", { name }) }`(zh 例:「此处将展示「{{name}}」的 AI 生成内容,点击"测试"或"生成真实预览"查看实际效果。」)。
- 由此自定义模块在本地预览即出现,滚动定位、启用/禁用动画全部生效,消除"没有对应模块展示"这一类问题。

**验收**
- 同一模块连续 关→开→关→开,每次都正确滚动/播放动画。
- 修改"默认展开/密度/排序"同样触发定位。
- 滚动只发生在卡片内部,设置弹窗与预览外框不动;动画肉眼流畅(Tauri WKWebView 下验证)。
- 新增自定义模块立即出现在预览并被定位;操作菜单侧(`data-menu-id`)同样验证。

---

## 3. 图3/图4:模块启用开关外置(对齐"操作菜单-单词"风格)

### 3.1 语义先答复(图3 中的疑问)

"显示此模块"当前的真实语义就是**启用/停用**:关闭后该模块不会进入 AI 请求的模块声明集(`allowedIds` 只收 `enabled`,[CardPreview.tsx:234](../../src/components/settings/CardPreview.tsx);后端按请求声明集校验),不是"生成了但藏起来"。因此不存在"不显示但仍开启"的状态。开关外置后,行右侧 Toggle 即代表启用,不再需要"显示此模块/启用此模块"这行文案。

### 3.2 方案

- [CardModuleRow.tsx](../../src/components/settings/CardModuleRow.tsx) 行结构改为与 [SelectionMenuSettings.tsx:66](../../src/components/settings/SelectionMenuSettings.tsx) 一致:
  `[拖拽柄] [chevron+名称(点击展开)] [↑] [↓] [Toggle(启用)]`
- 展开区删除"显示此模块"行,保留:描述文案、默认展开、内容密度、(自定义模块的)编辑器。
- 停用状态的行:名称与图标降透明度(仍可拖动排序、仍可展开调整密度,与操作菜单行为一致)。
- 展开互斥改为手风琴(见 Q5;`open` 状态从行内 `useState` 上提为 `CardDesignSettings` 的 `openId`,同时是 §4 未保存守卫的挂载点)。
- i18n:删除 `settings.tools.showModule` 的使用(键保留一版本以防回滚),Toggle 的 aria-label 复用 `settings.tools.toggleModule`。

**验收**:折叠态即可开关模块;开关后预览按 §2 定位/动画;行样式与"操作菜单-单词"视觉一致。

---

## 4. 自定义模块草稿化:不输入不保存、离开有提示

### 4.1 现状

"添加自定义模块"点击即落库(预填名称+默认提示词,折叠态,[CardDesignSettings.tsx:179](../../src/components/settings/CardDesignSettings.tsx));编辑已有模块时,`CustomActionEditor` 的草稿只存在组件内,折叠/切换即**静默丢弃**。

### 4.2 方案

**新增流程(草稿模式)**
- 点击"添加自定义模块"不再写 config,只在 `CardDesignSettings` 建立 `draft` 本地状态,并以**展开状态**渲染在列表底部(视觉与正式行一致,可加"未保存"小圆点,复用 AI 服务卡的样式)。
- 字段初始为空 + 占位提示(见 Q3)。
- **丢弃规则**:名称与提示词 trim 后都为空(从未输入,或输入后又清空)时,点击其他模块/切换 tab/关闭设置 → **静默丢弃**,不落库、不弹窗。
- **保存规则**:点"保存"且名称+提示词非空 → 一次性写入 `modules` + `customModules`(即现在 `onChange` 落库的内容),清除草稿,触发 `onTouched` 定位预览。

**未保存守卫(草稿有内容,或编辑已有模块产生未保存修改)**
- `CustomActionEditor` 上报 `dirty`(`draft` 与 `value` 规范化比较)。
- 手风琴切换(点开其他模块)、切换 word/phrase/passage 子 tab、切换设置分区、关闭设置弹窗时,若当前编辑器 dirty → 弹确认对话框:
  - **保存**(名称/提示词有效时可用)/ **放弃修改** / **继续编辑**(取消切换)。
  - 用应用内轻量对话框(新建 `ConfirmDialog`,风格随 `DensityHelpDialog`),不用 `window.confirm`(无法定制三选项;现有 `overwriteConfirm` 的 confirm 顺带迁移)。
- 操作菜单的自定义动作按 Q4 结论同样处理(逻辑放在共用层:守卫状态挂在 `openId` 切换处,`SelectionMenuSettings` 已是手风琴,改动小)。

**验收**
- 添加后不做任何输入 → 点别处,列表无残留、config 无写入。
- 输入一半 → 点其他模块/关设置 → 弹三选项对话框,各分支行为正确。
- 保存后行为与现状一致(可测试、可导入、可同步源)。
- 编辑已有模块改动未保存 → 同样触发守卫(修复现有静默丢失问题)。

---

## 5. 实施拆分与顺序

| 步骤 | 内容 | 主要文件 | 依赖 |
|---|---|---|---|
| 1 | §1.A0 取消通道根因修复(含回归测试) | router.rs | 无(最优先,独立可发) |
| 2 | §2.A/B 滚动触发与滚动重写 | ToolsSettings / CardPreview / LearningCardView | 无 |
| 3 | §2.C 出现/消失动画 + §2.D 占位内容 | LearningCardModules / CardPreview / i18n | 2 |
| 4 | §3 开关外置 + 手风琴 | CardModuleRow / CardDesignSettings / i18n | 2(定位联动) |
| 5 | §4 草稿化 + 未保存守卫 + ConfirmDialog | CardDesignSettings / SelectionMenuSettings / CustomActionEditor / ToolsSettings | 4(手风琴) |
| 6 | §1.A/B 测试诊断化与状态语义 | router.rs / settings.rs / AiServiceCard / AiSettings / i18n | 1 |

前端回归:`npm run lint && npx tsc --noEmit && npm run test:unit`;后端:`cargo check && cargo test --lib`。手动验证按各节"验收"清单,在 Tauri 窗口(WKWebView)而非浏览器中检查动画表现。

## 6. Figma 设计提示(高层)

- **模块行(外置开关)**:一行内左起拖拽柄、chevron+模块名、上移/下移箭头、启用 Toggle;停用态整行降透明;展开区仅剩描述、默认展开、内容密度、自定义编辑器。与"操作菜单-单词"行完全同构。
- **未保存确认对话框**:小尺寸居中卡片,标题"保存对「{name}」的修改?",三按钮:主按钮"保存"、次按钮"放弃修改"、幽灵按钮"继续编辑"。
- **AI 测试失败明细**:展开卡片底部状态条下方逐密钥一行(密钥标签 · 原因 · 耗时),"未发出网络请求"用警示色前缀;折叠徽标失败态为"不可用 · {原因短语}"。
