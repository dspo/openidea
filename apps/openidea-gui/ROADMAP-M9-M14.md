# openidea-gui 路线图 M9–M14：完整对标第一个 big bang commit

> 承接 M0–M8（IME 门 → 工作台 → 编辑器 → LSP → run/git → 编辑器成熟 → Go 体验 → 工作台打磨 → 日用化）。
> 分支链末端：`feat/gpui-m8-daily-driver`（= origin/Native 当前 tip）。

## 总目标与第一价值维度

**openidea 的最大价值是对 JetBrains IDEA UI 与使用习惯的复刻；GPUI 版必须达到同等的
UI 与习惯一致性。** 本轮终点 = 完整对标第一个 big bang commit（`9a2025b7`,
"Initial open-source release"）的产品体验，验收以该 commit 的四份权威清单对账：

1. **README 九条产品宣称**（editor/insight/search/run&debug/build/vcs/tools/languages/plugins）
2. **`intellij-macos` 键位预设：221 条绑定**（`src/platform/keymaps/intellij-macos.ts`）
3. **21 个工具窗注册清单**（project/structure/bookmarks/commit 左；DB/Gradle/Maven/
   Hierarchy/Coverage 右；Terminal/Run/Debug/Build/Problems/FindUsages/Git/PullRequests/
   TODO/Profiler/InspectionResults/Duplicates/Find 下；Services 下）
4. **产品形态细节**：macOS 全菜单结构、状态栏可配置 widgets、stripe 的 badge/
   onUse/⋮ 菜单语义、Commit 工具窗（非对话框）、en/zh 双语、4 套键位预设 +
   录制器、project trust、viewers、欢迎页

## 差距评估（2026-11-05，GPUI = M8 末端）

| 维度 | 第一个 commit | GPUI 现状 | 差距 |
|---|---|---|---|
| 键位体系 | 221 条 intellij-macos + vscode×2 预设 + 覆盖/自定义/chord/录制器 | 自建 registry，绑定数十条量级，无预设体系 | **大** |
| 菜单栏 | IDEA 全菜单结构 | 薄菜单 | **大** |
| 工具窗系统 | 21 窗 + stripe（badge/onUse/⋮/Move to/Maximize）+ ⌘⇧数字 | project/run/find 三个 dock 页，无 stripe 概念 | **大** |
| 状态栏 | 可配置 widgets（显隐菜单、对齐、优先级） | 固定若干项 | 中 |
| 编辑/导航习惯 | ⌘E/⌘⇧E/⌘[⌘]/F2/F3/⌘F12/⌃⇥/⌘⌥B/⌘L/⌘D/⌘//⌥↑↓/⌘⇧↑↓… | M2/M3 覆盖部分 motion 与跳转 | 中大 |
| Commit 形态 | 左侧 Commit 工具窗（IDEA 2020+ 习惯） | ⌘K 对话框 | 习惯差异 |
| 语言 | 60+ via languages/*.toml | Go 深耕 + Rust 顺带验证 | 广度缺 |
| 诊断 | related-information 多跳 + inspections | 单条诊断 + quick fix | 中 |
| 双语 | en/zh text catalog | 英文硬编码（既定缓建） | 中 |
| 功能家族 | DAP/测试树/构建链/VCS 全量/GitHub/DB/HTTP/Docker/TODO/scratch/coverage/profiler/插件/viewers | 全缺（既定缓建） | 大 |
| 编辑器内核/LSP/Go DX | — | **已达成且超额**（性能预算 20/20、soak 0 panic） | 领先 |

结论：**功能内核与单语言体验已超第一个 commit 的日用基线；差距集中在「IDEA
习惯面」（键位/菜单/工具窗/微习惯）与「功能家族广度」**。本轮顺序即按此排定：
习惯面优先（用户价值最高、且是既有功能的放大器），家族广度随后。

## 边界（本轮不再缓建的 + 仍缓建的）

- **本轮纳入**（此前缓建项回归范围）：终端、调试器、测试树、构建链、VCS 全量、
  GitHub、工具家族、多语言广度、插件、zh 双语、related-info 后端放开。
- **仍缓建**：主题切换系统（tokens 多主题）——仅保持现硬编码暗色，收官时按
  需追加；post-first-commit 的能力（notebook/agent/remote/spell/web 视图）不属
  本轮目标；Windows/Linux 平台适配仍不在范围（macOS 单平台）。

---

## M9 — 键位与导航习惯层（≈M2 量级）

**目标**：以 `intellij-macos` 221 条绑定清单为验收表，键位与导航习惯对齐 IDEA。

**范围**：
1. 键位体系升级：preset 数据移植（intellij-macos 全量 + intellij-windows/vscode 两系
   数据一并入库备用）、用户覆盖、双击 chord、物理键位匹配（macOS ⌥ 符号问题）。
2. 编辑微习惯：⌘D 复制行/选区、⌘/ 行注释、⌘⇧/ 块注释、⌥↑/⌥↓ 扩展/收缩选区、
   ⌘⇧↑/↓ 移动行、⌘⌫ 删至行首等（按 221 清单编辑段逐条对账）。
3. 导航习惯：⌘E 最近文件、⌘⇧E 最近位置、⌘[/⌘] back/forward、F2/⇧F2 下一/上一
   错误、F3/⌘F3 书签/书签列表、⌘F12 文件结构 popup、⌃⇥ Switcher、⌘L 行跳转、
   ⌘⌥B/⌘⇧B 实现/类型声明跳转、⌘⇧⌫ 最后编辑位置。
4. **放开后端约束第一处**：`ide-core::Diagnostic` 加 related information +
   `ide-lsp` 转换（单独提交，Tauri 版同受益），诊断 tooltip 多跳落地。
5. 缺功能的 action 策略：先注册 action（可在 Find Action/键位表中出现）后补功能，
   221 清单逐条标注 done/placeholder。

**出口**：`intellij-macos` 221 条逐条对账表（done/placeholder/na）；真实导航流
（back/forward、recent files、书签、错误跳转、switcher）实跑 transcript。

## M10 — 工作台结构层（≈M2–M3 量级）

**目标**：工作台结构对齐 IDEA：菜单栏、工具窗 stripe 体系、状态栏 widgets、Commit 工具窗。

**范围**：
1. macOS 菜单栏全 IDEA 结构（File/Edit/View/Navigate/Code/Refactor/Build/Run/
   Tools/VCS/Window/Help），全部经 action registry 生成。
2. 工具窗系统重构为 contributions 模型（对齐 Tauri 版 `contributions.ts` 语义）：
   左/右/下三边 stripe、图标、badge（如 Problems 计数）、`stripe: shown/onUse`、
   header ⋮ 菜单（Move to/Maximize/Hide + 各窗自定义项）、⌘⇧+数字显隐。
   **21 个注册位全部建齐**：无功能的窗注册但隐藏/占位，后续里程碑逐个点亮。
3. 状态栏 widgets 化：line:col、错误计数、encoding、行分隔符、VCS 分支、语言服务
   状态；右键显隐菜单（Status Bar Widgets 习惯）。
4. **Commit 工具窗**（左侧，IDEA 2020+ 习惯）：changelist 勾选 + diff 预览 + 消息
   历史 + 提交后操作，替代 ⌘K 模态对话框（对话框保留 ⌘K 直达提交）。
5. Project trust 对话框（首次打开外部目录时）；Structure 工具窗（⌘F12 的窗形态）；
   Bookmarks 工具窗。

**出口**：21 工具窗注册位与 stripe 交互对照截图（vs Tauri 版并排）；菜单栏全结构
截图；trust/commit 窗实跑；状态栏 widget 显隐菜单实测。

## M11 — 终端 + 调试器（≈M2+M3 量级，预计续传一次）

**目标**：补齐 IDEA 的两个日用重器。

**范围**：
1. 终端：`vte` 入 `ide-terminal`（模拟后置后端），Terminal 工具窗（bottom，
   "Local × + ▾" header 形态、多 tab、随前台会话 cwd）；渲染走自绘 cell grid。
2. DAP 调试器（delve 已配于 `languages/go.toml`）：gutter 断点（⌘F8）、Debug
   工具窗（Variables/Watches/Call Stack/Console 四页）、F7 步入/F8 步过/F9 继续、
   当前行高亮、断点条件；launch/attach 配置形态并入 run 配置体系。
3. Run/Debug 服务化：Services 工具窗最小面（运行中的会话列表+日志跟随）。

**出口**：Go 程序断点→单步→看变量→继续全流程实跑（fixtures/go-app 或 scratch）；
终端多 tab + 外部命令交互 transcript；⌃R 运行态与 Debug 会话在 Services 窗可见。

## M12 — 测试 + 构建链 + 检查（≈M3 量级×1.5）

**目标**：IDEA 的构建/测试/检查三件套。

**范围**：
1. 测试树：Run/Debug 工具窗的树形态（套件/用例/状态图标/时长）、单测右键运行、
   失败跳转+diff 对照；Go 走 `go test -json` 解析（app 侧 parser + ide-run 执行），
   Java 侧经 `ide-build` 测试模型。
2. Maven/Gradle（`ide-build`）：项目 sync、sync 状态、Build 工具窗输出、构建型
   run 配置；右侧 Maven/Gradle 工具窗（生命周期/依赖树最小面）。
3. Inspections：Problems 工具窗（诊断聚合 + 过滤）、`ide-analysis` 的重复代码
   检查接入（InspectionResults/Duplicates 工具窗）、F2 导航联动。

**出口**：fixtures 的 Java 项目 sync→跑测试→失败跳转闭环；Go 项目测试树绿红态
+失败跳转；重复检查出结果可跳转。

## M13 — VCS 全量 + GitHub（≈M3 量级）

**目标**：VCS 面从「最小面」到第一个 commit 全量。

**范围**：
1. Git 工具窗（bottom）：log 历史 graph（分支拓扑着色）、branches popup
   （新建/切换/删除/rebase 最小集）、blame 注释模式 + 行内 blame、冲突解决
   最小 UI（ours/theirs 选择）、changelists。
2. GitHub PR 工具窗（左，IDEA 习惯位）：PR 列表/diff/评论/merge 操作
   （`ide-github`）；认证经既有 token 流。
3. 行内 blame 开关、⌘K 提交窗与 VCS 窗的数据一致性（同 `ide-vcs::Repo`）。

**出口**：scratch 仓库走完分支操作+历史浏览+blame；PR 列表与一次 merge dry-run
（或对真实仓库只读浏览 + 操作层 mock 验证）。

## M14 — 工具家族 + 语言广度 + 插件 + 收官（≈M4–M8 总和，分两次派单）

**目标**：点亮剩余全部注册位，产品形态收官。

**范围**：
1. 工具家族：DB client（`ide-db` 三库 + SSH 隧道，右窗）、HTTP client
   （`ide-http`，编辑器内 .http 形态 + 响应窗）、Docker（`ide-docker`）、TODO
   视图、scratch files、Coverage、Profiler 工具窗、Hierarchy（调用/类型层级）。
2. 语言广度：60+ 语言按 `languages/*.toml` 全清单冒烟（检测/注释/高亮/LSP 起停），
   矩阵表收官；抽样 5 语言深验（含 Python/Java/Rust/TS +1）。
3. 插件：WASM 插件（`wit/plugin.wit`）加载 UI、插件列表、跑通 `plugins/` 示例。
4. 双语：text catalog（en/zh.json）加载进 GPUI 版，跟随系统语言，`strings.rs`
   常量迁入 catalog。
5. Settings 扩容：keymap 页（预设切换/覆盖/录制器，对齐 Tauri 版）、其余页只读。
6. viewers（图片/文本只读窗）、welcome 页对齐（最近项目/打开/学习入口）。
7. **收官对账**：README 九条 × 221 键位表 × 21 工具窗三方清单逐项打勾，与
   Tauri 版并排截图对照，产出收官报告（含残余差距与后续路线建议）。

**出口**：三方清单全勾（或显式 na 标注）；zh 界面截图；示例插件运行证据。

---

## 协作流程与纪律（延续 M0–M8）

code-deliver 派单；每 phase 一提交、提交时全绿（fmt + clippy `-D warnings` + 单测）；
实跑验证 + 窗口级截图（绝不截全屏）；clean-room（Zed 不读或仅编码前参考+笔记先行）；
**止步本地：禁止 `git push`、禁止 `gh pr create`**（推送由主会话统一到 origin/Native）；
每里程碑 REPORT + 人工清单；新 worktree 基于上一里程碑分支；前序 worktree 勿动；
主 workspace 不受影响——**例外**：M9 起「小范围、单独提交、显著说明」的后端改动
允许（related-info 等以 Tauri 版同受益为准则）；长命令分开跑；只杀自己启动的进程。

## 附录：本轮的验收权威文件

| 清单 | 第一个 commit 路径 |
|---|---|
| README 九条宣称 | `git show 9a2025b7:README.md` |
| 键位预设（221 条） | `src/platform/keymaps/intellij-macos.ts` |
| 工具窗注册（21 个） | 各 feature 的 `registerToolWindow` 调用 |
| 扩展点语义 | `src/workbench/contributions.ts` |
| 双语目录 | `src/platform/locales/{en,zh}.json` |
