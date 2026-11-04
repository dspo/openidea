# openidea-gui 路线图 M5–M8：Go 开发者的日用 IDE

> 承接 M0–M4（IME 门 → 工作台 → 编辑器 → LSP 智能层 → 运行面/Git）。
> 分支链：`spike/gpui-ime-m0` → `feat/gpui-m1-workbench` → `feat/gpui-m2-editor`
> → `feat/gpui-m3-insight` → `feat/gpui-m4-run-git` →（本文档所在分支）→ M5 起。

## 总目标

把 `apps/openidea-gui` 从「已验证的 GPUI 宿主」推进到 **Go 开发者的日用 IDE**。
单一语言深耕 Go（业务语言），编辑器手感齐、gopls 体验完整、壳顺手、长期稳定。
最终判据是 M8 出口条件：**作为默认编辑器连用一周，零 P1**。

选 Go 而非 Rust/Python 的理由：用户最熟悉、业务在用，dogfood 真实度最高；
LSP 层自 M3 起就是经 `languages/*.toml` 驱动的通用实现，gopls 只是换一份配置，
语言广度（Go/Rust/Python 以外的 60+ 语言）留作后续低成本冒烟，不在本轮。

## 边界（明确缓建清单）

以下一律缓建，出现在任何里程碑的实现里即为越界：

- GitHub PR、Maven/Gradle 构建链、test runner 树（M4 的 run 配置可跑 `go test`
  命令行，但没有测试树 UI）
- 工具家族：DB client、HTTP client、Docker、scratch、TODO view
- 调试器 UI（DAP/delve 已在 `languages/go.toml` 配好，UI 缓建）
- 集成终端（维持既有决定；`ide-terminal` + steer-ghostty 路线见 M4-REPORT）
- 多语言广度矩阵、notebook/agent/remote/spell 等 post-first-commit 项
- 主题 token 系统与 i18n（维持既有决定：`theme.rs` 硬编码 + `strings.rs` 常量）

## 方案总览

| 里程碑 | 主题 | 核心问题 | 量级参照 |
|---|---|---|---|
| M5 | 编辑器成熟 | 写代码的手感齐了吗（语言无关底座） | ≈M2 |
| M6 | Go 开发体验深度 | gopls 全量体验接通了吗 | ≈M2 |
| M7 | 工作台打磨 | 壳本身够不够顺手 | ≈M3 |
| M8 | 日用化收官 | 能不能当默认编辑器 | ≈M3–M4 |

M5 与 M6 的切分逻辑：M5 是任何语言都受益的编辑器底座（M6 的依赖），
M6 是 Go 语言智能的数据面与体验层。两者都不碰工作台壳（M7）。

---

## M5 — 编辑器成熟（语言无关底座）

**目标**：写代码的本地手感补齐。做完这一步，编辑器在「无 LSP」维度上不再有欠账。

**范围**：

1. 滚动条：竖 + 横（M2 遗留：只有滚轮/触控板、无水平滚动）。
2. Semantic tokens 叠加：LSP `textDocument/semanticTokens` 作为一个 span 源叠进
   `Highlighter` 的行渲染（M2 情报：`line_runs` 支持叠层，接缝已留）；
   顺带修掉 5 个靠 grammar 谓词着色的关键字。
3. ⌥⏎ code actions 接线：LSP `codeAction` → 菜单 → 应用 workspace edit
   （复用 M3 rename preview 的 workspace-edit 消费路径，走 `Documents::edit` 漏斗）。
4. LSP 格式化（⌘⌥L）：`textDocument/formatting`，Go 文件即 gofmt 语义。
5. 签名帮助：`signatureHelp`，popup 形态复用补全 popup。
6. Live templates / snippets：从 Tauri 前端 `features/templates` 的模型移植为
   app 内 Rust；先内置 Go/Rust 两个模板集，不做模板管理 UI。
7. Hover markdown 渲染（当前纯文本；gpui-component 有现成渲染组件则白拿）。
8. ⌘F 文件内搜索/替换；⇧⌘F Find in Files（走 `ide-search`，结果面板新 dock 页）。
   项目级批量替换缓建。
9. Palette 查询移出 UI 线程（M3 债：Host 请求通道）。
10. **按语言配置缩进**：读 `ide-language` 的 `indent`（`languages/go.toml` 声明
    `tab-size = 4, insert-spaces = false`）——Go 用 Tab，编辑器当前写死空格，
    必须改为按文档语言取缩进策略。

**出口条件**：在 `fixtures/go-app` 完成真实循环——snippet 展开 → 编辑（Tab 缩进正确）
→ ⌘⌥L 格式化 → semantic tokens 生效 → ⇧⌘F 搜全项目并跳转。

---

## M6 — Go 开发体验深度

**目标**：gopls 的体验完整接进编辑器，达到「Go 开发者愿意切换」的程度。

**范围与关键实现细节**：

1. gopls 生命周期泛化验证：M3 的 `LspManager` 已按 `languages/*.toml` 驱动
   （rust-analyzer 只是第一个实例）。gopls 用 `languages/go.toml` 现成配置：
   `root-markers = ["go.work", "go.mod"]` 决定项目根与多模块感知。
   `which gopls` 检查，缺失按 toml 的 `install-hint` 安装（本机工具，不动仓库）。
2. **保存时自动整理 import**（Go DX 的命门）：`willSave`/`waitUntil` +
   `source.organizeImports` code action，⌘S 路径上先跑整理再落盘。
3. Inlay hints：LSP 3.17 `inlayHint`/`resolve`——参数名、类型提示，视口驱动
   按需请求，随滚动增量取（对齐 M2 高亮的视口策略）。
4. Quick fixes 数据面：gopls 的 add import、fill struct、simplify 及
   `gopls.*` commands（`workspace/executeCommand` 通路），统一进 M5 的 ⌥⏎ 面板。
5. 诊断升级：related information 多跳列表、suggestion 一键应用、
   go vet/staticcheck（经 gopls 配置）标注区分。
6. Go to Symbol：`workspace/symbol`，进 palette 的符号分组。
7. 跨包 rename / find usages 正确性日测（Go 的大小写可见性、嵌入结构体字段）。
8. 诊断/索引状态：gopls 初次索引大仓库时的状态栏进度（避免「假死感」）。

**出口条件（dogfood 自举）**：在 `fixtures/go-app` 与用户业务仓库的 scratch 副本上，
完成一个真实开发循环：加一个函数 → 补全（含包限定建议）→ 保存（import 自动整理）
→ inlay hints 常开 → 跨包 rename → ⇧⌘F 验证改动波及点。

---

## M7 — 工作台打磨

**目标**：壳从「能用」到「不看文档能自然上手」。

**范围**：

1. 启动项目选择器 / welcome（替换 env/cwd 推导，M1 遗留）：最近项目列表 +
   打开目录对话框。
2. Dock 系统补全：面板显隐（⌘1/⌘4…）、拖拽调宽、布局持久化（重启还原）、
   多输出 tab（M4 情报：`render` 的 `when_some(run_panel)` 槽位即扩展点）。
3. Tab 管理：pin、⌘⇧T 恢复关闭、溢出滚动。
4. 右键上下文菜单：文件树（新建/重命名/删除，走 `ide-vfs`）与编辑器
   （跳转/格式化/重构入口）。
5. 通知/toast 系统：自动消失、可堆叠（清 M1 债：状态消息永不消失）。
6. 最小 Settings：字体大小、缩进展示（默认随语言）、键位表只读列表。
7. 焦点与 Esc 链系统性梳理：overlay → popup → panel → editor 的统一后撤序
   （M3 曾因 overlay 焦点悬死导致全部 action 静默失效，需要防御性回归测试）。

**出口条件**：新用户不读文档完成「打开项目 → 编辑 → 跑 → 提交」全流程；
打磨前后截图对比进报告。

---

## M8 — 日用化收官

**目标**：稳定性与性能达到「敢设为默认编辑器」。

**范围**：

1. 性能预算扩展（perf HUD 已有，扩成预算表）：gopls 常开时击键→像素延迟、
   补全 popup 首帧、大仓库（业务仓库体量）索引期间的滚动帧率。
2. 长时 soak：连续日用一周，issue 收集（P1/P2 全修，P3 列清单）。
3. 稳定性：panic hook（崩溃不静默）、unwrap 审计、窗口/tab 状态恢复。
4. 回归 harness 固化：截图基线 + 性能基线，本地一条命令可跑。
5. 收官报告：按「日用 Go IDE」逐项验收，附完整缓建清单复核。

**出口条件**：用户把它设为默认编辑器连用一周，零 P1。

---

## 协作流程与纪律（不变）

沿用 M0–M4 的既定流程：code-deliver 派单；每 phase 一提交、提交时全绿
（fmt + clippy `-D warnings` + 单测）；实跑验证 + 窗口级截图（绝不截全屏）；
clean-room（Zed 仅编码前参考，笔记先行，实现只依据笔记 + 本仓 crate +
Apache 的 gpui-pre/gpui-component）；**止步本地：禁止 `git push`、禁止
`gh pr create`**；每里程碑交付 REPORT + 人工清单；新 worktree 基于上一里程碑
分支（M5 基于 `feat/gpui-m4-run-git`），前序 worktree 勿动；
主 workspace 不受影响（`cargo metadata --no-deps` member 数不变、根
`Cargo.lock` 零 diff）；只杀自己启动的进程。

## 附录：接缝情报索引

| 来源 | 内容 |
|---|---|
| M1-REPORT | `Documents`/`DocPatched`（按 version 抑制回声）、vfs watch、bus |
| M2-REPORT | `Highlighter` span 叠层、视口虚拟化、大文件只读策略、性能测量法 |
| M3-REPORT | `LspManager`（tokio runtime 捕获）、action registry/`bind_keys`、palette、焦点 bug 史 |
| M4-REPORT | dock 槽位、`RunPanel` uniform_list、`ide_vcs::Repo`、diff overlay、终端路线 |
| `languages/go.toml` | gopls 配置、root markers、缩进策略、delve（缓建） |
| `fixtures/go-app` | dogfood 目标 |
