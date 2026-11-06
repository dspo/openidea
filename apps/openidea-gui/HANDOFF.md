# HANDOFF — openidea-gui 迁移交接（macOS → Fedora）

> 写给下一台机器（Fedora）上的接手 agent。写作时间：M10 收尾后（分支
> `feat/gpui-m10-structure` = `origin/Native` tip）。本文档带你从零到能派发 M11。
> **权威优先级**：`ROADMAP-M9-M14.md`（路线）> 各 `M*-REPORT.md`（事实）>
> 本文档（索引与流程）> `README.md`（**已过时**，仍停留在 M0 spike 时代，勿依赖）。

## 0. 一页速览

- **是什么**：openidea（Rust+Tauri 的开源 IDE，fork 自 `l0ng-ai/openidea`）的
  GPUI 原生前端宿主 `apps/openidea-gui`。已完成 M0–M10（IME 门→工作台→编辑器→
  LSP→run/git→编辑器成熟→Go DX→打磨→日用化→习惯层/结构层）。
- **在哪**：全部工作已汇流 **`origin/Native`**（`https://github.com/dspo/openidea.git`）。
  **Native 是唯一事实源，对原 Mac 机器零依赖。**
- **接着做什么**：`ROADMAP-M9-M14.md` 的 M11（终端+调试器）→ M12（测试树+检查）
  → M13（本地 VCS 全量）→ M14（五语言深耕收官）。
- **怎么开始**：§5 接手流程（clone → 工具链 → Linux 首验门 → 按 §4 流程派 M11）。
- **最高纪律**：§7 红线清单（rebase-only 同步、后端零 diff、agent 不推送）。

## 1. 目标与背景

- **产品定位**：openidea 的最大价值 = 对 JetBrains IDEA UI 与使用习惯的复刻。
  GPUI 版必须达到同等的 UI 与习惯一致性（不嵌 WebView 的原生前端）。
- **本期焦点**（路线图 M9–M14）：①IDEA 习惯面（221/357 键位、菜单、工具窗
  stripe、状态栏 widgets）；②五语言本地开发体验：Go（已深耕到 M6 基线）→
  Rust/Python/TS/JS/Node 拉齐。
- **本期暂缓（≠不做，M14 收官报告给回收方案）**：Maven/Gradle 与 Java 构建链、
  GitHub PR、DB/HTTP/Docker 工具家族、WASM 插件 UI、60+ 语言广度、zh 双语、
  主题切换、post-first-commit 能力（notebook/agent/remote/spell）。
- **fork 原则**：长期同步上游、**不向上游贡献**。后端树（`crates/ languages/
  src/ src-tauri/ wit/`、根 lockfile）目标零 diff；一切前端实现只落
  `apps/openidea-gui`（独立 crate，根 Cargo.toml 以一行 exclude 隔离）。

## 2. 方案（已沉淀的架构决策，不要重新讨论）

1. **第三宿主**：后端 35 个 UI 无关 crate 原样复用，进程内直连（无 IPC/序列化）。
   `src/host.rs` 是防腐层——UI 不散点 `use ide_*`，上游 API 变动的波及面收敛于此。
2. **技术栈**：longbridge gpui-kit 线（gpui-pre 快照 + gpui-component 0.8.x），
   与用户的另一个项目 `~/dspo/manox-app`（Steer）同 pin。**注意**：Fedora 上没有
   这个本地路径——`git clone https://github.com/dspo/manox-app`（同一 GitHub
   账号有权限）作为参考仓库，终端（M11）与组件用法大量参照它。
3. **验证哲学**：每个里程碑实跑验证（transcript + 窗口级截图 + 性能预算），
   `script/verify.sh` 一条命令（单测→截图冒烟→预算门禁）。自 M0 起，实跑累计
   抓到十余个真 bug（焦点悬死、render 线程崩溃、双⇧死代码等）——这是流程核心。
4. **clean-room 双条款**：Zed（GPL-3.0）不读或仅编码前参考+笔记先行；
   **上游 Tauri 前端代码同样只许语义对齐、不许复制**。键位 preset 的
   action-id→键位映射属事实数据，允许数据级移植。
5. **协作模式**：用户在主会话派 code-deliver 子 agent 实施（见 §4），agent 本地
   增量提交，**不推送**；主会话统一推 `origin/Native`。
6. **终端技术路线（M11 已定）**：libghostty-vt 核心 + 自绘 GPUI element，架构照
   manox-app 的 `crates/steer-ghostty`（PTY 用 `ide-terminal` 的 portable-pty；
   pin Ghostty rev + 手抄 FFI；build.rs 的 zig 自动获取/盖章缓存方案照搬）。

## 3. 排期全景

### 已完成（M0–M10，全在 Native 上，每期一份 `apps/openidea-gui/M*-REPORT.md`）

| M | 主题 | 关键产出/情报所在 |
|---|---|---|
| M0 | IME 门（自绘 element 编辑器级 CJK IME） | M0-REPORT；InputHandler 协议模式 |
| M1 | 工作台壳 + 后端接线 | `host.rs`/`bus.rs`；Documents 漏斗、vfs watch |
| M2 | rope 编辑器（tree-sitter 高亮/多光标/折叠/虚拟化） | Highlighter span 叠层、perf HUD、55MB 首帧 2ms |
| M3 | LSP 智能层 + action registry | LspManager（tokio 捕获）、palette、焦点 bug 史 |
| M4 | run 配置 + Git 最小面 | dock 槽位、RunPanel、`ide_vcs::Repo`、diff overlay |
| M5 | 编辑器成熟 | 语义 tokens、⌥⏎、格式化、签名帮助、snippet、⌘F/⇧⌘F |
| M6 | Go DX | 保存整理 import、inlay hints、gopls 脾性记录（**每家 LSP 都有暗坑，M14 会再用**） |
| M7 | 工作台打磨 | welcome/recents、dock 持久化、toast、settings 最小面 |
| M8 | 日用化收官 | `script/verify.sh`、perf budgets、panic hook/crash.log、会话恢复、34min soak |
| M9 | 首次上游同步 + 习惯层 | 357 条键位对账（`src/reconcile_data.rs`，done 69/placeholder 158/na 130）、导航栈、书签、Switcher |
| M10 | 工作台结构层 | stripe 体系（`toolwindow.rs` 的 `lit` 标志即点亮开关）、Commit 工具窗、trust 对话框、状态栏 widgets、macOS 全菜单 |

### 剩余（详见 `ROADMAP-M9-M14.md`，含各期出口条件）

- **M11 终端 + 调试器**：libghostty-vt 终端 + DAP（delve/codelldb/debugpy/js-debug，
  toml 均已声明）+ Services 窗。≈M2+M3 量级，预计续传一次。
- **M12 测试树 + 检查**：五语言测试树（go test -json / cargo test / pytest
  junitxml / vitest-jest JSON）+ Problems 窗 + 重复代码检查。≈M3 量级。
- **M13 本地 VCS 全量**：log graph（**用上游重构后的新 API**）、branches、blame、
  冲突最小 UI、Find Usages 窗。≈M3 量级。
- **M14 五语言深耕收官**：Rust/Python/TS/JS/Node 拉齐 Go 基线（options 差异走
  app 侧覆盖层，目标零 toml 改动）+ TODO/scratch/viewers/welcome + keymap 设置页
  （含录制器）+ 三方对账收官。≈M8 量级，可拆两次派单。

## 4. 每个里程碑如何实施（协作流程）

派单指令的固定骨架（主会话发给 code-deliver 子 agent）：

1. **第一步读规格**：ROADMAP 对应节 + 前一期 REPORT 的「接缝情报」+ 相关源码。
2. **phase 0 固定项**：`git fetch upstream` → **在新分支上 rebase onto
   upstream/main（禁止 merge commit）** → 树不变证明（rebase 前打备份分支，
   `git diff backup..HEAD` 必须为空）→ `script/verify.sh` 回归全绿 → fork 卫生
   检查（后端树 diff 为空）。
3. **phase 切分**：每 phase 一个提交，提交时 fmt + clippy `-D warnings` + 单测全绿。
4. **实跑验收**：出口条件逐条给证据（transcript、窗口级截图、性能数字）；
   自动化不了的进人工清单。**截图绝不截全屏**（隐私纪律）；M0–前一期行为
   全量回归不许破。
5. **交付物**：`M<NN>-REPORT.md`（含给下一期的接缝情报）+ PR 正文写
   `/tmp/m<nn>-pr-body.md`（不真的开 PR）。
6. **agent 红线**：不推送任何 remote、不 `gh pr create`、只杀自己启动的进程
   （记 PID）、长命令分开跑（工具超时 10 分钟）。

## 5. Fedora 接手流程（step by step）

```sh
# 0. 仓库与远端
git clone https://github.com/dspo/openidea.git && cd openidea
git remote add upstream https://github.com/l0ng-ai/openidea.git
git checkout -b work origin/Native          # Native 是唯一事实源
gh auth login                                # dspo 账号，需对 dspo/openidea 有 push 权限
git clone https://github.com/dspo/manox-app ~/projects/dspo/manox-app   # 参考仓库（组件/终端）

# 1. 工具链（仓库要求 Rust 1.96+ / edition 2024；app 目录自带 rust-toolchain.toml 与 lockfile）
rustup install stable
# 系统依赖（以 Zed 官方 Linux 文档 Fedora 一节为准核对，典型如）：
sudo dnf install gcc gcc-c++ git curl wget file openssl-devel \
  libxkbcommon-devel wayland-devel wayland-protocols-devel dbus-devel \
  mesa-libGL-devel mesa-vulkan-drivers gtk3-devel
# LSP/DAP 工具按 languages/*.toml 的 install-hint 安装（gopls/rust-analyzer/
# basedpyright/vtsls/codelldb/delve…）；zig 不用手装（ghostty build.rs 自动获取）

# 2. 冒烟
cd apps/openidea-gui && cargo run            # 首次冷构建 5 分钟量级
```

**Linux 首验门（派 M11 之前必须过，等于 M0 的 Linux 重放）**：
①app 在 Linux 编译并开窗（真 GPU 或 lavapipe）；②**编辑器内 fcitx5 中文输入
全链路**（组合态/候选窗锚定/上屏）——M0 的保证是 macOS 的，Linux IME 必须重验；
③截图通道可用；④键位合成通道可用（见 §6）。任何一项不过先修/先评估，
再动 M11。首验结果与平台差异记录进 M11-REPORT 的专门章节。

**注意 `runtime_shaders`**：该 feature 是当年本机 Xcode 缺 Metal Toolchain 的
权宜（运行时编译 shader）。Linux/Vulkan 路径下重新评估——能离线编译就去掉，
决策与理由记录进报告。

## 6. GUI 自动化与「不抢用户鼠标」方案（迁移的核心动机）

原 Mac 上 agent 直接驱动真实会话（抢鼠标、被锁屏拦）。Fedora 上**建议隔离会话**：

| 方案 | 做法 | 优点 | 代价 |
|---|---|---|---|
| **Xvnc 隔离会话（推荐起步）** | `dnf install tigervnc-server xdotool`；`vncserver :2 -geometry 2560x1440`；`export DISPLAY=:2`；软件渲染 `LIBGL_ALWAYS_SOFTWARE=1`（wgpu 走 lavapipe） | 与用户会话完全隔离；xdotool 输入合成简单直接；IME（fcitx5）可独立跑在该会话 | 软渲染——**性能预算数字不作准**（预算门禁要么跳过、要么标注 VM 环境只做回归对比） |
| 嵌套 Wayland headless | `WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway`（嵌套在用户会话内）+ `grim` 截图 + `ydotool` 输入（需 uinput 权限） | 可用真 GPU；Wayland 语义 | 配置复杂；ydotool 需要守护进程与权限 |
| 真会话（同 Mac 模式） | 直接驱动主桌面 | 一切真实 | 抢鼠标（用户迁移就是要避开它），只留作最后手段 |

截图：X11 用 `import -window <id>` / `maim -i<id>`；Wayland 用 `grim`。
「屏幕锁定」检测的 Linux 类比：`loginctl show-session` / DBus screensaver 接口。
防休眠：`systemd-inhibit` 替代 macOS 的 `caffeinate`。
文本输入合成若被 IME 干扰：沿用「剪贴板粘贴」通道的经验（Mac 上验证过）。
性能预算的正确姿势：日常回归用隔离会话（相对对比有效）；**权威数字在真 GPU
会话测**，且 Fedora 硬件的预算基线要重新标定（budgets.json 的绝对值不跨机）。

## 7. 纪律红线（每期 REPORT 验收项）

1. 同步上游**只能 rebase + 解冲突**，禁止 merge commit；每次同步附树不变证明。
2. **后端树零 diff**：`git diff upstream/main..HEAD -- crates/ languages/ src/
   src-tauri/ wit/ Cargo.lock` 必须为空（根 Cargo.toml 仅允许 exclude 行）。
3. LSP/终端等行为差异走 **app 侧覆盖层**（先例：`LspManager::set_config_hook`
   承担了 go.toml 两行的职责），不改 `languages/*.toml`。
4. agent **不推送任何 remote、不建 PR**；主会话统一推 `origin/Native`
   （rebase 改写历史后用 `--force-with-lease`）。
5. 截图只截窗口不截全屏；只杀自己启动的进程；不弄脏仓库（dogfood 走
   `fixtures/` 与 `/tmp` scratch）。

## 8. 原机器遗留（仅供参考，无依赖关系）

Mac 上 `~/worktrees/openidea/` 有 M0–M10 的十个 worktree（各期分支同 Git 仓库
共享，均已被 Native 覆盖）；另有 `m10-pre-rebase-backup` 备份分支仅存在于原机器。
这些都可清理，不影响 Fedora。

## 9. 没被点名但你大概率需要的

- `apps/openidea-gui/README.md` 过时（停在 M0），别按它理解现状。
- gpui 在 Linux 的 IME（尤其 fcitx5）与窗口管理器行为是首验门的真正考题，
  Zed 上游与 gpui-kit 的 Linux 进展可参考（manox-app 也维护 Linux 面）。
- macOS 全局菜单栏在 Linux 不存在——IDEA 在 Linux 上本就画**窗口内菜单**，
  M10 的菜单结构挂在 action registry 上，Linux 侧做窗口内呈现反而更贴 IDEA。
- 原生对话框（rfd）在 GNOME 走 xdg-desktop-portal，隔离会话里需装 portal 前端。
- ghostty 的 build.rs 首次会下载 pin 定的 Zig 工具链与 tarball（需网络），
  之后走盖章缓存。
- 各期「给下一期的接缝情报」是连续叙事：M10-REPORT（stripe 点亮开关、Bus 通道、
  状态栏独立实体方向）→ M9-REPORT（picker 7 模式、chord、对账表）→ 更早各期
  按需回查。接手后第一件事建议通读 M10/M9 两份报告的接缝节。
