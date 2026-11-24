# Terax v0.9.0 整合进度

更新日期：2026-11-10。分支：`integrate/terax-v0.9.0`。

## 目标与边界

将 Terax v0.9.0 融入 Phantom，启用 Ghostty/WASM + WebGPU 终端及 Ghostty WebGL 回退，保留浏览器、WebSocket、rmux、Room、移动输入和桌面端行为。

基线：`d91bbe2`。上次上游合并：`b69532b`，上游父提交 `ebed02e`。
目标：[上游 v0.9.0](https://github.com/crynta/terax-ai/releases/tag/v0.9.0)，commit `d0b569c0aa401c79b08ae8fc048be797739c88b4`，本地标签 `upstream-v0.9.0`。

执行顺序：P0 → P1 → P2 → P3。环境阻碍不依赖的工作可先推进，例如 Rust 依赖下载期间验证 Ghostty core。
每项只有取得验收证据后才能标记完成。手工验证与自动验证分开记录。
不部署、不替换运行中的服务、不关闭用户现有 rmux 会话。

## 当前状态摘要

以下清单依据已提交实现及当前工作区的验证记录同步，最新检查见“本轮进度”。`[x]` 表示该条描述的工作已有证据，不代表所属阶段整体通过；未勾的大项按子项区分已完成、待迁移和待验收。后文执行记录是历史快照，其中的“待完成”以本清单为准。

| 阶段 | 当前状态 | 剩余事项 |
| --- | --- | --- |
| P0 | 6/6 项已执行并记录 | 原始 Rust 基线未取得完整通过证据，环境失败已记录，不能用当前测试冒充原始基线 |
| P1 | 8/9 项完成，P1.2b 部分完成 | 其余 Rust 上游差异对齐；Linux 桌面编译及自动测试已通过，实机行为仍见 P3 |
| P2 | 本轮推进 P2.2 终端行为与 P2.3 Room/agent/媒体边界，不再只推进 P2.4；5 个大项仍未整体验收 | P2.2/P2.3 的已验证路径和实机缺口已拆开勾选，见下；P2.4 网络流控与连接池已迁移，其余差异及实机验收待完成 |
| P3 | P3.1/P3.2 Linux 自动检查通过，PTY 与 rmux 目录恢复已复验；修复测试隔离后 rmux 大输出完整性失败 | rmux 大输出、真实 GPU/GUI/设备、资源度量及完整差异审计 |

### 分项变化与验收边界

| 条目 | 已取得的实现/自动验证证据 | 仍未取得的证据 |
| --- | --- | --- |
| P2.1 | 移动键盘入口、触摸焦点与防误触自动验证 | 真实移动设备 IME、粘贴、焦点与快捷键 |
| P2.2 | 本轮新增真实 SIMD/scalar WASM 组合测试、真实 PTY 语义事件及 AI 隐私边界验证 | 实际渲染 surface 上的鼠标选区、搜索 UI、链接点击、系统剪贴板和块交互 |
| P2.3 | 本轮新增 Room 跨连接广播/回放、真实 PTY agent marker 信号和内联媒体 HTTP 安全边界验证 | 真实 coding-agent、OS 通知、Room peer/mesh、媒体渲染及品牌 UI |
| P2.4 | Git、编辑器监听、会话保存、文件树/复制/剪贴板/索引、网络地址安全、连接池及 AI 流式字节窗口/ack/取消/二进制传输 | 其余上游差异、macOS/Windows 原生编译和平台 GUI |
| P2.5 | WASM 产物校验、回退逻辑及真实 Chromium SwiftShader WebGL 页面/终端输入冒烟 | HTTPS、真实 GPU 的 WebGPU/WebGL、设备兼容及完整交互验收 |

## 独立工作与测试环境

- 原服务目录：`/path/to/original/Phantom`，保持 `main`；原有 `.phantom/agents.yaml` 修改保留原处。
- 当前整合目录：独立 clone `/path/to/Phantom`，分支 `integrate/terax-v0.9.0`。从已更新的 `/tmp/phantom-terax-v090` 复制，Git 对象、依赖与 `target/` 均位于磁盘目录；本地 `.phantom/agents.yaml` 改动保留。旧 worktree 保留，不再承担本轮构建。
- 后端测试地址：`127.0.0.1:19889`。前端开发地址：`127.0.0.1:15174`，严格占用该端口，不自动切换端口。
- Cargo 产物：当前 checkout 自己的 `target/`；前端产物：当前 checkout 自己的 `dist/`。禁止输出到原服务目录。
- 测试配置和运行状态：`/tmp/phantom-terax-v090-runtime`；使用独立 XDG config/data/cache，关闭自动启动 agent 和 Room peer/hub，默认本地 PTY。
- 测试不重启当前服务、不使用当前服务端口、不接管现有 rmux 会话。rmux 实测须使用独立测试会话或 daemon。

运行方式（Linux，均在 `/path/to/Phantom` 内）：

```sh
pnpm run dev:integration:server
pnpm run dev:integration
```

后端启动脚本接受独立 clone 和 linked worktree，强制独立 XDG 状态目录和 Cargo target，拒绝从 `main` 分支启动；Vite integration mode 将 `/ws`、`/files` 和 `/blob` 转发到测试后端。

## P0：基线、冲突与落地方案

- [x] P0.1 创建整合分支并确认起点、工作区状态。
- [x] P0.2 建立此进度文档，明确优先级与验收条件。
- [x] P0.3 拉取固定版本标签，记录 commit、共同祖先、上游变更量。
- [x] P0.4 试算合并冲突，列出 Rust 路径迁移、删除/修改冲突和 Phantom 定制点。
- [x] P0.5 执行前端和 Rust 基线检查，记录已有失败。初始前端检查通过；初始 Rust 检查受依赖下载阻碍，后续严格 Clippy 和桌面构建失败亦已记录。本项是“执行并记录”，不是“原始基线全绿”。
- [x] P0.6 确定浏览器 PTY 适配接口，保留二进制输入、输出顺序、解析确认和有界队列。

验收：目标版本可本地复现；冲突清单和传输方案明确；已有失败与整合引入的失败可区分。

## P1：终端核心可运行

- [x] P1.1a 引入固定版本的 Ghostty core、许可证及 SIMD/scalar WASM；校验 checksum 和实际解析行为。
- [x] P1.1b 融入上游渲染器、终端入口和依赖，保留 Phantom 构建别名和独有模块。证据：Ghostty 入口已接入，类型、生产构建和前端自动测试通过。
- [x] P1.2a 迁入共享 `wire/output/queue/presentation/wake/hub`，为本地 reader 和 rmux 提供有界队列。
- [ ] P1.2b 对齐 Tauri 原生 stream 命令，以及其余 Rust 上游改动与依赖。
  - [x] 接入原生 `pty_stream_open`、`terax-pty` poll/ack 协议和共享 hub；已有桌面库 48 项测试通过记录。
  - [ ] 对齐其余 Rust 上游差异；当前 Linux GLib/GTK/WebKit 依赖已具备，桌面编译、严格 Clippy 及自动测试通过。
- [x] P1.3a 接通浏览器二进制 poll/ack 与共享流控；真实本地 PTY + Ghostty WASM 解析冒烟通过。
- [x] P1.3b 将新流控接入实际 Ghostty UI 生命周期，保留桌面端原生传输。证据：`useGhosttyTerminalSession` 和 `pty-bridge` 已接线，浏览器 WebSocket / Tauri 原生传输分流保留；桌面实机验收另见 P3.4。
- [x] P1.4 迁移 rmux 新建、稳定会话名、重连接管、输出回放和关闭语义。
- [x] P1.5 覆盖二进制键盘输入、分块输出、退出顺序、断开释放及设备查询去重的关键不变量。
- [x] P1.6 前端类型检查、生产构建及 headless Rust 编译通过。当前复验：前端 1813 项、core 434 项、server 92 项及桌面 117 项测试通过；桌面运行时 GUI 和跨平台验收另列。

验收：真实浏览器能启动 shell、输入、输出、缩放和重连；rmux 接管不重复创建 shell；隐藏终端持续解析输出。

## P2：行为与其余上游改动

- [ ] P2.1 保留移动键盘入口、触摸焦点、中文 IME、粘贴与快捷键。
  - [x] 保留移动键盘入口；触摸单击聚焦及长按/滑动防误触逻辑已有自动验证记录。
  - [ ] 真实移动设备中文 IME、粘贴、焦点和快捷键整体验收。
- [ ] P2.2 验证选区、搜索、链接、OSC 7/52/133、命令块和 AI 终端上下文。
  - [x] OSC 7/52/133 与命令块组合验证：SIMD/scalar 真实 WASM 分字节解析，命令输出中的伪 cwd 不改变工作区；剪贴板查询、非 clipboard 选择及非法内容不写剪贴板；块的 cwd/命令/退出码/输出和 alternate-screen 恢复正确。独立真实 PTY 经二进制 poll/ack、模型和语义路由的端到端检查通过。
  - [x] 选区、搜索及链接的模型/控制器组合验证：中文选区、历史分步搜索及搜索控制器重建、OSC 8/普通链接、危险 scheme 拒绝、浏览器 noopener/noreferrer、native opener 失败后的焦点恢复通过。覆盖两个 backend 标签的模型契约，不代表实际 GPU/WebGL surface 渲染验收。
  - [x] AI 上下文桥验证并修复隐私绕过：active-terminal 与任意 leaf 读取统一校验当前 tab/pane 所有权及 Privacy，最多请求 300 行并脱敏；关闭标签遗留 ref、非活动私有分屏和歧义 leaf id 不读取。真实 Ghostty 模型释放 presentation 后继续解析输出，AI 仍读到最新脱敏内容。
  - [ ] 真实浏览器/桌面选区、搜索栏、链接点击/焦点、OSC 52 系统剪贴板、命令块交互及真实 shell 集成整体验收。
- [ ] P2.3 验证 Room、agent 状态和通知、文件上传/下载、内联媒体与 Phantom 品牌。
  - [x] 隔离 HTTP 文件上传/下载边界验证：并发同名上传不覆盖、已有 symlink 不跟随、非法文件名拒绝，以及下载内容、attachment 和 nosniff 响应头通过。
  - [x] Room 隔离 WebSocket 验证：一连接发帖，另一连接实时接收带 epoch 的广播；断开后新建连接以 since-seq 回放缺失消息，内容、顺序及 epoch 正确。未启动 Room peer/hub 网络或实际 agent。
  - [x] agent 状态传输验证：真实独立 PTY 输出受控 OSC 133/777 marker，经 Rust 检测和 WebSocket 顺序传出 started/attention/working/finished/exited，agent id 与 PTY id 正确；不等同于运行真实 Codex/Claude 或系统通知实测。
  - [x] 内联媒体 HTTP 边界验证：上传后下载字节一致，SVG/图片类型仅在显式 inline 时返回受限 MIME、sandbox 和 nosniff；即使请求 inline，HTML 仍为 attachment。未执行浏览器脚本或实际解码媒体。
  - [ ] Room peer/mesh、真实 coding-agent 和通知呈现、媒体解码/渲染、Phantom 品牌及真实浏览器上传/下载整体验收。
- [ ] P2.4 对齐其余 v0.9.0 改动，包括文件树、Git、编辑器、AI 会话、安全和资源优化。
  - [x] Git 未跟踪目录预算展开及输出截断保护，自动测试通过。
  - [x] 编辑器文件级 watcher 和监听释放迁移，自动测试及隔离 WebSocket 验证通过；macOS 实机验收另列。
  - [x] AI 会话文件持久化、备份恢复、旧数据迁移和流式保存调度，自动测试及隔离 WebSocket 验证通过。
  - [x] 文件树多选、批量移动/删除和终端多路径拖放源码整合，冲突替换增加备份恢复，自动测试通过（`11de0a1`）。
  - [x] 文件树可见性资源生命周期迁移：隐藏时释放所属监听、暂停重读，显示时补同步；浏览器上传和原生拖放监听随可见性暂停。工作区隔离、异步过期响应和恢复流程的自动测试通过。
  - [x] 授权复制共享层及 WebSocket 协议迁移：部分失败报告、同名副本、符号链接保留、有限递归及原子不覆盖发布，Linux 自动测试与隔离 WebSocket 实测通过；Tauri 适配源码已接入，桌面编译验收仍见 P3.2。
  - [x] 剪贴板工作流与输入分流自动验证：原生粘贴重复请求合并、根目录/工作区/隐藏/卸载失效、部分失败及刷新处理；浏览器不调用服务端系统剪贴板，输入框粘贴不上传文件，多文件上传固定原环境。
  - [x] 文件索引共享层及 WebSocket 接入：有界 packed 缓存、两槽 LRU、45 秒 TTL、并发冷查询合并、变更期间禁止缓存旧扫描；写入/创建/删除/移动/复制/HTTP 上传和已投递 watcher 事件触发失效，重新注册目录监听时补同步。Linux 自动测试及隔离 WebSocket/HTTP 验证通过；Tauri 适配源码已接入，桌面编译仍见 P3.2。
  - [x] 剪贴板粘贴及原生拖放复制后自动多选：目标刷新并展开后，按文件树顺序选择可见的成功项、设置锚点并滚动定位；刷新失败、全部失败或无可见项不覆盖选区，切根目录/工作区/隐藏/卸载后拒收旧结果。组合 hook 及生命周期自动测试通过，真实 GUI 验收另列。
  - [x] 网络地址安全迁移：共享 core 识别 IPv6 中内嵌的 IPv4、非公网保留地址及带末尾点的 metadata 主机名；私网授权不豁免 metadata 拦截。命名重定向只进入已校验并固定解析的主机，AI 请求和 ping 禁用隐式环境代理以保留地址校验边界。纯函数及真实本地 HTTP/流式/独立子进程自动验证通过。
  - [ ] 文件树多选、拖放、上传和替换确认的真实 GUI 验收。
  - [ ] 三平台系统剪贴板读取的原生编译及真实 GUI 验收，包括复制结果自动多选和滚动定位。
  - [x] AI 网络流式字节窗口、ack、取消及请求所有权、二进制 IPC 和客户端连接池迁移。Tauri 与 WebSocket 共用 core；真实隔离 WebSocket 验证慢消费者停发/恢复、非法 ack、跨连接隔离、响应头前后取消与断线释放。
  - [x] 桌面密钥 IO 移到阻塞线程池，Linux 桌面复用共享 SecretsStore；写入与缓存发布串行，落盘失败保留旧缓存，随机独占临时文件替换固定临时路径。共享存储权限、并发保存、失败回滚及临时符号链接回归通过。
  - [x] LSP 资源优化：合并内存检查与 child waiter，删除独立轮询线程，管道/等待线程使用 256 KiB 栈；迁入内存预算、检查间隔、deadline 和 LSP framing 回归。保留共享 core 进程组/Job Object 路径。
  - [x] 编辑器换行列设置及路径绑定的延迟聚焦迁移；换行默认 80 列、范围 20 至 500，配置读取/写入/跨窗口变更接入。CodeMirror extension 和配置限额回归通过；实际焦点交互仍需 GUI 验收。
  - [x] Git 刷新状态迁移：工作区与路径共同标识在途请求，切仓库清除旧快照、拒收过期结果，路径复用兼容 Windows/UNC。迁入 15 项状态/路径/消费者策略回归；消费者可见性和固定仓库 UI 的接线尚未迁移。
  - [ ] 其余编辑器、AI、安全和资源优化差异逐项对齐。
    - 已确认缺口：Git 固定仓库/返回上下文及可见消费者接线，批量关页保护和保存确认，agent 通知去重/节流/声音，预览 loopback cookie 提示，文件图标引擎、字体/主题/窗口合成资源优化。导出收缩等纯清理差异需单独判定，无须为了源码一致删除 Phantom 使用的接口。
- [ ] P2.5 验证 WASM 资源发布、HTTPS 浏览器加载和 WebGPU 不可用时的 Ghostty WebGL 回退。
  - [x] SIMD/scalar WASM 校验与解析自检通过，生产构建包含两版 WASM 资源。
  - [x] WebGPU 不可用时的 WebGL 回退选择逻辑已有自动验证记录。
  - [x] 生产页面在真实无头 Chromium 中通过 HTTP 加载；SwiftShader WebGL canvas 实际渲染，键盘输入 `printf` 后可见输出，无未捕获异常。软件渲染证据不等于真实 GPU 或 HTTPS 验收。
  - [ ] HTTPS 实际加载、真实 GPU 的 WebGPU/WebGL 渲染及设备兼容性验收。

验收：Phantom 独有路径未丢失；无 xterm 残留调用；安全校验和用户配置保留。

## P3：验证与交付

- [x] P3.1 当前修订的 `pnpm lint`、`pnpm check-types`、`pnpm exec vitest run --maxWorkers=2`、`pnpm build`。183 个测试文件 / 1813 项通过；lint 97 warning、1 info、无错误。后续源码变更和最终交付前仍须重跑。
- [x] P3.2 Linux Rust workspace 与 `src-tauri` 的严格 Clippy、测试及桌面编译通过；macOS/Windows 原生编译与真实桌面行为仍待验收。
- [ ] P3.3 真实 PTY/rmux 冒烟：大输出、断线重连、退出、tab 切换、分屏缩放。
  - [x] 当前修订真实本地 PTY 语义/退出冒烟：隔离后端、二进制 poll/ack、真实 Ghostty 解析及零 pending 字节、有序退出通过（`pnpm run check:terminal-behavior`）。早先 4 MiB/独立 rmux 证据见执行记录，不据此声称已覆盖最新修订的全部场景。
  - [ ] 当前修订 rmux 长输出与重接：修复 SDK endpoint 隔离后，256 KiB live flood 仍有字节缺失，检查未通过，原完整性断言保留。此前本机测试的临时 socket 不在 SDK 环境发现允许的目录内，实际回退到了日常 daemon，不能作为独立 daemon/socket 的通过证据。
  - [x] rmux 目录恢复：无 OSC 7 的忙碌前台进程重新接入后，连接内 `pty_get_cwd` 可恢复真实进程 cwd，其他连接不能读取该 PTY。独立 Chromium 验证从另一项目切入时文件树和标签匹配，刷新保留相同 rmux PID。
  - [x] 测试 daemon 隔离：`RMUX_TMPDIR` 指向本次临时目录，SDK endpoint 与 CLI 使用其下用户专属 socket；后端仅能列出本次探针会话才允许继续创建测试终端。失败清理后，日常 daemon 复查无 `phantom-isolated-*` / `phantom-probe-*` 会话。
  - [ ] 真实 GUI tab 切换和分屏缩放。
- [ ] P3.4 GUI 验证：Codex/Claude TUI、滚屏、中文 IME、手机、WebGPU 与 WebGL。
- [ ] P3.5 资源验证：隐藏标签空闲 CPU、反复切换和缩放后的内存、长时间 agent 输出。
- [ ] P3.6 汇总改动、检查证据、剩余限制和回退方式；更新架构说明。
  - [x] 当前核心架构说明同步到 Ghostty model/surface、共享 PTY stream 和统一 AI 终端上下文隐私边界；分项自动证据与实机缺口已列入本清单。
  - [ ] 完成交付前完整差异审计、最终证据汇总及回退步骤。

验收：自动检查通过；未执行的设备或平台检查明确列出，不将编译通过等同于 GUI 验证通过。

## 已确认的技术差异

1. v0.9.0 已移除 xterm，终端模型改为 libghostty-vt，每个 leaf 保留解析状态，渲染 surface 独立管理。
2. 新 `pty-bridge` 使用 `terax-pty` 自定义协议、`PtyOutputHub` 和解析确认；现有 `invoke` 别名不能直接替代该 fetch 路径。
3. Phantom 的 PTY 有本地与 rmux 两条后端路径，共享逻辑已抽入 `phantom-core`，不能直接覆盖上游 Rust 目录。
4. 近期移动输入、Codex 滚屏和 rmux 查询去重修复必须迁移到新实现。

## 已落地：P0.6 / P1.2a / P1.3a 传输适配

已选定：在现有鉴权 WebSocket 上复用上游 `PtyStreamTransport`、`PtyOutputHub`、`PtyOutputReceiver` 和 `wire/queue/output`；不为浏览器新增 HTTP PTY 服务。

- `pty_open`/`pty_attach` 保留稳定 rmux session name，补入上游 pane/presentation 字段。
- 输入保持 `string | Uint8Array`，Ghostty 键盘编码、粘贴和终端回复不经过 UTF-8 二次编码。
- 输出按解析完成后的累计 offset 归还 credit；不允许读到字节即确认、静默丢弃或无界缓存。
- 浏览器复用现有鉴权与连接生命周期；原生 `terax-pty` 不能由现有 `convertFileSrc` 误映射为 `/files`。
- poll/ack 分别使用二进制 tag 2/3，响应 tag 2 携带 request id、状态和原始上游 wire bytes；旧终端 I/O tag 1 保留。
- 解析窗口上限 1 MiB / 128 chunks，pending + in-flight 上限 2 MiB；本地 reader 等待 credit，rmux 使用异步 Notify 等待，不阻塞 Tokio worker。
- 每连接最多四份在途响应，permit 保留到 WebSocket writer 完成发送；断开关闭队列并撤销 token，释放阻塞的 reader。初版为两份，rmux 验证后增至四份。
- 上游纯队列/协议逻辑落到 `phantom-core`，Tauri 与 HTTP/WebSocket 的平台适配保持薄层。

验收场景：同步/异步 parser、超过窗口的大输出、重复/伪造确认、数据先于 open 返回、输出排空后退出、断开后取消等待、rmux 重接与设备查询去重。
本地 PTY 的实际流控与退出顺序、独立 rmux daemon 的重连和回放已验证；桌面原生协议已接线，其余 Rust 差异仍属 P1.2b，完整桌面构建和真实 GUI 验收尚未关闭。

参考：[迁移说明](https://github.com/crynta/terax-ai/blob/v0.9.0/docs/architecture/ghostty-webgpu-migration.md)、[发布验证记录](https://github.com/crynta/terax-ai/blob/v0.9.0/docs/architecture/ghostty-release-readiness.md)。

## 本轮进度（2026-11-10，磁盘目录继续集成）

- 修复中文输入法候选窗远离输入位置：WebGL/WebGPU 的隐藏 textarea 随终端光标按 CSS 像素定位，补偿 GPU 网格偏移；不改组合输入内容或选区，原生 Copy 菜单临时定位期间不覆盖，静止光标不重复写样式。新增 6 项回归，184 文件 / 1819 项、类型、lint 和构建通过（lint 保留原有 97 warnings / 1 info）。私有 rmux 与独立 Chromium 验证 2x DPR、窗口缩放、组合输入及分屏的实际输入框坐标；macOS 原生候选窗仍需实机确认。
- 修复重新接入会话后文件树和标题错位：浏览器在 PTY 建立后异步查询一次连接内的真实进程 cwd，不阻塞输出，也不向运行中的 Codex 注入 shell 命令；仅接受 rmux 的 Process/RootProcess 来源，未知目录不再回退到另一个终端的项目。关闭和代际失效后拒收迟到结果。前端新增 4 项回归，183 文件 / 1813 项、类型、lint 和构建通过；server 92 项及严格 Clippy 通过。独立 Chromium 的旧 8100 复现失败、修复版目录/标签同步及刷新相同 PID 验证通过。
- 更正测试隔离证据：本机 `${runtime}/rmux.sock` 不在 rmux SDK 环境发现允许的用户 socket 根目录内，SDK 忽略后回退到日常 daemon，失败的测试因此留下 `isolated-*` 会话。现已设置私有 `RMUX_TMPDIR`、使用其下 `rmux-<uid>/default`，并在任何测试 PTY 创建前核对唯一探针；旧 `check:rmux-stream` 入口统一复用该检查，不再覆盖 `dist/index.html`。修正后私有 daemon 的 cwd/连接所有权检查通过，日常 daemon 无测试残留；256 KiB 大输出检查仍失败，最新仅收到 179200 个 fixture 字节，未放宽或跳过断言，不能声称 rmux 完整验收。记录见 `/tmp/phantom-cwd-rmux.log`。
- 8100 已加载本轮后端与页面修复，沿用原 Phantom HTTPS 证书、认证和日常 rmux daemon；取消影响 mise 的临时 XDG 覆盖。临时 18810 验证服务已关闭，8099 未重启。
- 浏览器图片粘贴回归：Ghostty 原先在 Cmd+V/Ctrl+Shift+V 时取消原生事件并仅读取文本，截断 Phantom 的 `/blob` 图片路径。浏览器现在保留默认粘贴行为，仅停止按键向应用冒泡；Tauri 仍使用原文本剪贴板分支。新增 2 项回归先失败后通过；前端全量 1809 项、类型、lint 和构建通过。独立 Chromium 在 8100 HTTPS/rmux 上验证 Mac 快捷键事件不被取消，以及受控 ClipboardEvent 的 PNG 上传、路径输入和浏览器解码；这不是 macOS 实机原生剪贴板验收。页面产物已更新，无需重启后端或中断现有 rmux 会话。
- 继续审计：按 `ebed02e..upstream-v0.9.0` 的变更集比对当前源码，识别仍等同旧版的文件，未将“与上游不同”直接当成缺陷。完成桌面密钥阻塞 IO、共享存储事务、LSP 线程/等待优化、编辑器换行列与延迟聚焦、Git 刷新状态隔离；上述子项已拆入 P2.4，其他明确缺口保留未勾选。
- 本次新增密钥回归 3 项、LSP 回归 7 项、前端回归 20 项。Rust workspace 测试 643 项通过（core 434、server 92、桌面 117），严格 Clippy 通过；前端 182 文件 / 1807 项、类型及生产构建通过，lint 97 warning、1 info、无错误。`check:ai-stream` 再次通过，未调用真实模型、未写用户密钥或操作现有服务。日志位于 `/tmp/phantom-integration-{cargo-tests,clippy,frontend-tests,types,lint,build,ai-stream}.log`。
- 基线：拉取远程集成分支至 `3fabda1d`，再合入远程 `main` 的 `72ce300e`，本地 merge 为 `b7607b22`。本次检查时远程 main 尚未包含集成分支；没有推送或改写远程 main。
- 工作目录迁至 `/path/to/Phantom`。`/path/to/projects` 需要 root 创建，经用户确认采用已有可写目录。保留旧工作区与本地 agents 配置，未部署或重启原服务。
- 复用上游 `net/stream.rs` 与 packet pump，迁入共享 core：每请求 128 KiB 未确认窗口、64 KiB packet、256 个未确认边界、16 个活动请求，以及停读超时。JS 使用 64 KiB 队列，按队列余量累计 ack，释放 abort listener；Tauri raw IPC 与浏览器 WebSocket 二进制响应均接入。WebSocket 格式为 `[0x04][u64 LE invoke req][kind][payload]`，kind 0/1/2/3 分别为 headers/body/end/error。
- 请求以 webview label 或连接内 registry 隔离。取消覆盖 DNS/响应头等待和 body 停读，webview 导航/销毁撤销所属请求；socket 断开关闭 registry，拒绝排队任务晚启动。旧 JSON 事件适配仅保留在测试中，原有 SSRF、重定向和连接池回归实际经过新 pump。
- 修复桌面测试遗漏的 8 处 FileIndexState 参数，Linux 桌面检查不再受旧签名阻断。隔离脚本改用系统临时目录，不依赖旧机器的 `/tmp/opencode`。rmux 检查修复把命令回显误认成结束标记的竞态，未降低字节完整性断言。
- 前端：179 文件 / 1787 项、类型检查、lint（100 warning、1 info、无错误）和生产构建通过。首次无限并发时 3 项 CPU 密集测试超时，限制两个 worker 后全部通过，未改超时阈值。Rust：`cargo clippy --workspace --all-targets --locked -- -D warnings` 及 `cargo test --workspace --locked` 通过，共 633 项（core 431、server 92、桌面 110），无失败或忽略项。
- 集成：`pnpm check:ai-stream`、`pnpm check:terminal-behavior`、`pnpm check:rmux-isolated`、`pnpm check:fs-index` 通过。真实 Chromium 生产页及终端输入通过，WebGL driver 为 ANGLE/SwiftShader；这不是硬件 WebGPU/移动设备验收。
- 剩余：其余上游差异逐项审计，macOS/Windows 编译及剪贴板实机，真实 GPU/HTTPS、移动 IME、Codex/Claude TUI、Room mesh、文件树 GUI 与长时间资源度量。未把这些项标为完成。
- 回退：提交后在集成分支使用 `git revert <本轮提交>` 撤销修复，避免改写共享历史；`.phantom/agents.yaml` 属于用户改动，应保留。`b7607b22` 为本轮修复前的本地合并基线；旧 `/tmp/phantom-terax-v090` 与原服务目录保持可用，8099 原服务未被替换。

## 本轮进度（2026-11-06）

- 本轮转向 P2.2/P2.3，避免仅在 P2.4 内累积子项。新增 `terminalBehavior.integration.test.ts`，对 SIMD/scalar 两版真实 WASM 和两个 backend 标签组合执行 24 项跨组件检查：碎片 UTF-8/OSC、语义路由与块元数据、中文选区、分步历史搜索/控制器恢复、链接安全和 alternate-screen。它不创建 GPU surface，不声称 WebGPU/WebGL 真正渲染通过。
- 发现并修复 `useAiLiveBridge.readLeafBuffer` 未检查 Privacy 的缺口：原先只有活动终端 getter 拒读私有 tab，任意 leaf getter 仍可读。抽出纯 `readTerminalContext`，统一用最新 tabs 校验叶节点所有权、隐私和歧义 id，然后请求 300 行并脱敏。新增 10 项纯函数及 4 项真实桥 getter 接线测试，覆盖私有非活动分屏、关闭后残留 ref、切 pane 和实时切 Privacy；另补 5 项真实链接 handler 的危险 scheme、浏览器打开选项和原生失败焦点回归，共新增 43 项前端测试。
- 新增 `pnpm run check:terminal-behavior`，复用有端口预检/HTTP 指纹的独立复制测试后端，在随机 XDG 运行目录内创建并清理自己的 `/bin/sh` PTY。真实 shell 输出受控 OSC 7/52/133/777，经 WebSocket 二进制 poll/ack、上游 hub/receiver、真实 WASM 和产品语义路由/命令块，验证 cwd、clipboard 回调、命令块和有序退出；累计 received/processed 相等且 pending 为零。测试只打印 agent markers，不启动真实付费 coding-agent，不写 OS 剪贴板。
- 同一隔离后端实测 Room 发帖、另一 WebSocket 收广播、断开后按 seq 回放缺失消息及 epoch 一致；未配置 peer/mesh 或向现有 Room 发帖。HTTP 上传/inline 下载验证 SVG/图片的 MIME/sandbox/nosniff、HTML 强制 attachment 及字节一致；图片使用传输边界 fixture，不能算图片解码/浏览器渲染验收。
- 本轮复验：前端 178 文件 / 1776 项、类型、lint（100 warning、1 info、无错误）、生产构建通过；Rust core 399 项 / server 92 项，共 491 项通过；独立 `check:terminal-behavior` 完整通过。没有改动 Rust，本轮不将原有严格 Clippy 或桌面 GLib 缺口标成解决；真实 GUI/设备、Room mesh、实际 CLI 通知和资源度量仍未完成。未部署、未重启现有服务、未操作现有 rmux 会话。

- `dfcfa30` 和 `cfa77ce` 已推送到 `origin/integrate/terax-v0.9.0`，未合并 main。随后继续 P2.4 网络差异审计，先迁入共享 core 的地址分类安全子项；完整 AI 流控、请求取消/所有权、原生二进制 IPC 和连接池仍未迁移，已在清单拆开列出。
- 新 `net/ip.rs` 对齐上游 IPv4-mapped/compatible、SIIT、NAT64、6to4、Teredo 的内嵌 IPv4 分类；Teredo 取服务端/客户端中限制最严的类别。非全局 IPv6、IPv4 文档/保留地址不再当作公网；保留显式私网授权，metadata/link-local 即使授权仍拒绝。IPv6 URL 主机先去方括号再解析，避免将字面地址交给 DNS；metadata 主机名去末尾点后匹配。
- 重定向判断抽为纯函数。与上游相比进一步收紧：命名目标必须已有经过校验的固定解析，即使 `allowPrivateNetwork=true` 也不跟随未经分类的新主机名；可直接分类的地址字面量仍按公网/私网授权处理。非法 scheme/userinfo、metadata 和超长重定向链继续拦截。被阻止的重定向返回原 3xx，未更换 WebSocket/Tauri 命令或 AI 流事件结构。
- AI HTTP 请求/流和 `lm_ping` 使用 `no_proxy()`，不继承 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 等隐式代理，避免代理重新解析目标使 DNS pin 失效。这是有意的兼容性收紧，不是已支持代理配置迁移；需要环境代理的部署尚需后续显式、可校验的代理方案。本轮未修改其他 Room/peer 网络客户端和用户配置文件。
- 新增 18 项测试，网络共 26 项通过。真实随机 loopback 端口覆盖私网默认拒绝/显式授权、本地模型 ping、相对重定向、命名 DNS pin、阻止新主机/metadata 重定向及流式事件结构；独立子进程覆盖设置环境代理后 request/stream/ping 仍直接访问测试目标，测试未修改父进程环境，也未访问真实 metadata 服务或调用付费模型。
- 本轮复验：Rust core 399 项、server 92 项，共 491 项通过。前端首次与 Rust 编译/生产构建并行时，既有 Ghostty 100000 次 renewal 压力测试超出 5 秒；其余 1732 项通过。停止编译竞争后以原 `--maxWorkers=2` 和原超时全量重跑，175 文件 / 1733 项通过，未跳过或放宽测试。类型、lint（100 warning、1 info、无错误）及生产构建通过；严格 headless Clippy 仍仅报原有 10 项诊断，桌面 GLib/真实设备缺口保留。未部署、未重启现有服务或操作 rmux 会话。

- 进度清单与文件索引/HTTP 上传改动已提交为 `dfcfa30`，未推送、未合并 main。随后继续完成复制后自动多选：剪贴板和原生拖放统一等待目标刷新、展开及 React 行列表提交后，仅选择当前树中可见的成功副本，第一项作为锚点并滚动定位；隐藏文件仍服从当前显示偏好，全失败或无可见项保留原选区。
- 原生拖放增加作用域代际，切根目录或工作区后再切回也不恢复旧复制的刷新/选区回调；刷新期间隐藏、卸载或切环境时忽略结果。刷新异常与复制异常分开提示，避免已复制成功却显示为复制失败。已开始的文件 IO 不声称可取消；浏览器上传入口及其字节传输保持不变。
- 新增 22 项自动回归，覆盖粘贴等待刷新、多成功项、原生部分成功、刷新失败、根目录/环境/隐藏/卸载失效、切走再返回的过期结果，以及真实 `useFileTree` + 粘贴 + 选区 hook 组合。前端全量 175 文件 / 1733 项、类型、lint（100 warning、1 info、无错误）和生产构建通过；本阶段未修改 Rust，沿用本轮 core 381 / server 92 项通过记录。以上为受控 IO 的自动验证，不替代原生平台编译和真实 GUI 选区/滚动验收；未部署、未重启现有服务或操作现有 rmux 会话。

- 接续 `e06bc2e` 后工作区中已有的文件索引改动，完成检查、补测及验证，未覆盖既有实现。`phantom-core::fs::index` 为搜索和文件列表共享缓存，key 包含 canonical root、隐藏文件偏好和深度；调用方的展示路径和 WSL 路径语义保留。最多保留两个索引，每个路径文本最多 16 MiB、扫描最多 50000 项；无后台扫描、定时器或新增 watcher。TTL 在查询时检查，不承诺空闲 45 秒后自动释放缓存；被在途查询持有的 Arc 不计入两槽保留数。
- 文件变更前后均使相关缓存失效，扫描期间收到变更的结果只返回本次请求，不发布为可复用缓存。忽略重型目录内部事件，保留 `.git/info/exclude` / `.git/config` 失效；本轮补上父级 `.gitignore` / `.ignore` / Git 忽略配置变动对嵌套搜索根目录的失效测试。未被现有 watcher 观察到的外部变更、全局忽略规则变更仍靠下次查询的 TTL 重建，不声称递归实时监控。
- HTTP 上传接入同一失效机制，同时将“先检查空闲文件名再创建”改为 `create_new` 独占创建，避免并发上传截断其他文件或跟随已有/悬空 symlink。新增并发无覆盖及 symlink 回归测试，保留上传文件名后缀和浏览器下载响应行为。未扩大文件 API 的访问权限。
- `pnpm run check:fs-index` 复用独立复制测试环境，增加 warm cache、全部应用内变更入口、深度/隐藏/截断、父级忽略规则、HTTP 并发上传/下载及真实 watcher 失效验证。修正测试脚本对 `git init` 已创建 `.git/info` 的重复创建错误，完整检查通过；仅启动并清理本脚本自己的测试后端，无 PTY/rmux 会话。
- 本轮复验：前端 175 文件 / 1711 项、类型、lint（100 warning、1 info、无错误）及生产构建通过；Rust core 381 项、server 92 项通过，共 473 项。严格 headless Clippy 仍报原有 10 项诊断；`cargo check -p terax --locked` 仍在 `glib-sys` 因缺少 `glib-2.0.pc` 失败，未进入桌面应用代码验证。未部署、未重启现有服务、未操作现有 rmux 会话；其余 P2 和 P3 的真实 GUI/平台/资源验收保持未完成。

- 已提交并推送复制基础层：`b344d8d`，远程分支仍为 `origin/integrate/terax-v0.9.0`，未合并 `main`。以下剪贴板工作为其后续变更。
- 系统剪贴板源码接入：Tauri 专有 `fs_clipboard_paths`，Linux 使用 GTK URI/GNOME 文件列表，macOS 使用 NSPasteboard 文件 URL（含 Finder 引用 URL 解析），Windows 使用 CF_HDROP。Linux/macOS 调度到主线程并限制等待时间，Windows 读取在阻塞任务执行；不向 headless WebSocket 暴露服务端 OS 剪贴板。
- 共享 URI 列表解析器限制 1024 个来源和 1 MiB 输入，去重并拒绝远端 URI 主机、普通文本路径、query/fragment、NUL 和无法无损表示的路径。Unix 文件名中的字面反斜杠被明确拒绝，避免规范化后指向另一个文件；GNOME `cut` 标记也仅复制，不删除来源。macOS/Windows 读取也对路径数量和累计字节数设限；Windows 原生 CF_HDROP 可保留用户选中的 UNC 路径，不将 URI 远端主机误解为本地路径。
- 文件树桌面 Ctrl/Cmd+V 和右键 Paste 已接线，空目录可粘贴，目标取选中目录/选中文件的父目录/根目录；成功刷新时展开目标目录。粘贴期间合并重复触发；读取剪贴板期间切根目录、切环境、隐藏或卸载后不再启动复制，已开始的复制仍使用原环境且不刷新新页面。复制已成功但刷新失败时明确告知，不重试复制。原生三平台 IO 尚无本机编译及 GUI 通过证据，不能据此关闭系统剪贴板验收。
- 浏览器保留文件事件粘贴与 Upload Files，经 HTTP 发送客户端文件字节，不把客户端文件路径当服务端路径。重命名/搜索输入框及 contenteditable 保留正常文本粘贴；文件选择器结束时若当前环境/根目录已变化或文件树已隐藏，则取消上传。批量上传固定初始 workspace，避免后续文件发往另一 WSL distro。
- 本轮验证：前端 175 文件 / 1711 项、类型、lint（100 warning、1 info、无错误）及生产构建通过；Rust core 362 项、server 90 项通过，共 452 项。隔离 `check:fs-copy` 新增 headless 拒绝 `fs_clipboard_paths` 的真实 WebSocket 边界检查并通过。URI 解析与 hook/HTTP 流程使用自动测试，不代表系统剪贴板或浏览器 GUI 实测；复制结果自动选中、桌面 GLib 依赖、严格 Clippy 原有 10 项及平台设备验收缺口保留。

- P2.4 复制基础层：上游有界批量复制迁入 `phantom-core::fs::copy`，返回 `{created, failed}`，单项失败不阻断其余项。默认冲突不覆盖；显式 `renameOnConflict` 选择 ` copy` / ` copy N` 副本名。每项在目标同文件系统临时目录完成复制后才发布，失败清理暂存；这不是整个批次的原子事务，也不承诺断电持久性。
- 目标必须在捕获且已授权的绝对 workspace root 内，canonicalize 后再次检查目录和 symlink 目标；源是用户明确选择的绝对 OS 路径，允许来自工作区外，复制 symlink 本身而非遍历其目标。上限为 1024 个来源、128 层递归和 10000 个副本后缀；拒绝 socket 等特殊文件，保留普通文件权限。
- 共享 `fs::publish::rename_no_replace` 使用 Linux `renameat2(RENAME_NOREPLACE)`、macOS `renamex_np(RENAME_EXCL)` 和 Windows 不带替换标志的 `MoveFileExW`；普通 `fs_rename` 也改用此原语，支持移动悬空链接并拒绝覆盖目标链接。仅 Linux 有本轮执行证据，macOS/Windows 平台编译及实机验证未完成。
- WebSocket 修正 `destDir` 与原 `to` 不匹配的问题，兼容 `to` 参数名但强制要求 `root`，无 root 的旧请求会被拒绝。Tauri 复制命令移至阻塞任务，二者复用同一 core。原生拖放携带 root，报告部分失败且仅在当前根目录仍匹配、有成功项时刷新；浏览器 HTTP 上传逻辑保留。系统剪贴板入口、索引缓存及其失效通知仍待迁移，不能关闭完整剪贴板任务。
- 新增 `pnpm run check:fs-copy`：自动构建 worktree 自己的后端，先检查独立端口 `19889` 空闲，以 `/tmp/opencode/phantom-copy-check-*` 隔离 XDG 状态和测试文件，再校验独有 HTTP 指纹后连接 WebSocket。实测授权、缺失 root 拒绝、参数兼容、并发同名复制、部分失败、数量上限及 symlink 安全；只清理自己创建的进程和目录，不创建 PTY、不接管 rmux 会话。
- 本轮检查：前端 173 文件 / 1687 项、类型、lint（100 warning、1 info、无错误）、生产构建及 `check:fs-copy` 通过；Rust core 356 项、server 90 项通过，共 446 项。严格 Clippy 仍停在原有 10 项诊断；桌面 GLib 依赖与真实 GUI 验收缺口保留。

- P2.4 资源优化：侧栏折叠状态现在传给文件树；隐藏时解除所属目录监听和文件事件订阅，停止新目录请求，保留树和展开状态。显示时按当前偏好统一补同步一次，不因普通重渲染重复请求。浏览器上传和原生拖放监听随可见性拆装，文件搜索暂停但保留查询；Git 装饰的路径规范化请求在隐藏时停用。
- 保留 Phantom 的共享 watcher 串行注册/移除；文件树监听显式绑定申请时的 workspace，切换 Local/WSL 后仍在原环境释放。展开缓存按 workspace/root 隔离；切目录、隐藏或卸载时使旧读取失效，同目录只接受最新请求，删除子树后拒收该子树的在途结果。已发出的 IPC 不声称可取消，只忽略过期结果并停止后续请求。
- 新增 23 项自动回归：监听所有权、请求生命周期、侧栏折叠状态、隐藏/显示重同步、快速切根目录和工作区、过期事件及拖放注册、浏览器上传监听恢复。Hook 生命周期使用受控调度与模拟 IO，不代表真实浏览器/桌面 GUI 验收；真实 CPU/内存度量和设备检查仍留在 P3.4/P3.5。
- 本轮复验：前端 172 文件 / 1681 项、类型、lint（100 warning、1 info、无错误）及生产构建通过；Rust core 341 项、server 88 项通过。严格 headless Clippy 仍停在原有 10 项诊断，未修改 Rust 源码或压制告警；桌面依赖和实机验收缺口保留。未部署、未启动或重启服务、未操作现有 rmux 会话。

- 接续文件树多选迁移，解决终端拖放合并冲突；终端仍兼容单路径及多路径 shell 引号粘贴，浏览器上传目的地改用仍处于选中的锚点，修复残留 `selectedPath` 导致的类型错误。
- 批量移动和删除先去重并折叠已选父子路径，避免同时操作父目录与后代；移动规划独立为纯函数，覆盖 Windows 分隔符、递归目标和同名冲突。移动前重新读取完整目的目录（包含隐藏文件），不再仅信任可能过期或不完整的树缓存；部分失败也会提示。
- 批量替换先将原目标移到同目录 `.phantom-move-<uuid>.bak`，源移动失败时尝试恢复；恢复或备份清理失败时保留备份并显示路径。替换流程固定使用发起时的 workspace，避免确认提示期间切换环境后误操作。此流程不是跨 IPC 的原子事务，页面中断后可能留下需人工恢复的备份。
- 本轮检查：前端 169 文件 / 1658 项测试、类型检查、生产构建通过；lint 无错误（102 warning、1 info）；新增纯函数与变更执行测试通过。Rust core 341 项、server 88 项通过，共 429 项。
- 严格 headless Clippy 仍停在未改动的 `fs/file.rs` dead code、`history/mod.rs` 和 `shell/mod.rs` needless borrow、`pty/agent_detect.rs` 缺少 Default 等 10 项诊断；桌面库测试在 `glib-sys` 构建阶段缺少 `glib-2.0.pc`，未进入应用代码验证。不以此关闭 P2.4 或 P3.2。
- 下一步仍需文件树剪贴板/索引等其余上游差异对齐，以及真实浏览器多选、拖放、上传和冲突确认 GUI 验证；移动 IME、HTTPS、桌面和资源验收缺口保持未完成。本轮未部署、未重启服务、未操作现有 rmux 会话。

- 编辑器文件级 watcher 隔离 WebSocket 端到端实测通过（`scripts/check-fs-watch.mjs`，`pnpm run check:fs-watch`，node13/agy 接棒推进）：
  - 环境隔离：独立运行目录 `/tmp/phantom-terax-v090-watch-test`，端口 `19889`。
  - 覆盖四大核心阶段：
    1. Phase 1：调用 `workspace_authorize` 授权测试目录，调用 `fs_watch_add` 成功注册目录与单文件 `{paths: [dir], files: [file]}`。
    2. Phase 2：外部直接修改被监听文件，经独立 WebSocket 实时接收到 `fs:changed` 事件，其 `paths` 数组精确包含被改写文件绝对路径。
    3. Phase 3：调用 `fs_watch_remove` 释放目录与文件监听；后续二次改写文件，验证不会再收到任何 `fs:changed` 事件，确认引用与监听已正确卸载。
    4. Phase 4：安全校验拦截：对 `/etc/shadow`、`../../etc/passwd` 等工作区外的越权文件路径，未授权即被严格拦截，不会泄漏监听。
  - 检查证据：`pnpm run check:fs-watch` 通过，`pnpm run check-types` 0 错误。

- 编辑器文件级 watcher：上游 WatchSet、目录事件归并、文件时间戳比较、Linux/Windows notify 和 macOS FSEvents 后端迁入共享 core。打开文件注册随目录订阅增减，文件注册检查工作区授权及 symlink 目标；父目录删除后仍可释放文件注册。Phantom 搜索当前直接扫描磁盘，本阶段未引入上游文件索引缓存。
- Tauri 监听注册和移除移至阻塞任务，前端串行发送，避免快速关闭时 remove 先于 add 完成。前端 166 文件 / 1628 项、类型、lint（无错误，102 警告、1 info）和生产构建通过；Rust core 341 项、server 88 项通过，覆盖真实文件修改事件、监听释放及未授权路径。隔离 WS 事件验证由 node13/agy 接手；macOS 专用后端尚无目标平台编译及硬件实测证据，不能关闭桌面真实验收。
- AI 会话文件持久化已接入共享 core、Tauri 和浏览器命令：复用上游原子替换、上一版备份、损坏恢复与迁移校验；浏览器旧 localStorage 消息逐项导入，成功后才移除旧消息键。会话列表和 activeId 仍沿用现有元数据存储，不代表已完成跨设备历史同步。
- 会话写入按 session 串行并合并待写快照，失败保留最新快照并提供重试；显式 flush 请求 durable 保存。桌面关闭最多等待 4 秒，失败时提示并允许继续关闭；浏览器隐藏/pagehide 尽力 flush，强制关页无法保证异步请求完成。
- 本阶段验证：core 全量 314 项、server 88 项通过；前端 165 文件 / 1627 项通过，类型、lint（无错误，102 警告、1 info）和生产构建通过。首轮并行编译期间 3 项既有终端压力测试超时，降低测试并发后全量复跑通过。
- 桌面库测试重建在 `tao`/`tauri-utils` 依赖编译阶段报 `Disk quota exceeded`，尚未进入应用代码验证；已请 node13/agy 补桌面构建与隔离 WebSocket 保存/重载/删除/非法 id/鉴权验证。真实 macOS 图形验收仍未完成。
- 编辑器文件监听生命周期：卸载时释放已申请的目录监听，忽略异步监听注册期间卸载后到达的文件事件；新增卸载回归测试。前端全量 163 文件 / 1605 测试通过，lint 无错误（104 警告、1 info）；文件级 macOS watcher 与其余编辑器上游差异仍待迁移。
- node13/agy 已在独立环境复验 `8d87751`：Git 52 项和 phantom-core 全量 284 项库测试通过，解除此前该提交的 Cargo 测试缺口。
- P2 Git：共享 core 改用 `--untracked-files=normal`，按上游预算展开小目录（总计 500 文件、16 目录）；单次列表超过 256 KiB 时终止 git 并保留折叠目录。新增预算、解析、实际仓库及截断回归测试。限额算法独立编译验证 7 项通过；完整 Cargo Git 套件因 `Disk quota exceeded` 尚未完成，已请 node13/agy 在独立构建环境复验，不能据此关闭 P2.4。
- 协同分工：Codex 负责 Git、编辑器同步及 AI 持久化源码；node13/agy 已确认负责隔离浏览器集成、移动 IME、HTTPS、WebGL 回退和资源度量。macOS 遮挡、休眠恢复及原生渲染保留真实硬件验收缺口。
- AI 流式保存调度已迁入（`a5968b1`），相关测试 6 项和 TypeScript 检查通过；会话文件持久化整体迁移仍未完成。
- 桌面窗口状态接入上游 presentation 快照和 macOS 遮挡、休眠、窗口缩放监听，隐藏状态直接通知共享 PTY hub，并在页面重载后恢复。PTY 创建与窗口销毁串行，避免销毁时遗留正在创建的进程；macOS 取消关闭主窗口不再误关设置窗口。桌面实际 GUI 与 macOS 验证仍待完成。
- 桌面 Rust 库测试 48 项通过（含窗口状态、协议和创建/销毁并发测试）。完整 Cargo 测试构建静态库时超出本机磁盘配额；本次使用关闭调试信息的 `cargo test --lib`。
- P2 差异梳理：上游文件树批量移动、剪贴板和索引，Git 未跟踪文件处理，编辑器同步，以及 AI 会话文件持久化、网络流控等尚需逐项对齐；不能仅凭终端测试关闭 P2.1 至 P2.5。移动 IME、Room、媒体、HTTPS 和 WebGL 回退仍需实际环境验证。
- Ghostty 渲染器和终端入口已接入，浏览器 WebSocket 与 Tauri 原生 PTY 流控均已接线；保留移动键盘和稳定会话字段。
- 前端 1595 项测试通过，新增浏览器 PTY 测试 3 项通过；生产构建和 lint 通过（lint 有既有警告）。Rust 本轮 275 + 43 项测试通过。真实本地 PTY 4 MiB 输出与退出顺序冒烟通过。
- 浏览器实测 shell 输入、输出、分屏和缩放；移动输入、WebGPU 回退及中文 IME 的完整复验结果尚待确认。
- 待完成：独立 rmux 重连与回放、桌面 GUI、真实设备 IME、长时间资源表现，以及其余 v0.9.0 功能对齐；本次提交为阶段进度。

## 执行记录

### 2026-11-06

- 完成 AI 会话文件持久化隔离环境端到端验证（`scripts/check-ai-persistence.mjs`，`pnpm run check:ai-persistence`）：
  - 环境隔离：使用独立数据目录 `/tmp/phantom-terax-v090-ai-test/data`，端口 `19889`，完全隔绝生产数据。
  - 覆盖五大核心阶段：
    1. Phase 1：通过 WebSocket 调用 `ai_session_save` 验证原子写入落盘，校验 `$XDG_DATA_HOME/phantom/conversations/session-<id>.json` 文件结构与原始内容一致。
    2. Phase 2：调用 `ai_session_load` 成功重载 JSON 字符串；不存在的会话 ID 正确返回 `null`。
    3. Phase 3：保存新版本内容，验证原子轮转生成 `.bak` 备份文件且保留上一版本完整数据。
    4. Phase 4：安全校验与路径穿越拦截：非数组 JSON 拒绝、语法损坏 JSON 拒绝；对 `../../etc/passwd`、`../escape`、空字符及非法特殊字符 ID，在 save/load/delete 各命令下均严格拦截报错。
    5. Phase 5：调用 `ai_session_delete` 验证主会话文件与 `.bak` 备份文件均从磁盘物理抹除，二次加载返回 `null`。
  - 桌面库构建说明：系统缺失 `glib-2.0` / `webkit2gtk` 等 C 开发头文件（由于本机构建机无桌面 GUI 运行时环境），桌面二进制与 GUI 实体验收继续按既定策略保留硬件与桌面环境缺口。

- 完成隔离浏览器端集成验证自动化套件落地（`scripts/check-browser-integration.mjs`，`pnpm run check:browser-integration`）：
  - 环境隔离：后端监听 `127.0.0.1:19889`，Vite integration mode 监听 `127.0.0.1:15174`，通过 WebSocket 反向代理转发 `/ws`。
  - 覆盖五大核心阶段：
    1. Phase 1：未签发 token 返回 403 Forbidden，校验鉴权边界。
    2. Phase 2：验证 WebGPU 不可用环境平滑回退至 `ghostty-webgl`，并验证单色字形图集 key 在主题切换时复用。
    3. Phase 3：4 MiB 本地 PTY 连续数据流经 15174 代理推流，Ghostty WASM 分块消费与 credit 回收，4,194,304 字节无损解析，有序退出码 0，未结清字节为 0。
    4. Phase 4：多 Pane 分屏并发流验证，同时打开两个独立 PTY 并行推流与解析，验证 `PtyOutputHub` 解复用隔离，无串流且无信号量许可枯竭。
    5. Phase 5：移动端触摸单击（400ms 内且位移 <= 10px）聚焦识别与长按/滑动防误触逻辑验证。
  - 产物构建验证：`pnpm run build` 耗时 17s 构建成功，`pnpm check-types` 0 错误。

- 完成 rmux 独立重连、历史回放及 Ghostty WASM 流控端到端验证与问题修复（P1.4 / P1.5）：
  - 修复 `rmux_pty.rs` 在处理非 UTF-8 二进制键盘输入时的严格解码报错，降级为 lossy 转换以兼容特殊键序列。
  - 调整 `pty_stream.rs` 中 `replies` 信号量许可数（从 2 提升至 4），避免高并发 poll/ack pipeline 耗尽许可导致 WebSocket 异常关闭。
  - 新增独立端到端自动化测试脚本 `scripts/check-rmux-stream.mjs`（`pnpm run check:rmux-stream`），全程使用独立 rmux daemon socket `/tmp/rmux-1001/test-rmux-v090` 和端口 `19889`，绝不干扰生产实例。
  - 自动化验证 5 阶段完整通过：
    1. Phase 1：稳定会话创建 (`pty_open` 带 session name)，验证 `pty_sessions` 状态为 `attached: true`。
    2. Phase 2：256 KiB 数据洪流经 rmux 管道推流进入 Ghostty WASM，无损解析且 credit 正常回收；验证 QueryFilter 成功拦截 DA/DSR 设备查询（`\x1b[c`、`\x1b[5n`、`\x1b[6n`）。
    3. Phase 3：`pty_close` 分离语义，验证 session 保持运行 (`attached: false`)，shell 不被杀死。
    4. Phase 4：`pty_attach` 重连接管现有会话，验证 `PaneOutputStart::Oldest` 历史回放无损流向全新 Ghostty WASM 实例。
    5. Phase 5：接管后交互执行命令与有序退出（退出码 0），清理隔离资源。
  - 检查证据：
    - `pnpm run check:rmux-stream` 通过。
    - `pnpm run check:ghostty-core` 通过（SIMD 与 scalar）。
    - `pnpm check-types` 通过。
    - `pnpm test` 通过（160 测试套件，1598 项测试）。
    - Rust 测试通过（phantom-core 275 项，phantom-server 88 项，总计 363 项）。

- 按用户要求迁入独立 worktree。迁移暂存备份：`5160343f9125e6ee82b03b61be3893b375a2795b`。原服务目录恢复为 `main`，只保留原有 `.phantom/agents.yaml` 修改。
- 整合代码的 tracked diff 与迁移暂存逐项一致；新文件也已恢复到独立 worktree。停止原目录的构建任务，后续检查只在 worktree 执行。
- 独立前后端已启动：`127.0.0.1:15174` 和 `127.0.0.1:19889`。后端本地 PTY、Room 和 secrets 使用独立配置/状态；未接管现有 rmux 会话。
- 首次 debug 构建触及 /tmp 配额；仅清理独立 worktree 的 target，改为 `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0` 后可继续构建，产物显著缩小。
- 依赖副本使用已有 `/path/to/pnpm-store` 完成离线 frozen-lockfile 安装，未更改原服务 node_modules。
- 已迁入共享流控 core 与浏览器适配；新增异步 credit / close 测试和 WebSocket 二进制回复、连接中取消、断开拒绝 pending 请求测试。
- 检查：84 个前端测试文件 / 620 个测试通过；headless Rust 275 + 88 = 363 个测试通过；TypeScript 和生产构建通过，产物仅位于 worktree。
- `node scripts/check-pty-stream.mjs` 使用实际前端 transport、PtyOutputHub、PtyOutputReceiver、真实本地 shell 和 Ghostty WASM，验证 4 MiB 连续输出、异步 parser credit 与尾部输出后退出；不是模拟 transport 测试。
- 冒烟复验经过前端 `15174` 的 WebSocket 代理到后端 `19889`，同时确认未签发 token 返回 403。两个 HTTP 地址均返回 200。
- Lint 与原基线一致：106 条 warning、1 条 info，无新增诊断。`pnpm run build` 产物位于独立 worktree 的 `dist/`。
- 新共享 stream 模块独立 Clippy `-D warnings` 通过。headless 整体严格 Clippy 停在已有 fs/history/shell/agent_detect 告警；未顺带修改这些文件。Tauri 全量检查尚未完成。
- 当前阶段：P0.6、P1.2a、P1.3a 已完成；下一项 P1.1b / P1.3b，导入渲染器并迁移终端 UI、移动输入和稳定会话字段。测试页仍使用旧终端入口，尚未启用 Ghostty UI。

- 上游固定为 `d0b569c0aa401c79b08ae8fc048be797739c88b4`。共同祖先 `ebed02ef82f5f55cb36e96b39d9b62a0ddb51631`。
- 共同祖先后：Phantom 66 个独有提交，上游 211 个独有提交。
- 按不做重命名识别的口径，上游涉及 663 个路径；Git 默认识别后为 662 项变更。
- 使用 `git -c merge.renames=false merge-tree --write-tree --messages --name-only HEAD upstream-v0.9.0` 预演，发现 57 个冲突路径。预演不修改工作区和分支。
- 冲突处理顺序：`phantom-core` 与 Tauri Rust 适配 → PTY bridge → Ghostty 终端入口 → App/Vite → 其他 UI 与文档。
- 重点冲突：原 `src-tauri/src/modules/{fs,git,history,proc,pty,shell}` 已抽到 `crates/phantom-core/src/`；不能恢复一套互不一致的旧路径来消除冲突。
- 终端冲突：`TerminalPane`、`TerminalStack`、`index`、`pty-bridge` 需迁移 Phantom 会话字段；上游删除 `rendererPool` 和 `useTerminalSession`，其移动输入与重连行为要迁入新路径后才能移除。
- 其他冲突：`.gitignore`、App、main、AI store、command palette、explorer、space serialize、settings、Vite；保留 Room、transport alias 和 Phantom 品牌。
- 开始 P1.1a：独立引入 Ghostty core，先验证二进制资源，再切换终端入口。P0.5 的 Rust 检查仍在补齐依赖，P0.6 的背压方案仍需落地验证。
- 完成 P1.1a：`packages/ghostty-core` 32 个文件逐个计算 Git blob hash，与目标版本完全一致，共 3,179,802 字节；包含上游许可证、源码来源与构建补丁。
- 新增 `scripts/check-ghostty-core.mjs`，执行 `pnpm run check:ghostty-core`。SIMD/scalar 两版均通过 SHA-256、分块 UTF-8/ANSI、DSR 光标回复、alternate-screen 恢复与 resize 保留内容检查。
- core 暂未接入运行时；workspace 依赖与锁文件变更推迟到 P1.1b 首次实际引用时，避免无关依赖重新解析。本阶段 `pnpm-lock.yaml` 与 `pnpm-workspace.yaml` 无改动。
- 本阶段复验：TypeScript 通过，80 个测试文件 / 576 个测试通过，Ghostty 自检通过，`git diff --check` 通过。生产构建基线已通过，本阶段未改前端运行时。
- Rust workspace 基线尚未完成：离线缺少锁定依赖，联网停留在 crates.io index 更新且未进入编译；已停止该次检查。未将 Rust 检查标为通过。
- core 引入阶段结束时：P0.1–P0.4、P1.1a 已完成；当时 P0.5 部分完成，P0.6 尚待推进。后续进度见本节前述传输适配记录。

### 2026-11-05

- 基线分支建立，起点 `d91bbe2`；创建文档前工作区干净。
- 读过项目约定并通过 CodeGraph 梳理 PTY、rmux 与浏览器传输调用路径。
- 开始 P0.3：普通沙箱拉取因 `.git/FETCH_HEAD` 只读失败，改用权限提升拉取。
- 已添加 `terax-upstream` remote。完整历史包含大量二进制，改用 `blob:limit=256k` 按需拉取。
- 前端基线：类型检查通过；80 个测试文件、576 个测试通过；生产构建通过。
- Biome 基线：433 个文件，106 条已有 warning、1 条 info，退出码 0。RTK 的 lint 路由误调用 ESLint，实际使用 `rtk proxy pnpm run lint` 验证。
- Rust 基线：离线缺少锁定的 `async-compression 0.4.52`；正在联网补齐依赖后检查。
- 当前阶段：P0。尚未融合业务代码。
