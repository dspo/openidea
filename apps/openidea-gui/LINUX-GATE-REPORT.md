# LINUX-GATE-REPORT — Fedora 接手首验门（2026-11-07）

> HANDOFF §5 的 Linux 首验门在 Fedora 44 机上的执行记录。M11 phase 0 派单前
> 必读；②的结论未定，派单时按本报告的"下一步"继续。机器与环境的工具链现状
> 见 memory（fedora-machine-toolchain）；本文只写门本身。

## 门项总览

| 门项                    | 结论                             | 证据                                                                                                    |
| ----------------------- | -------------------------------- | ------------------------------------------------------------------------------------------------------- |
| ① 编译并开窗            | **过**                           | `cargo build` 全绿；Wayland 原生窗口（首帧 320–470ms），见 85311adf                                     |
| ② fcitx5 中文输入全链路 | **未过（阻塞点已定位到证据级）** | 本文 §2                                                                                                 |
| ③ 截图通道              | **过**                           | headless sway + grim + crop，窗口级 PNG（132KB），verify.py smoke 01                                    |
| ④ 键位合成通道          | **过**                           | smoke 5/5 状态全绿（Ctrl+Shift+F / Ctrl+, / Ctrl+W / Esc 链 / Ctrl+Q 优雅退出），见 39a256da + a43b3db5 |

工程门基线：`cargo fmt --check` / `clippy -D warnings` / 122 单测 / smoke 全绿。
本报告对应的提交序列（`work` 分支，按红线未推送）：
85311adf → 51b2b849 → 1e8ee0b5 → 39a256da → 9049f3f9 → a43b3db5。

## 1. ①③④：过程揪出的平台差异（均已修复提交）

1. **gpui-pre-platform 的显示后端是 opt-in feature**（85311adf）。默认只有
   `multithreaded`：Linux 构建静默以 headless 平台启动——LSP/VCS/索引照常跑、
   "first paint" 日志照打，但窗口永远不出现。修复：显式启用 `wayland`。
   **x11 按产品决策永不启用**（Wayland-only）。
2. **键位主修饰键**（39a256da）。gpui 的 Linux 平台把 `platform` 修饰键映射到
   Super（`modifiers_from_xkb` 读 MOD_NAME_LOGO），而键位表按 macOS 约定把
   IDEA 的 `Meta+` 翻成 gpui `"cmd"`——⌘ 习惯键在 Linux 全变 Super+key。修复：
   注册时把主修饰键翻成 Ctrl（macOS 字符串不动；已含 Ctrl 的组合保留，防
   ⌃⌘R 塌缩成 ⌃R 抢走 Run 的键）。
3. **find 面板 UTF-16 切片崩溃**（9049f3f9）。`render_line_text` 把 UTF-16 列
   当字节偏移切片，CJK 注释行渲染必崩（smoke 抓到：`inside '真'`）。数据依赖、
   平台无关，macOS 只是没踩中。加 `utf16_byte` 边界换算 + 单测。
4. **无头会话的键盘焦点**（a43b3db5）。headless sway 的 seat 没有输入设备；
   wtype 每次调用即建即毁虚拟键盘（sway 日志 add→remove 间隔 2ms），seat 的
   keyboard capability 随之消失——**每个会话只有第一个键能送达**。修复：驱动
   全程挂一个 do-nothing 的 `wtype -s 2147483647` 常驻客户端。
5. **verify 驱动层跨平台**（51b2b849）。`MacDriver`（Quartz/osascript/
   screencapture）原样保留，`LinuxDriver`（sway 会话生命周期、swaymsg 找窗/
   聚焦、grim+convert 裁剪、wtype 合成、wl-copy 粘贴）并列。**双平台纪律：
   脚本/物料两套都保留，Linux 是增加不是替换。** verify.sh 的 stage 门按平台
   分发：Linux 跑 stage 1–2，stage 3 预算门默认跳过（budgets.json 是 macOS
   基线且 headless 是 lavapipe 软渲，绝对值不可比；`M8_VERIFY_BUDGET=force`
   可强制）。
6. 技术坑速查（勿重蹈）：sway 1.11 树 JSON 无绝对 x/y（先
   `[con_id=N] floating enable; move position 0 0` 锚定原点再裁剪）；grim 1.5
   无 geometry 参数（全输出截图 + convert -crop）；swaymsg 的 getopt 会吞
   `--custom`（mode 命令单字符串传、不带 `position 0,0`）；headless output 需
   `mode --custom WxH`；gpui 的 `log::info!` 不进 app 日志（只桥接了
   tracing），判断平台状态用 `[ime]`、字体解析行为、真实首帧耗时等侧证。

## 2. ②：IME 全链路——已建成的隔离链路与未过的缺陷

**隔离链路（已可用）**：`script/ime-probe.sh`（本文 commit 收编）= 私有
`dbus-run-session` + 私有 XDG 目录（预置 pinyin profile）+ headless sway +
fcitx5（input-method-v2 前端）+ 常驻虚拟键盘 + app。**绝不在共享 session
bus 上起第二个 ibus-daemon（`--replace` 会杀掉用户桌面的输入法）。**

已验证 ✓ 的部分：

- fcitx5 5.1.23 经 input-method-v2 连上 sway（自检
  "Using Wayland native input method protocol: 1"），pinyin/标点/云拼音 addon
  全部加载；`fcitx5-remote -t` dbus 控制有效（state → 2 / im=pinyin）。
- **app 的 text-input-v3 通道正常**：`[ime] firstRect`（候选窗锚定所需数据）
  与 commit_string → `insertText` 都正确到达。M0 的 IME 协议面在 Linux 可用。

未过的缺陷（两组证据，均有日志/截图，留存于 /tmp/takeover/ime/，易失）：

1. **组合从未发生**：dbus 切到 pinyin 后打 "ni"，app 收到的仍是逐字母
   commit；无 preedit、无候选窗；fcitx5 `--verbose key_trace=5` 全空——
   **按键从未进入 fcitx5 的处理管线**，默认触发键 Ctrl+Space 也到不了它
   （fcitx5-remote 状态不变）。
2. **双写**：fcitx5 附着时每个字符键 app 同时收到原始键事件（key_char 插入）
   - commit_string（IME 插入）→ 缓冲区 "nniihhaaoo"。**无 fcitx5 对照组单写
     正常**。

归属未定（三个嫌疑：fcitx5 waylandim 的直通设计 / gpui-pre 对两条通道都消费 /
sway 1.11 + wlroots 0.19.2 的 relay 行为；manox-app 无相关补丁可参照）。

**下一步（按序）**：

1. GNOME 真桌面手测（用户 2 分钟）：真桌面跑 app 切 ibus 拼音打 "nihao"。
   正常 → 缺陷只属于 sway+fcitx5 自动化路径，②按 GNOME/ibus 记录通过（平台
   差异写明）；同样异常 → gpui-pre 的 Linux IME 真缺陷，转上游对照。
2. 对照上游 Zed 的 Linux IME/commit_string 处理（clean-room：先笔记后代码）；
   双写本身独立于 IME 可修（客户端按 composing 状态去重）。
3. 任何一项的修复都走 app 侧覆盖层红线；修完重跑 ime-probe + smoke 回归。

## 3. 接手遗留（M11 派单前/派单 phase 0）

- upstream 领先 66 commits（接手时点），M11 phase 0 按 HANDOFF §4 rebase 并
  附树不变证明（接手时已验证后端树对 fork 点零 diff）。
- `runtime_shaders` feature 在 Linux/Vulkan 下重评估能否去掉（HANDOFF §5 注）。
- 字体：`DejaVu Sans Mono` 缺失警告（回退 Noto Sans Mono），gpui-component
  的默认偏好，待进 M14 键位/设置盘点。
- 推送：全部提交在本地 `work`（跟踪 origin/Native），按红线未推送；主会话
  推送用 `--force-with-lease` 语义并入 `origin/Native`。
