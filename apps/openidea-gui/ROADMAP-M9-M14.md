# openidea-gui 路线图 M9–M14（fork 原则修订版）：IDEA 习惯面 + 五语言本地 DX

> 承接 M0–M8。分支链末端：`feat/gpui-m8-daily-driver`（= origin/Native tip）。

## 总目标与本期焦点

**openidea 的最大价值是对 JetBrains IDEA UI 与使用习惯的复刻；GPUI 版必须达到同等的
UI 与习惯一致性。** 本期聚焦：①IDEA 习惯面（键位/菜单/工具窗/状态栏/微习惯）；
②五语言本地开发体验（Go 已深耕 → Rust/Python/TS/JS/Node 拉齐）。

## Fork 原则（本路线图的第一纪律）

本项目是 fork：**长期同步上游（l0ng-ai/openidea）、不向上游贡献**。因此：

1. **后端树目标零 diff**：`crates/`、`languages/`、`src/`、`src-tauri/`、`wit/`、
   `fixtures/`、根 `Cargo.lock` 与根 `Cargo.toml` 的 workspace 部分——除非结构性
   必需，不改动；所有 GPUI 前端实现一律在 `apps/openidea-gui` 独立 crate 内。
2. **现状盘点（2026-11-05）**：我方后端漂移仅 3 处——根 `Cargo.toml` 的
   `exclude = ["apps/openidea-gui"]`（结构性必需，保留）、`languages/go.toml` 两行
   （gopls semanticTokens / inlay hints 开关）。**M9 将把 go.toml 两行回迁 app 侧**
   （LSP server options 覆盖层），恢复 `languages/` 与上游零 diff。
3. **上游同步是周期性工作**：fork 基点已落后 upstream/main（ide-vcs log/history、
   ide-lsp、ide-workspace 等大改）。**每里程碑 phase 0 = 同步 upstream/main**；
   首次同步（M9 phase 0）工作量最大。同步冲突与修复一律发生在 app 侧。
4. **防腐层（adapter）纪律**：UI 代码不得散点 `use ide_*`——后端访问统一经 app 内
   `host.rs`/service 层收敛，上游 API 变动的波及面集中到一处文件集。
5. 后端改动默认禁止；此前计划中「related-information 需改 ide-core/ide-lsp」一项
   **撤销、回缓建清单**（等上游实现，或届时评估独立 shim crate，不动上游文件）。

## 边界（本期暂缓 ≠ 不做）

Maven/Gradle 与 Java 构建链；GitHub PR；DB/HTTP/Docker 工具家族；WASM 插件 UI；
60+ 语言广度冒烟；zh 双语；主题切换；post-first-commit 能力（notebook/agent/
remote/spell/web 视图）；Windows/Linux 平台；related-info（受 fork 原则约束）。

## 验收权威清单（本期版）

1. `intellij-macos` 221 条键位逐条对账（done/placeholder/na）
2. 工具窗：stripe 体系 + 本期窗口集点亮；缓建窗口留注册数据位不渲染
3. 五语言 DX 矩阵：检测/注释/高亮/语义 tokens/LSP 生命周期/补全/跳转/重命名/
   格式化/inlay hints/quick fixes/调试/测试/运行
4. 工作台形态：macOS 全菜单、状态栏 widgets、Commit 工具窗、trust 对话框
5. **Fork 卫生**：每里程碑结束时 `git diff upstream/main..HEAD -- crates/ languages/
   src/ src-tauri/ wit/ Cargo.lock` 中我方改动仅剩根 exclude 一行

---

## M9 — 上游同步 + 键位与导航习惯层（≈M2+ 量级）

**目标**：完成首次上游同步并建立同步机制；以 221 条绑定对齐 IDEA 键位与导航习惯。

**范围**：
0. **首次上游同步**：merge upstream/main 进链（上游 ide-vcs/ide-lsp/ide-workspace
   大改，app 修复集中在 host.rs/适配层）；同步后全量回归 M0–M8 行为；把 go.toml
   两行回迁 app 侧 server options 覆盖层。**此后每里程碑 phase 0 重复小规模同步**。
1. 键位体系升级：intellij-macos 221 条 preset 数据移植（win/vscode 数据一并入库
   备用）、用户覆盖、双击 chord、物理键位匹配（macOS ⌥ 符号）。
2. 编辑微习惯：⌘D 复制行/选区、⌘/ 与 ⌘⇧/ 注释、⌥↑/⌥↓ 扩展/收缩选区、⌘⇧↑/↓
   移动行等（按 221 清单编辑段对账）。
3. 导航习惯：⌘E/⌘⇧E 最近文件/位置、⌘[/⌘] back/forward、F2/⇧F2 错误跳转、
   F3/⌘F3 书签、⌘F12 文件结构、⌃⇥ Switcher、⌘L、⌘⌥B/⌘⇧B、⌘⇧⌹。
4. 缺功能的 action 先注册占位（Find Action/键位表可见）。

**出口**：上游同步完成且回归全绿；221 条对账表；导航流实跑 transcript；
go.toml 恢复与上游零 diff。

## M10 — 工作台结构层（≈M2–M3 量级）

（内容同前版：菜单栏全结构、工具窗 stripe 体系 + 本期窗口集、状态栏 widgets、
Commit 工具窗、project trust、Structure/Bookmarks 窗。实现全部在 app crate；
工具窗 contributions 模型对齐 Tauri 版 `contributions.ts` 语义，但为 app 内独立
实现，不 import 上游前端代码。）

**出口**：stripe 与 Tauri 版并排对照截图；菜单全结构截图；trust/commit 窗实跑。

## M11 — 终端 + 调试器（≈M2+M3 量级，预计续传一次）

（内容同前版：vte 入 app 侧接线 + Terminal 工具窗；DAP 断点/单步/变量/条件断点；
Services 窗最小面。**fork 修订**：`vte` 依赖加在 app crate，不改 `ide-terminal`；
调试适配器以 `languages/*.toml` 现状声明为准——rust=codelldb、python、ts/js 的
LSP+debugger 均已声明，**目标零 toml 改动**，确需改的先评估同步冲突。）

**出口**：五语言各一条断点调试实跑；终端多 tab transcript；会话入 Services 窗。

## M12 — 测试树 + 检查（≈M3 量级）

（内容同前版：Run/Debug 树形态 + gutter 运行标记 + 失败跳转；适配器 app 侧
parser——go test -json / cargo test(nextest JSON 优先) / pytest junitxml / vitest
jest JSON；Problems 工具窗聚合过滤；`ide-analysis` 重复检查接入。构建链暂缓，
`go build`/`cargo check` 走既有 run 配置。）

**出口**：五语言测试树绿红态 + 失败跳转实跑（fixtures 补样例不入仓库树根）；
重复检查可跳转。

## M13 — 本地 VCS 全量（≈M3 量级，上游同步受益项）

**fork 注意**：上游对 ide-vcs 的 log/history 有大重构——本期开工前先完成例行
上游同步，log graph 的数据面**用上游新 API**（白拿上游成果，这正是同步的价值）。

（内容同前版：Git 工具窗 log graph/branches popup/blame 双形态/冲突最小 UI/
changelists；Find Usages 工具窗；行内 blame 开关。GitHub 暂缓。）

**出口**：scratch 仓库分支操作 + 历史浏览 + blame + 冲突解决全流程；usages 树形
浏览实跑。

## M14 — 五语言深耕收官 + 产品形态补齐（≈M8 量级，可拆两次派单）

（内容同前版：五语言 DX 拉齐——rust-analyzer/basedpyright 或 pyright + ruff/
vtsls 或 typescript-language-server，按 `languages/*.toml` 现状走，**options 差异
走 app 侧覆盖层，目标零 toml 改动**；每语言实测脾性入报告；重命名抽样（Python
动态性、js/ts 边界）；TODO 视图、scratch、viewers、welcome；Settings 的 keymap
页（预设/覆盖/录制器）；收官三方对账 + 下期回收方案。）

**出口**：三方清单全勾（或显式 na）；每语言一条真实开发循环 transcript；
fork 卫生检查（后端树 diff 仅剩 exclude 一行）。

---

## 协作流程与纪律（延续 M0–M8 + fork 原则）

code-deliver 派单；每 phase 一提交、提交时全绿（fmt + clippy `-D warnings` + 单测）；
实跑验证 + 窗口级截图（绝不截全屏）；clean-room（Zed 不读或仅编码前参考+笔记先行；
**同样不得复制上游 Tauri 前端代码**——实现独立，语义对齐）；**止步本地：禁止
`git push`、禁止 `gh pr create`**（推送由主会话统一到 origin/Native）；每里程碑
REPORT + 人工清单；新 worktree 基于上一里程碑分支；前序 worktree 勿动；
**每里程碑 phase 0 同步 upstream/main**；**后端树零 diff（fork 卫生项进每期
REPORT 验收）**；长命令分开跑；只杀自己启动的进程。

## 附录：验收权威文件

| 清单 | 路径 |
|---|---|
| README 九条宣称 | `git show 9a2025b7:README.md`（对照基准，能力子集按本期边界裁剪） |
| 键位预设（221 条） | `src/platform/keymaps/intellij-macos.ts` |
| 工具窗注册与扩展点语义 | 各 feature `registerToolWindow` / `src/workbench/contributions.ts` |
| 语言定义 | `languages/*.toml`（五语言现状为准，目标零改动） |
| 上游 | `upstream` = `https://github.com/l0ng-ai/openidea.git` |
