# openidea-gui 路线图 M9–M14（本期修订版）：IDEA 习惯面 + Go/Rust/Python·TS/JS 本地开发体验

> 承接 M0–M8（IME 门 → 工作台 → 编辑器 → LSP → run/git → 编辑器成熟 → Go 体验 → 工作台打磨 → 日用化）。
> 分支链末端：`feat/gpui-m8-daily-driver`（= origin/Native tip）。

## 总目标与本期焦点

**openidea 的最大价值是对 JetBrains IDEA UI 与使用习惯的复刻；GPUI 版必须达到同等的
UI 与习惯一致性。** 本期在完整对标第一个 big bang commit（`9a2025b7`）的长线路径上，
聚焦两件事：

1. **IDEA 习惯面**：键位（221 条 intellij-macos 预设）、菜单栏、工具窗 stripe 体系、
   状态栏 widgets、编辑/导航微习惯——它们是所有功能的放大器，优先级最高。
2. **五语言本地开发体验**：Go（M6 已深耕）→ Rust、Python、TS/JS/Node 拉齐到同一
   基线：LSP/调试/测试/格式化/重构全通，纯本地，不依赖远端服务。

## 边界（本期暂缓 ≠ 不做，后续路线回收）

- **Maven/Gradle sync 与 Java 构建链**（本期不做 Java）
- **GitHub PR**（本期只做本地开发体验；Git 本地全量照做）
- **DB / HTTP client / Docker 工具家族**、**WASM 插件 UI**、**60+ 语言广度冒烟**、
  **zh 双语**、**主题切换系统**
- post-first-commit 能力（notebook/agent/remote/spell/web 视图）、Windows/Linux 平台

## 验收权威清单（本期版）

1. `intellij-macos` 221 条键位（`src/platform/keymaps/intellij-macos.ts`）逐条对账
2. 工具窗：stripe 体系建齐 + **本期窗口集**点亮（project/structure/bookmarks/commit/
   terminal/run/debug/problems/find/usages/todo/git/coverage→测试与检查相关窗）；
   缓建窗口（DB/GitHub/Docker/Maven/Gradle/profiler/inspection-results…）保留注册
   数据位、不点亮
3. 五语言 DX 矩阵：每语言「检测/注释/高亮/语义 tokens/LSP 生命周期/补全/跳转/重命名/
   格式化/inlay hints/quick fixes/调试/测试/运行」列项打勾
4. 工作台形态：macOS 全菜单、状态栏 widgets、Commit 工具窗、trust 对话框

## 差距评估摘要（对照第一个 commit，2026-11-05）

习惯面（键位/菜单/工具窗/状态栏/微习惯）= 大差距；功能家族（终端/调试/测试树/VCS
本地面）= 大差距；五语言里 Go 已达 M6 基线、Rust 顺带可用、Python/TS 未接；
编辑器内核/LSP/性能已超日用基线。详细矩阵见 git 历史前版本文档。

---

## M9 — 键位与导航习惯层（≈M2 量级）

**目标**：以 221 条 intellij-macos 绑定为验收表，键位与导航习惯对齐 IDEA。

**范围**：
1. 键位体系升级：preset 数据移植（intellij-macos 全量，intellij-windows/vscode 数据
   一并入库存备用）、用户覆盖、双击 chord、物理键位匹配（macOS ⌥ 符号问题）。
2. 编辑微习惯：⌘D 复制行/选区、⌘/ 与 ⌘⇧/ 注释、⌥↑/⌥↓ 扩展/收缩选区、⌘⇧↑/↓
   移动行等（按 221 清单编辑段逐条对账）。
3. 导航习惯：⌘E 最近文件、⌘⇧E 最近位置、⌘[/⌘] back/forward、F2/⇧F2 错误跳转、
   F3/⌘F3 书签、⌘F12 文件结构 popup、⌃⇥ Switcher、⌘L 行跳转、⌘⌥B/⌘⇧B、
   ⌘⇧⌹ 最后编辑位置。
4. **放开后端约束第一处**：`ide-core::Diagnostic` 加 related information +
   `ide-lsp` 转换（单独提交，Tauri 版同受益），诊断 tooltip 多跳落地。
5. 缺功能的 action 先注册占位（Find Action/键位表可见），221 清单逐条标注
   done/placeholder/na。

**出口**：221 条对账表；真实导航流（back/forward、recent files、书签、错误跳转、
switcher）实跑 transcript。

## M10 — 工作台结构层（≈M2–M3 量级）

**目标**：工作台结构对齐 IDEA。

**范围**：
1. macOS 菜单栏全 IDEA 结构（File/Edit/View/Navigate/Code/Refactor/Build/Run/
   Tools/VCS/Window/Help——缓建项的菜单条目灰色或隐藏），全部经 action registry 生成。
2. 工具窗系统重构为 contributions 模型（对齐 Tauri 版 `contributions.ts` 语义）：
   左/右/下三边 stripe、图标、badge（Problems 计数等）、`stripe: shown/onUse`、
   header ⋮ 菜单（Move to/Maximize/Hide + 窗自定义项）、⌘⇧+数字显隐。
   **体系 + 本期窗口集**点亮；缓建窗口留注册数据位不渲染。
3. 状态栏 widgets 化：line:col、错误计数、encoding、行分隔符、VCS 分支、语言服务
   状态；右键显隐菜单。
4. **Commit 工具窗**（左侧，IDEA 2020+ 习惯）：changelist 勾选 + diff 预览 + 消息
   历史；⌘K 对话框保留直达。
5. Project trust 对话框；Structure 工具窗（⌘F12 窗形态）；Bookmarks 工具窗。

**出口**：stripe 交互与 Tauri 版并排对照截图；菜单全结构截图；trust/commit 窗实跑；
状态栏 widget 显隐菜单实测。

## M11 — 终端 + 调试器（≈M2+M3 量级，预计续传一次）

**目标**：补齐本地开发体验的两个重器；调试覆盖五语言。

**范围**：
1. 终端：`vte` 入 `ide-terminal`，Terminal 工具窗（bottom，"Local × + ▾" header
   形态、多 tab、随前台会话 cwd）；自绘 cell grid 渲染。
2. DAP 调试器：gutter 断点（⌘F8）、Debug 工具窗（Variables/Watches/Call Stack/
   Console）、F7/F8/F9、当前行高亮、条件断点；launch/attach 并入 run 配置体系。
   适配器以 `languages/*.toml` 的 `[debugger]` 声明为准：Go=delve（已配）、
   Rust=codelldb/lldb-dap、Python=debugpy、TS/JS/Node=js-debug；**缺失的 toml 条目
   补齐（单独提交，含 install-hint）**，本机按 hint 安装。
3. Services 工具窗最小面（运行中会话列表 + 日志跟随，run/debug 会话统一入列）。

**出口**：五语言各一条「断点→单步→看变量→继续」实跑（fixtures 或 scratch）；
终端多 tab + 交互 transcript；⌃R/Debug 会话在 Services 窗可见。

## M12 — 测试树 + 检查（≈M3 量级，本期无构建链）

**目标**：IDEA 的测试与检查体验（构建链按边界暂缓；go build/cargo check 走既有
run 配置即够用）。

**范围**：
1. 测试树：Run/Debug 工具窗的树形态（套件/用例/状态图标/时长/重跑）、编辑器
   gutter 的运行单测标记、失败跳转 + 实际值/期望值对照。适配器（app 侧 parser +
   `ide-run` 执行）：
   - Go：`go test -json`
   - Rust：`cargo test`（nextest JSON 若可用优先，否则解析 libtest 输出）
   - Python：`pytest --junitxml`（或 `-v` 解析）
   - TS/JS/Node：vitest / jest 的 JSON reporter
   Java 走 `ide-build` 测试模型——**本期暂缓，接缝留注释**。
2. Problems 工具窗：诊断聚合（编辑器 + 全项目两级）、过滤、F2 导航联动。
3. Inspections：`ide-analysis` 重复代码检查接入（InspectionResults/Duplicates 窗）、
   与 Problems 的分级展示。

**出口**：五语言各自的测试树绿红态 + 失败跳转实跑（fixtures 建对应测试样例，不
弄脏仓库）；重复检查出结果可跳转。

## M13 — 本地 VCS 全量（≈M3 量级）

**目标**：VCS 从最小面到第一个 commit 的本地全量（GitHub 按边界暂缓）。

**范围**：
1. Git 工具窗（bottom）：log 历史 graph（分支拓扑着色）、branches popup
   （新建/切换/删除/rebase 最小集）、blame 注释模式 + 行内 blame、冲突解决最小 UI
   （ours/theirs）、changelists。
2. 行内 blame 开关；Commit 工具窗（M10）与 Git 窗共享 `ide-vcs::Repo` 数据一致性。
3. Find Usages 工具窗（bottom）：结果树形态（M3 jump list 的窗化）。

**出口**：scratch 仓库走完分支操作 + 历史浏览 + blame + 冲突解决；usages 窗树形
浏览 + 分组跳转实跑。

## M14 — 五语言深耕收官 + 产品形态补齐（≈M8 量级，可拆两次派单）

**目标**：把 Rust/Python/TS/JS/Node 拉齐到 Go 的 M6 基线；产品形态收官。

**范围**：
1. **五语言 DX 拉齐**（矩阵逐项：检测/注释/高亮/语义 tokens/LSP 生命周期/补全/
   跳转/重命名/格式化/inlay hints/quick fixes/调试/测试/运行）：
   - LSP/服务器：rust-analyzer（已通）、pyright 或 basedpyright + ruff、
     typescript-language-server 或 vtsls；按 `languages/*.toml` 走，缺配置修 toml
     （单独提交，先例：go.toml 的 semanticTokens/hints 开关）。
   - 每语言一节「实测脾性」进报告（gopls 的教训表明每家 LSP 都有暗坑：
     能力默认关、静默降级、位置语义差异）。
   - 抽样验证重命名正确性（Python 动态性、TS 的 js/ts 边界）。
2. **产品形态补齐**：TODO 视图（`ide-search` TODO 扫描）、scratch files、
   viewers 最小面（图片/只读文本）、welcome 页对齐（最近项目/打开）。
3. **Settings 扩容**：keymap 页（预设切换/覆盖/**录制器**，对齐 Tauri 版），
   其余页只读起步。
4. **收官对账**：221 键位表 × 本期工具窗集 × 五语言 DX 矩阵三方清单逐项打勾；
   与 Tauri 版并排截图对照；产出收官报告（残余差距 + 下一期路线建议：Maven/
   Gradle、GitHub、工具家族、WASM 插件、多语言广度、zh 双语如何回收）。

**出口**：三方清单全勾（或显式 na）；每语言至少一条真实开发循环 transcript；
keymap 录制器实测改键并生效。

---

## 协作流程与纪律（延续 M0–M8）

code-deliver 派单；每 phase 一提交、提交时全绿（fmt + clippy `-D warnings` + 单测）；
实跑验证 + 窗口级截图（绝不截全屏）；clean-room（Zed 不读或仅编码前参考+笔记先行）；
**止步本地：禁止 `git push`、禁止 `gh pr create`**（推送由主会话统一到 origin/Native）；
每里程碑 REPORT + 人工清单；新 worktree 基于上一里程碑分支；前序 worktree 勿动；
主 workspace 不受影响——**例外**：小范围、单独提交、显著说明的后端改动允许
（related-info 等以 Tauri 版同受益为准则）；长命令分开跑；只杀自己启动的进程。

## 附录：验收权威文件

| 清单 | 第一个 commit 路径 |
|---|---|
| README 九条宣称 | `git show 9a2025b7:README.md` |
| 键位预设（221 条） | `src/platform/keymaps/intellij-macos.ts` |
| 工具窗注册与扩展点语义 | 各 feature `registerToolWindow` / `src/workbench/contributions.ts` |
| 语言定义 | `languages/*.toml`（五语言为本期验收面） |
