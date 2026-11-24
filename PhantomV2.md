# Phantom V2 架构设计与落地方案

## egui 纯 Rust 重写，基于当前 Phantom 版本演进

**版本：v2.0（落地版，基于当前代码盘点重写）**
**方向决策：egui 纯 Rust UI 重写；复用 `phantom-core` + `phantom-server` 全部后端逻辑与 rmux；替换 React/TS/Tauri UI 层。**

---

## 0. 本次修订说明

上一版（`v1.0 Draft`，代号 Terax-RS）是一份**绿地架构畅想**：抽象描述了 Room / Agent / Session / Task / Event 模型与纯 Rust/egui 技术栈，但几乎不引用当前 Phantom 的实际代码。

本版**基于当前代码库盘点重写**，目标是"**基于当前 Phantom 版本开发 V2**"。核心变化：

- 用**现状盘点**替代绿地假设：当前代码已经实现了上一版当作"未来"的大部分东西（Room、多 Agent 编排、rmux 持久会话、mesh、WebSocket 协议）。
- 明确 **egui 重写的真实边界**：不是从零重建，而是**保留后端、替换 UI**。~70k 行 React/TS + JS transport 换成 egui + Rust 客户端；`phantom-core`、`phantom-server`、协议、Room、orchestrator、rmux、mesh 原样复用。
- 每个 V2 概念给出**↔ 现有代码映射**（文件、命令名），并附**复用清单**、**差距分析**、**分阶段落地路线**。

命名统一：项目名 **Phantom**，crate 前缀 `phantom-*`（上一版的 `terax-*` 命名弃用）。

---

# Part 1 — 现状盘点（V2 的地基）

## 1. 项目全貌

当前 Phantom 是上游 **Terax**（Tauri 2 + Rust + React 19 的 AI 原生终端 / ADE）的 fork，在其上做了三件关键事：

1. 把终端/fs/git/pty 等逻辑从 `src-tauri` 抽离进 **`crates/phantom-core`**（无 UI 框架依赖的领域层）。
2. 新建 **`crates/phantom-server`**：headless WebSocket 网关，让同一套核心逻辑通过浏览器/远程访问，并加入 Room、多 Agent 编排、rmux 持久会话、peer/mesh 多主机。
3. 前端 `src/modules` 增加 room / orchestration / agents / spaces 等模块。

```
Phantom workspace
│
├── crates/phantom-core        领域层（无 UI 依赖）
├── crates/phantom-server      headless WS 网关（Room/rmux/orchestrator/mesh）
├── src-tauri  (crate "terax") 桌面 Tauri app —— main 上当前不编译（见 §4）
└── src/                       React 19 前端（~70.6k 行 TS/TSX，23 模块）
```

> **关键判断：当前架构已经天然满足"Core 与 UI 分离"。** `phantom-core` 的模块文档明确写着"no UI-framework dependency"；`phantom-server` 是 headless；UI 与后端之间的接缝是一套 **93 个 invoke 命令** 的稳定契约。这正是 egui 重写能"只换 UI、留后端"的根本原因。

## 2. phantom-core —— 领域层（复用，几乎不动）

`crates/phantom-core/src/lib.rs` 导出的模块：

| 模块 | 职责 |
|------|------|
| `agent` | AI CLI Agent 探测与 hook 注入；OSC 777 状态标记；claude/codex/gemini 的 delivery 差异 |
| `pty` | PTY 会话（`session.rs`）、shell 初始化脚本、agent 探测（`agent_detect.rs`）、DA 过滤 |
| `fs` | 文件树、读写、glob、grep、search、watch、mutate |
| `git` | status / diff / log / branch / commit / operations / parser |
| `history` | shell 历史记录、解析、建议 |
| `proc` | 后台进程 / job 管理 |
| `net` | 出站 HTTP（SSRF/DNS-rebinding 防护） |
| `secrets` | OS keychain 密钥存取 |
| `shell` | shell 检测与封装 |
| `workspace` | 工作区根 / 路径解析 |
| `path` | 路径守卫 |

**结论：V2 直接依赖 `phantom-core`，无需重写。** 它已经是"纯领域 + 无 UI"的形态，正是上一版文档 §17 `terax-core` 想要的东西，而且已经存在。

## 3. phantom-server —— headless 网关（复用为 V2 的后端标准）

`crates/phantom-server` 是当前 Phantom 最有价值的资产，也是 V2 的后端标准实现。它已经实现了上一版文档当作"未来 Room Protocol / Session / Agent Runtime"的全部内容：

| 文件 | 内容 | 对应上一版文档章节 |
|------|------|------|
| `protocol.rs` | WS 线协议：二进制 PTY 帧 + JSON invoke/Channel（镜像 Tauri IPC） | §23 Room Protocol |
| `session.rs` / `rmux_pty.rs` | PTY 会话 + **rmux 集成**（`rmux-sdk` git 依赖，pin revision） | §18/§19 Session + rmux |
| `room/mod.rs`,`store.rs` | **Room = 追加式共享消息日志**；人和 CLI agent 都往里发 | §9/§22 Room |
| `room/peer.rs` | **peer/mesh 多主机**：Room 跨机器 | §22.1 Room Server |
| `room/client.rs`,`commands.rs`,`http.rs` | Room 客户端、命令、HTTP 面 | §22.2 Room Manager |
| `orchestrator.rs` | **可插拔 LLM 大脑**（OpenAI-兼容），驱动 dispatch/nudge | §24 Task Router / §52 |
| `agents.rs`,`agent_commands.rs` | Agent 注册、探测、生命周期命令 | §20/§21 Agent Registry |
| `auth.rs` | 密码 / HTTPS / **mesh CA**（host 证书可重签，无需巡检） | §53 安全 |
| `blob.rs`,`say.rs` | 二进制 blob 存取、语音/播报 | — |
| `config.rs` | YAML 配置（bind-addr / auth / cert / orchestrator） | §48 Server |
| `main.rs`,`ws.rs` | 启动、WebSocket 升级、连接循环 | §48 Server |
| `files.rs`,`fs_commands.rs`,`git_commands.rs`,`history_commands.rs`,`net_commands.rs`,`shell_commands.rs` | 各领域命令处理，桥接到 `phantom-core` | — |

### 3.1 命令契约（V2 的真正 API 面）

UI 与后端之间是 **93 个 invoke 命令**（分组见 附录 A）。egui 客户端**驱动的就是这同一批命令**，一个都不用改后端。分组：

```
pty_*        终端会话（open/attach/resize/close/kill/list/sessions…）
fs_*         文件系统（read_dir/read_file/write_file/glob/grep/search/watch…）
git_*        源码控制（status/diff/log/commit/stage/branch/push/pull…）
history_*    shell 历史
secrets_*    keychain
shell_bg_*   后台进程
agent_*      Agent（list/ensure/open/dispatch/nudge/enable_hooks/hooks_status…）
room_*       Room（history/post/roster）
orchestrate* 编排（orchestrate/orchestrate_stop）
ai_http_stream / lm_ping / get_launch_* 其它
```

### 3.2 线协议（V2 直接复用）

`protocol.rs` 的设计已经为"薄客户端"优化，egui 客户端天然受益：

- **二进制帧**走终端 I/O 热路径：`[u8 tag][u32 le pty_id][payload…]`，`TAG_PTY_DATA=0x01`。避免每次按键 JSON 编解码。
- **文本帧**走 JSON 控制消息：
  - `ClientMsg::Invoke { req, cmd, args }` —— 镜像 Tauri `invoke`，`req` 关联回复。
  - `ServerMsg::{ Result{req,ok,err}, PtyExit{id,code}, AgentSignal{id,kind,agent}, Stream{req,event} }`。
- `AgentSignal` 来自 OSC 777 标记，直接映射 Agent 状态变化。

## 4. 桌面 app 现状（重要约束）

`src-tauri`（crate `terax` v0.8.6）在 **main 上当前不编译**（phantom-core 抽离的桌面胶水未收尾：`mod fs` 无文件、`lib.rs` 引用已迁移的 `fs::tree/...`、私有 State 导入、缺 `emit_conout_marker`）。

**可运行路径是 `phantom-server` + 浏览器前端**（HTTPS + 密码认证，绑定 Tailscale IP，mesh CA 签发自签证书）。

> **对 V2 的含义：** 桌面 Tauri 壳已是负资产，不值得修。V2 用 egui 原生壳替代它，正好绕开这块坏死的胶水，而后端（phantom-server）本就是干净可跑的——这进一步支持"换 UI、留后端"的路线。

## 5. 前端现状（将被 egui 替换的部分）

`src/` ~**70.6k 行 TS/TSX**，23 个模块。按体量（重写工作量的参考）：

| 模块 | 行数 | egui 重写优先级 |
|------|-----:|------|
| ai | 12,642 | P3（Agent/编排面板，逻辑多在后端，UI 可简化） |
| terminal | 7,757 | P1（核心，需自绘终端网格渲染器） |
| explorer | 6,405 | P2 |
| editor | 5,500 | P2（降级为 Review/Quick-fix 编辑器，见 §12） |
| source-control | 3,091 | P2 |
| tabs | 2,920 | P1（布局） |
| theme | 2,684 | P1（egui 主题） |
| lsp | 2,043 | P4（后置，见风险） |
| spaces | 1,840 | P2 |
| git-history | 1,747 | P2 |
| agents | 1,638 | P3 |
| command-palette | 1,393 | P2 |
| settings | 1,110 | P2 |
| room | 867 | P3 |
| shortcuts | 724 | P1 |
| statusbar | 612 | P1 |
| preview | 590 | P4（图片/PDF/dev-server，egui 内有限支持） |
| orchestration | 560 | P3 |
| header/updater/sidebar/markdown/workspace | 合计 ~1.3k | P1–P4 |

**接缝：** `src/lib/transport/`（`client.ts`,`protocol.ts`,`core.ts`,`event.ts`…）是 UI↔后端的 JS shim，镜像 Tauri invoke+Channel。**egui 重写替换的正是"React 组件 + 这个 JS shim"**，换成"egui 面板 + Rust transport 客户端"。

---

# Part 2 — V2 目标架构

## 6. 核心判断：换 UI，留后端

```
                     保留（复用）                        替换（重写）
        ┌─────────────────────────────────┐   ┌───────────────────────────┐
        │ phantom-core   领域层            │   │ React 19 前端 (~70.6k 行) │
        │ phantom-server Room/rmux/编排     │   │ src/lib/transport JS shim │
        │ 93-command invoke 契约           │   │ src-tauri (terax) 桌面壳   │
        │ WS 协议 (二进制PTY + JSON)        │   └───────────────────────────┘
        │ rmux-sdk / mesh CA / auth        │              │
        └─────────────────────────────────┘              ▼
                       ▲                          ┌───────────────────────────┐
                       └── 同一契约驱动 ──────────│ egui UI + Rust transport   │
                                                  │ 原生壳 (eframe) + WASM     │
                                                  └───────────────────────────┘
```

**原则：**
1. **后端零改动优先。** egui 客户端说的是同一套 93 命令 + 同一 WS 协议。
2. **两种能力后端**（本已存在的分层）：
   - *Native*：桌面进程内直接调用 `phantom-server` 的命令处理器（或 in-process 嵌入），走 `phantom-core`。
   - *Remote*：WASM/远程客户端通过 WebSocket 连 `phantom-server`。
3. **过渡期双前端并存**：React 前端仍可跑（经 phantom-server），egui 客户端逐面板上线，不强制一次切换。

## 7. Crate 布局（V2）

```
phantom/
├── crates/
│   ├── phantom-core          【复用】领域层
│   ├── phantom-server        【复用】headless 网关（Native 也可 in-process 嵌入）
│   ├── phantom-protocol      【抽取】把 server/protocol.rs 的类型独立成 crate，
│   │                                 供 server 与 egui 客户端共享（消除 JS shim）
│   ├── phantom-client        【新】Rust transport 客户端：Native(直连) / Remote(WS)
│   │                                 两种 backend，实现同一 trait
│   ├── phantom-terminal      【新】终端网格模型 + ANSI 解析 + egui 渲染器
│   ├── phantom-ui            【新】egui 面板 / 布局 / 主题 / widgets（无平台依赖）
│   └── phantom-editor        【新】轻量 Review/Diff 编辑器（见 §12）
├── apps/
│   ├── phantom-desktop       【新】eframe 原生壳（Native backend）
│   └── phantom-web           【新】eframe WASM 壳（Remote backend，WS 连 server）
└── src-tauri/                【退役】保留到 egui 桌面达到功能对等后删除
```

依赖方向（上依赖下，领域不依赖 UI）：

```
phantom-desktop / phantom-web
        │
        ▼
     phantom-ui ──► phantom-terminal, phantom-editor
        │
        ▼
   phantom-client ──► phantom-protocol
        │                    ▲
        │  (Native 直连)      │ (Remote WS)
        ▼                    │
  phantom-server ────────────┘
        │
        ▼
   phantom-core
```

## 8. 能力后端（Capability Backend）

上一版 §3.3 的 Native/Remote 抽象**在当前代码里已经是现实**：`phantom-core`（本地能力）与 `phantom-server`（远程 WS）就是两个后端。V2 把它形式化为 `phantom-client` 的一个 trait：

```rust
// phantom-client：UI 只依赖这个 trait，不关心 Native / Remote
#[async_trait]
pub trait Backend {
    /// 对应 ClientMsg::Invoke —— 驱动 93 命令中的任意一个
    async fn invoke(&self, cmd: &str, args: Value) -> Result<Value>;
    /// 订阅 ServerMsg::Stream / PtyExit / AgentSignal 与二进制 PTY 帧
    fn subscribe(&self) -> EventStream;
    /// 二进制热路径：向某 pty 写入
    fn pty_write(&self, id: u32, bytes: &[u8]);
}

pub struct NativeBackend { /* in-process：直接调用 phantom-server 的 handler */ }
pub struct RemoteBackend { /* WebSocket：连 phantom-server */ }
```

- **Native**（`phantom-desktop`）：进程内嵌 `phantom-server` 的命令分发，省掉本地回环 WS；PTY 二进制帧变成进程内 channel。
- **Remote**（`phantom-web` / 跨机）：`RemoteBackend` 通过 WS 说 `phantom-protocol`，与现有 React 前端走的是**同一个服务端**。

---

# Part 3 — 领域模型 ↔ 现有代码映射

上一版把 Room/Agent/Session/Task/Message/Event 描述为待建模型。实际上大多已有具体实现。V2 沿用现有实现，只补齐缺口（Task、Event Store）。

| V2 概念 | 现有实现 | 状态 |
|---------|----------|------|
| **Project / Workspace** | `phantom-core::workspace`、config 的工作区根 | ✅ 有 |
| **Session** | `phantom-server::session.rs` + `rmux_pty.rs`（rmux-sdk） | ✅ 有，持久化已解决 |
| **Agent** | `phantom-core::agent`（探测/hook/OSC 777）、`server::agents.rs`、`agent_*` 命令 | ✅ 有 |
| **Room** | `server::room/`：追加式日志 + `Ref`（handoff/blob 引用）+ @mention→`RmuxState::nudge` + peer/mesh | ✅ 有，且比上一版设计更具体 |
| **Message** | Room 日志条目（携带引用而非内容："文件即协议"） | ✅ 有 |
| **Event** | WS `ServerMsg::Stream` / `PtyExit` / `AgentSignal`；`broadcast`(cap 256) | ⚠️ 有事件流，**无持久化/replay** |
| **Task** | `orchestrator.rs` 的 dispatch/nudge + `.phantom/handoff/*.md` | ⚠️ 无显式 Task 实体，靠编排+文件表达 |
| **Orchestrator brain** | `orchestrator.rs`：OpenAI-兼容 LLM，`.phantom/agents.yaml` | ✅ 有 |
| **Mesh / 多主机** | `room/peer.rs` + `auth.rs` mesh CA | ✅ 有 |

### Room 的实际设计（值得 V2 完整继承）

Room 不是抽象的"协作运行时"，而是一个**追加式共享消息日志**，设计要点（`room/mod.rs` 注释）：

- **消息携带引用，不携带内容**：`Ref` 命名一个 handoff 文件或 blob，接收方用自己的文件工具读。日志保持小，"文件即协议"。
- **@mention 注入 agent 会话**：`@claude` 解析到参与者，消息交给 `RmuxState::nudge`（等 agent hook 报 idle 再投递）。
- **不引入新线协议**：发帖是普通 `invoke`，扇出复用前端已监听的 `event` 帧。
- **跨主机**：`peer.rs` 让日志跨机器；`LOCAL_PEER` 命名本机参与者。

---

# Part 4 — 复用清单与差距

## 9. 复用清单（carry-over）

**原样复用（0 改动）：**
- `phantom-core` 全部模块。
- `phantom-server` 全部命令处理器、`session.rs`/`rmux_pty.rs`、`room/`、`orchestrator.rs`、`agents.rs`、`auth.rs`、`blob.rs`、`config.rs`。
- rmux-sdk 集成、mesh CA、HTTPS/密码认证。
- WS 协议语义与 93 命令契约。

**薄适配（小改）：**
- 把 `server/protocol.rs` 的**类型**抽成 `phantom-protocol` crate，让 egui 客户端与 server 共享同一份 Rust 类型（取代 `src/lib/transport` 的 JS 手写镜像）。
- `phantom-server` 增加一个 in-process 入口（Native backend 直调 handler，不走回环 WS）。

**全新（egui 侧）：**
- `phantom-client`（Native/Remote transport）。
- `phantom-terminal`（网格模型 + ANSI 解析 + egui 渲染）。
- `phantom-ui`（面板/布局/主题）。
- `phantom-editor`（Review/Diff）。
- `phantom-desktop`（eframe 原生）、`phantom-web`（WASM）。

## 10. 差距分析（新增工作）

| 差距 | 说明 | 归属 |
|------|------|------|
| egui 终端渲染器 | 替换 xterm.js/WebGL：ANSI 解析→网格→egui 自绘；光标/滚动/选区/复制粘贴/resize | `phantom-terminal`（重，P1） |
| egui 编辑器/Diff | 替换 Monaco：定位为 Review + Quick-fix（§12），不做全 IDE | `phantom-editor`（P2） |
| Rust transport 客户端 | 替换 `src/lib/transport` JS shim | `phantom-client`（P1） |
| **Event 持久化 / replay** | 现有事件流无持久化；V2 需要 SQLite event store 支持重连状态恢复（上一版 §25/§49） | `phantom-server` 扩展（P3） |
| **显式 Task 实体** | 现在靠 orchestrator+handoff 文件；V2 可选引入 Task 表（room_id/creator/assignee/status） | `phantom-core`/`server`（P3，可选） |
| WASM 构建与瘦身 | egui+wgpu/glow 的 WASM 目标、字体、二进制体积（保持轻量是 Terax 的价值观） | `phantom-web`（P4） |
| LSP | 前端有 lsp 模块（2k 行）；egui 侧后置（见风险 §14） | P4 |

---

# Part 5 — UI 重写（egui）

## 11. 主界面布局

沿用当前三栏 + 底部事件区（与 VS Code/Zed 类似），egui dock 实现：

```
┌───────────────────────────────────────────────────────────┐
│ 菜单 / Project / Room / Space                               │
├──────────┬──────────────────────────┬─────────────────────┤
│ Room &   │                          │ Agent /             │
│ Explorer │   Terminal / Editor      │ Orchestrator        │
│ (左栏)   │   (中区，tabs)           │ (右栏)              │
│          │                          │                     │
├──────────┴──────────────────────────┴─────────────────────┤
│ Events / Tasks / Messages（Room 日志 + 事件流）            │
└───────────────────────────────────────────────────────────┘
```

## 12. 面板 ↔ 现有模块 ↔ 命令

| egui 面板 | 取代的 React 模块 | 驱动命令 | 备注 |
|-----------|-------------------|----------|------|
| Terminal | terminal, tabs | `pty_*` + 二进制帧 | 核心，自绘渲染器 |
| Explorer | explorer | `fs_*` | 文件树/搜索/watch |
| Editor(Review/Diff) | editor | `fs_read_file/write_file`, `git_diff*` | **降级**：文件查看 + Diff + 修改确认 + AI Review，不做全 IDE |
| Source Control | source-control, git-history | `git_*` | 提交图/暂存/diff |
| Room | room | `room_*` | 追加式日志 UI |
| Agent | agents, ai | `agent_*` | 状态/attach/发消息/派任务 |
| Orchestrator | orchestration, ai | `orchestrate*`, `ai_http_stream` | 大脑控制 + 手动 dispatch/nudge |
| Command Palette | command-palette | 多 | egui 覆盖层 |
| Settings | settings | `secrets_*`, config | 密钥入 keychain |
| Status Bar | statusbar, header | — | |
| Preview | preview | — | 图片/PDF；dev-server 预览降级/后置 |

**编辑器定位（重要取舍）**：AI Coding 时代，代码编辑不是瓶颈。egui 编辑器**不追 Monaco**，只做 Review Editor + Quick-fix + Diff 确认 + AI Review。全功能编辑仍可在终端里用外部编辑器。

## 13. 终端渲染器（P1 核心）

```
PTY 输出(二进制帧) → ANSI 解析 → TerminalGrid(rows×cols, Vec<Cell>) → egui 自绘
```

- MVP：ANSI、光标、滚动、输入、resize。
- 后续：Unicode/emoji 宽字符、GPU 加速、搜索、复制粘贴、链接检测。
- 数据源：Native 走进程内 channel；Remote 走 WS 二进制帧（`decode_pty_data`）。

---

# Part 6 — 落地路线与风险

## 14. 风险与取舍

| 风险 | 说明 | 缓解 |
|------|------|------|
| egui 终端成熟度 | 不如 xterm.js（TUI 兼容、宽字符、性能） | 复用现成 crate（如 egui 终端组件）起步，再自绘；PTY 保真靠 `phantom-core::pty` 不变 |
| egui 编辑器 | 不如 Monaco | 明确降级为 Review/Diff，不做全 IDE |
| ~70k 行 UI 重写成本 | 大 | 后端零改动 + 双前端并存，按面板增量迁移，不一次性切换 |
| LSP 回退 | 前端 2k 行 lsp 功能在 egui 侧缺失 | 后置到 P4；架构预留 `LanguageService` trait；Review 导航优先 |
| 轻量价值观 | Terax 主打 7–8MB；egui+wgpu+WASM 可能变重 | 体积预算纳入 CI（`.size-limit` 思路）；WASM 懒加载字体/语言包 |
| 桌面胶水坏死 | src-tauri 不编译 | 不修，egui 原生壳直接替代 |

## 15. 分阶段落地路线

每阶段都**建立在现有可跑的 phantom-server 之上**，先出可用的 egui 客户端，再逐面板对齐。

**P0 — 后端定桩（1 周）**
- 把 `phantom-server` 确立为 V2 唯一后端标准；补 in-process 入口。
- 抽 `phantom-protocol` crate（类型共享）。
- 验收：`phantom-server` clean build + run（已满足），协议类型独立可被 Rust 客户端引用。

**P1 — egui 外壳 + 终端（3–5 周）**
- `phantom-client`（Native + Remote）、`phantom-terminal`、`phantom-ui` 骨架、`phantom-desktop`(eframe)。
- 面板：Terminal、Tabs、布局、主题、状态栏、快捷键、命令面板。
- 验收：egui 桌面壳能开 PTY、跑 shell、跑 TUI（vim/htop）、resize/滚动/复制粘贴。

**P2 — 文件 / 编辑 / Git（3–4 周）**
- 面板：Explorer、Editor(Review/Diff)、Source Control、Git History、Spaces。
- 复用 `fs_*` / `git_*` 命令。
- 验收：浏览/编辑/保存文件、看 diff、暂存提交、看提交图。

**P3 — Room / Agent / Orchestrator（4–6 周）**
- 面板：Room、Agent、Orchestrator。
- 复用 `room_*` / `agent_*` / `orchestrate*` / rmux / peer。
- 新增：SQLite event store + replay（重连状态恢复）；可选 Task 实体。
- 验收：多 Agent 在 Room 协作、@mention 注入、编排大脑派活、断线重连恢复状态、跨主机 mesh。

**P4 — WASM Web 客户端（4–6 周）**
- `phantom-web`(eframe WASM) 经 WS 连 phantom-server（与 React 前端同服务端）。
- 面板：Room/Agent 状态、发任务、Review Diff、看终端（远程 client 定位）。
- 验收：浏览器 egui 客户端连远程 workspace，功能达"Room Client"级。

**P5 — 收尾（2–3 周）**
- 删除 `src-tauri`（terax 壳）与 React 前端（达到功能对等后）。
- LSP（可选，rust-analyzer / typescript-language-server）。
- 体积/性能瘦身，跨平台打包（macOS/Linux/Windows/WSL）。

## 16. 最终形态与竞争力

```
                    Phantom V2
                         │
                 Multi-Agent Room
                         │
        ┌────────────────┼────────────────┐
     Developer        AI Agents         远程/多设备
     (egui 原生)     (rmux 持久)        (egui WASM)
        │                │                  │
     Review           Execute            Observe
        └──────────────► Ship Code ◄───────┘
```

| 能力 | 现状 | V2 增量 |
|------|------|---------|
| 纯 Rust 栈 | core/server 已是 | UI 也转 Rust（egui），单语言单工具链 |
| Desktop + Web | React+Tauri / phantom-server | egui 原生 + WASM，共享领域模型与协议 |
| rmux 持久会话 | ✅ | 不变，直接用 |
| Room 多 Agent 协作 | ✅（追加式日志/mesh） | 加 event replay、可选 Task |
| Agent Protocol | CLI 探测/hook/OSC 777 | 预留 ACP/MCP 适配 |
| 轻 Editor | Monaco（重） | egui Review/Diff（轻，贴合 AI Coding） |
| Human Review | AI diff | egui Diff 确认为一等公民 |

**一句话定位：** Phantom V2 = 把已经跑起来的 Room + rmux + 多 Agent 编排后端，配上一个纯 Rust/egui 的原生 + Web 前端；不是新造轮子，而是**给现有可跑后端换一层单语言、可 WASM 的皮**。类比：rmux≈容器运行时，Room≈集群/命名空间，Agent≈Worker，Orchestrator≈调度器，egui≈控制台 Dashboard。

---

# 附录 A — 93 命令契约（分组）

```
终端       pty_open pty_attach pty_resize pty_close pty_close_all pty_kill
           pty_list_shells pty_sessions pty_shell_name
           pty_has_foreground_job pty_has_foreground_process
文件       fs_read_dir fs_read_file fs_write_file fs_stat fs_list_files
           fs_create_file fs_create_dir fs_rename fs_copy fs_delete
           fs_glob fs_grep fs_grep_interactive fs_search fs_canonicalize
           fs_watch_add fs_watch_remove  list_subdirs
Git        git_status git_diff git_diff_content git_log git_show_commit
           git_stage git_unstage git_discard git_commit git_commit_files
           git_commit_file_diff git_list_branches git_checkout_branch
           git_fetch git_pull_ff_only git_push git_remote_url
           git_resolve_repo git_panel_snapshot
历史       history_list history_record history_suggest history_commands
密钥       secrets_get secrets_get_all secrets_set secrets_delete
后台进程   shell_bg_spawn shell_bg_list shell_bg_logs shell_bg_kill
Agent      agent_list agent_ensure agent_open agent_dispatch agent_nudge
           agent_enable_hooks agent_hooks_status list_agents
           dispatch_agent nudge_agent read_handoff
Room       room_history room_post room_roster
编排       orchestrate orchestrate_stop
其它       ai_http_stream lm_ping get_launch_dir get_launch_files
```

# 附录 B — 关键代码位置速查

```
领域层        crates/phantom-core/src/{agent,pty,fs,git,history,proc,net,secrets,shell,workspace,path}
网关          crates/phantom-server/src/{main,ws,session,rmux_pty,protocol,orchestrator,agents,auth,blob,config}.rs
Room/mesh     crates/phantom-server/src/room/{mod,client,commands,http,peer,store}.rs
命令桥接      crates/phantom-server/src/{fs_commands,git_commands,history_commands,net_commands,shell_commands,files}.rs
rmux 依赖     rmux-sdk (git, pinned rev) — crates/phantom-server/Cargo.toml
现前端(退役)  src/modules/*（~70.6k 行）、src/lib/transport/*（JS shim）
坏死桌面壳    src-tauri（crate "terax" v0.8.6，main 不编译）
运行配置      ~/.config/phantom/config.yaml（bind-addr/auth/cert/orchestrator）；PHANTOM_CONFIG
```
