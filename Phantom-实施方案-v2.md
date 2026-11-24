# Phantom v2：AR/移动优先的无头 AI 协作工作站实施方案

**Phantom (幻影)** 是一款面向 **AR 眼镜 + 手机** 移动开发场景的极简 AI 开发环境。计算与上下文全部由运行在 CentOS 8 上的守护进程承载，客户端只是一块可随时丢弃的显示层。

> 本文取代 `Phantom：下一代无头 AI 协作工作站实施方案.md`（v1，保留作对比）。
> **本版基于对 `github.com/crynta/terax-ai` 的实际代码勘察修订**，勘察结论见 §2，v1 差异见附录 A。

---

## 一、场景约束（设计的第一性依据）

Phantom 不是"桌面 Web IDE 缩小版"。以下是硬约束，所有设计决策都必须回溯到这里：

AR 眼镜按**外接显示器**对待（见 §8），因此显示侧无特殊约束。真正的硬约束集中在输入与网络：

| 约束 | 实测量级 | 设计含义 |
| --- | --- | --- |
| **手机软键盘缺键** | 无 Ctrl/Esc/Tab/方向键/Alt | 必须自建修饰键条 + 命令面板兜底，见 §7。**这是移动可用性的成败点** |
| **移动网络 RTT** | mesh over 4G/5G，30–150ms，切换时秒级中断 | 连接假设为**每天断几十次**，需增量 resync，见 §6 |
| **手机浏览器后台** | iOS Safari 切后台 ~30s 杀 WebSocket 与 timer | 不做保活（做不到），做主动 detach + 快速 resync |
| **蜂窝流量与首屏** | 冷启动可能在移动网络 | 前端产物需体积预算，见 §5.4 |
| **移动端浏览器差异** | clipboard image 权限不稳；`keydown.keyCode` 常为 229 | 图片走降级链（§9）；输入用 `beforeinput`（§5.2） |

> AR 眼镜自身不引入新约束——它是一块虚拟大屏，其上跑的是普通桌面浏览器。手机才是约束来源。

---

## 二、代码勘察结论（决定架构的事实基础）

对 `crynta/terax-ai` 实测：

```
64,511 LOC  TypeScript/React   src/          ← 前端，已是 Web 技术栈
13,743 LOC  Rust               src-tauri/    ← 后端模块，纯逻辑
```

- **前端选型**：CodeMirror 6（非 Monaco）+ xterm.js v6 + React。已含 `@xterm/addon-serialize`、`addon-webgl`、`addon-fit`、`addon-search`
- **已有模块**：`editor` `terminal` `lsp` `git-history` `source-control` `command-palette` `shortcuts` `agents` `ai` `explorer` `spaces` `settings` `theme`
- **Rust 模块**：`pty` `fs` `git` `lsp` `shell` `proc` `secrets` `workspace` `history` `net` `agent`
- **IPC 面**：`invoke` 仅散布于 **14 个文件**，`@tauri-apps/api/event` 仅 8 个文件

### 由此推出的两个架构定论

**定论一：不使用 WASM。**

原计划用 Rust→WASM 换取"编辑器本地零延迟"。但 Terax 前端**已经是浏览器内的 React + CodeMirror**——编辑器状态本就在本地，按键本就不 round-trip。WASM 要达成的目标已免费达成。走 WASM 需将 64.5k LOC TypeScript 重写为 Rust，投入极大而延迟收益为零。**否决。**

**定论二：Phantom ≈ Terax 的 Tauri IPC 层替换为 WebSocket。**

工作量远小于预期。以 [pty-bridge.ts](src/modules/terminal/lib/pty-bridge.ts) 为例，整个 PTY 层接缝只是：

```ts
openPty(cols, rows, handlers, cwd, blocks, shell) -> { id, write, resize, close }
```

`invoke` + `Channel` 换成 WebSocket 即通。Rust 侧同理：`src-tauri` 模块本就是纯逻辑，剥掉 `#[tauri::command]` 装到 Axum 上即可。

**Phantom 的真正原创工作，集中在 §7 AR/移动输入层与 §8 显示规范。** 其余多为适配。

---

## 三、架构总览

```
┌──────────── 客户端（手机浏览器 / AR 眼镜 WebView，PWA） ─────────────┐
│  Terax 前端（React + CodeMirror 6 + xterm.js）  ← 原样复用          │
│    + Phantom 移动层（新增）                                         │
│        · 修饰键条  · 手势层  · 语音输入  · AR 主题                   │
│    + transport 适配层（新增，替换 @tauri-apps/api）                  │
└──────────────────────────┬──────────────────────────────────────────┘
                           │ WebSocket 单连接多路复用（postcard 二进制帧）
┌──────────────────────────┴──────────────────────────────────────────┐
│  phantom-server（Axum + Tokio）                                     │
│    · 静态资产托管（Brotli 预压缩）    · 设备鉴权 / 会话              │
│    · Terax src-tauri 模块去 Tauri 化复用：                          │
│        fs / git / lsp / proc / workspace / history / secrets        │
│    · AI Provider 代理（Key 只存服务端）  · MCP Server               │
│    · Blob 落盘 + 缩略图                                             │
│    · agent 状态聚合：订阅 pane.state_events() → AgentEvent → Push   │
│    └── rmux 客户端（rmux-sdk，本地 IPC）                            │
├─────────────────────────────────────────────────────────────────────┤
│  rmux-server（独立守护进程，同机）  ← 接管全部 PTY                   │
│    · pane 生命周期  · Attach/Detach  · 终端仿真（含 DA/DSR 代答）    │
│    · pane options (@agent.state)  · state_events 流                 │
│    └── panes: claude code / opencode / shell / build ...             │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 四、Terax 复用策略

### 4.1 前端：新建 transport 适配层

不改业务模块，只替换它们依赖的传输原语。新建 `src/lib/transport/`：

```ts
// 与 @tauri-apps/api/core 保持同签名，业务代码零改动
export async function invoke<T>(cmd: string, args?: unknown): Promise<T>;
export class Channel<T> { onmessage: (m: T) => void }   // 映射到 ws 流
export function listen<T>(event: string, cb: (e: {payload: T}) => void): Promise<UnlistenFn>;
```

改造方式：Vite `resolve.alias` 把 `@tauri-apps/api/core` 与 `/event` 指向本地实现。**14 个 invoke 文件与 8 个 event 文件无需逐个改动**，仅需处理语义不兼容的少数点（见 4.3）。

```ts
// vite.config.ts
resolve: {
  alias: {
    "@tauri-apps/api/core":  "/src/lib/transport/core.ts",
    "@tauri-apps/api/event": "/src/lib/transport/event.ts",
  }
}
```

### 4.2 Tauri Plugin 映射表

11 个 plugin 在浏览器无对应，逐个定案：

| Plugin | 处置 | 说明 |
| --- | --- | --- |
| `clipboard-manager` | **替换** | `navigator.clipboard` + §9 降级链 |
| `notification` | **替换，且升级为核心能力** | Web Notification + Push API。配合 §6.3 agent 检测，实现「AI 跑完推送到手机」——AR 场景最有价值的功能之一 |
| `store` | **替换** | 服务端 KV（持久化设置）+ IndexedDB（设备本地缓存） |
| `os` | **替换** | 服务端返回主机信息；客户端信息用 `navigator` |
| `opener` | **替换** | `window.open` / `<a target="_blank">` |
| `log` | **替换** | 经 ws 上报服务端落盘 |
| `updater` | **移除** | 服务端部署即更新，前端只需版本不匹配时提示刷新 |
| `window-state` | **移除** | 改为 localStorage 存布局 |
| `autostart` | **移除** | 由 systemd 负责 |
| `process` | **移除** | 重启由 systemd 管；前端 `location.reload()` |
| `plugin-log` (Rust 侧) | **替换** | `tracing` + 文件 appender |

### 4.3 已知语义缺口（需逐个处理）

- **`Channel` 背压**：Tauri Channel 为进程内调用，无背压问题；ws 上需加流控，否则终端高速输出会撑爆客户端。方案：服务端按 §6.2 的 Ack 节流
- **多窗口**：`openSettingsWindow.ts` 依赖 Tauri 多窗口 → 改为前端路由 / 模态
- **文件拖放**：`useExplorerFileDrop.ts` 用 Tauri 原生 drop 事件 → 改用 HTML5 DnD + §9 上传链
- **`keyring.ts`**：AI 凭据原走系统钥匙串 → 改为服务端 `secrets` 模块 + 文件权限 0600，浏览器永不接触（§11）
- **`proxyFetch.ts`**：原经 Rust 绕过 CORS → 保留，改为 Axum 代理端点

### 4.4 后端：去 Tauri 化

`src-tauri/src/modules/` 下多数模块是纯逻辑，改造机械：

```rust
// 之前
#[tauri::command]
pub async fn read_file(path: String) -> Result<String, Error> { ... }

// 之后：逻辑函数不变，改由 ws 路由分发
pub async fn read_file(path: &str) -> Result<String, Error> { ... }
```

`fs` `git` `lsp` `history` `workspace` `proc` 可近乎直接复用。`pty` 模块特殊处理，见 §6.3。

---

## 五、前端改造与性能预算

### 5.1 渲染栈风险：xterm WebGL

Terax 已用 `@xterm/addon-webgl`。该 addon 在部分 AR 眼镜的 Android WebView 上会崩溃或渲染错乱。必须实现启动探测 + 降级链：

```
WebGL addon → Canvas renderer → DOM renderer
```

并建立目标设备/浏览器矩阵，实机逐项验证。

### 5.2 CodeMirror 6 移动端

选型正确（CM6 移动支持显著优于 Monaco，体积也小得多）。仍需处理：

- 移动端 `keydown` 的 `keyCode` 常为 229，不可依赖；用 `beforeinput` 捕获
- IME 组合期间（`compositionstart`→`compositionend`）不可干预编辑状态
- CM6 的默认 keymap 在无物理键盘时大量失效 → 由 §7.3 命令面板兜底

### 5.3 PWA 化

- Manifest + Service Worker（仅缓存静态资产，**不缓存 API**）
- `display: standalone`，隐藏浏览器 UI（AR 上每一行都珍贵）
- 注册 `share_target`，支撑 §9 的截图分享主路径

### 5.4 体积与性能预算（CI 卡门禁）

Terax 已有 `.size-limit.json`，沿用并按移动场景收紧：

| 指标 | 预算 |
| --- | --- |
| 首屏 JS (Brotli) | < 300 KB |
| 全量 JS (Brotli) | < 1.5 MB，语言包与非首屏模块懒加载 |
| 首次可交互 @5Mbps | < 3 s |
| 按键 → 屏幕更新 (P95) | < 16 ms |
| 后台返回 → 可交互 | < 800 ms |

---

## 六、会话层：rmux 接管与 AI 可观测性

### 6.1 决策：全部 PTY 走 rmux

Terax 自带 PTY（`portable_pty`），但进程内托管意味着 **phantom-server 重启 = 所有 AI 任务死亡**。移动场景下服务端升级、崩溃恢复是常态，因此全部 pane 交由独立守护的 `rmux-server`。

代价：`src-tauri/src/modules/pty/`（2591 LOC）需重构，不能简单删除——见 6.3。

### 6.2 断线重连协议

服务端每 pane 维持单调递增 `seq`（粒度 = 一次 PTY read chunk）与输出 ring buffer。

> Terax 已有 `src-tauri/src/modules/shell/ringbuffer.rs` 的 `BoundedRingBuffer`：byte-oriented、monotonic offset、带 `dropped` 计数、`read_from(since) -> (Vec<u8>, u64, u64)`。**语义与本设计完全吻合，直接复用**，只需从 `shell` 提升为通用组件并接到 rmux 输出流。

```rust
enum ClientMsg {
    Attach { pane: PaneId, last_seq: Option<u64> },
    Detach { pane: PaneId },
    Input  { pane: PaneId, data: Bytes },
    Resize { pane: PaneId, cols: u16, rows: u16 },
    Ack    { pane: PaneId, seq: u64 },
    FileRead  { path: String },
    FileWrite { path: String, base_version: u64, patch: Vec<TextPatch> },
    Blob   { id: Uuid, mime: String, chunk: u32, total: u32, data: Bytes },
    Ai     { req: Uuid, kind: AiKind, ctx: AiContext },
}

enum ServerMsg {
    Output  { pane: PaneId, seq: u64, data: Bytes },
    Resync  { pane: PaneId, mode: ResyncMode },
    AgentEvent { pane: PaneId, agent: String, kind: AgentEventKind },  // ← §6.3
    PaneExit{ pane: PaneId, status: i32 },
    FileChanged { path: String, version: u64 },
    Diff    { path: String, hunks: Vec<Hunk> },
    BlobReady { id: Uuid, abs_path: String, thumb: Option<Bytes> },
    AiDelta { req: Uuid, token: String },
    Error   { code: ErrCode, msg: String },
}

enum ResyncMode {
    Incremental,                                // 后续 Output 帧即补发
    Snapshot { screen: Bytes, base_seq: u64 },  // 客户端 reset 后应用
}
```

重连算法：

```
Attach { last_seq: Some(n) }
  ringbuffer.read_from(n) 若 dropped == 0  → Incremental，重放 (n, seq_end]
  否则                                      → Snapshot，客户端 xterm.reset() 后写入
```

快照来源：服务端侧 vt100 解析器 dump，或复用前端已有的 `@xterm/addon-serialize`（客户端主动缓存）。优先服务端，保证多设备一致。

客户端 `Ack` 后可裁剪 buffer，但保底保留 4 MB 支持多设备各自落后。

### 6.3 AI 可观测性层的迁移（已完成 rmux 侧勘察）

`pty` 模块不是简单封装，含三块能力。基于对 rmux 0.9.0 的实测，逐项定案：

| 文件 | LOC | 作用 | 定案 |
| --- | --- | --- | --- |
| `shell_init.rs` | 1085 | 向 bash/zsh/fish 注入 init 脚本，使其 emit OSC 标记 | ✅ **可迁移**。`rmux-sdk` 的 `EnsureSession` 提供 `.environment()` `.shell()` `.command()` `.working_directory()`（底层 `NewSessionRequest.environment: Option<Vec<String>>`，`NAME=VALUE` 形式）。原 `portable_pty::CommandBuilder` 的参数可完整映射 |
| `agent_detect.rs` | 452 | 解析 OSC 777 `notify;Terax;<agent>;<event>`，识别 claude/codex/gemini/pi 的运行/完成/等待状态 | ⚙️ **改造，不下移为字节流过滤器**。见下方「更优通道」 |
| `da_filter.rs` | 340 | 代答 DA1/DA2/DSR 查询 | ❌ **必须删除**。rmux 已自带完整终端仿真并代答：`rmux-core/src/input/dispatch.rs:289` 即 `parser.reply("\x1b[?1;2c")`，与 Terax 的 `DA1_REPLY` 同一串字节。保留会导致双重响应、终端错乱 |

#### 更优通道：pane options + state_events，而非字节流过滤

rmux SDK 提供结构化的 pane 状态流，官方示例直接演示了 agent 用法：

```rust
let mut stream = pane.state_events(PaneStateEventsOptions {
    include_foreground: true, ..Default::default()
}).await?;

pane.set_option("@agent.state", "running").await?;   // @ 前缀 = 用户自定义选项
```

因此 agent 状态不必在 rmux→ws 的字节流里重新解析 OSC，改走结构化链路：

```
shell hook (来自 shell_init 注入)
  → rmux set-option -p @agent.state <state>
    → phantom-server 订阅 pane.state_events()
      → ServerMsg::AgentEvent
        → Web Push → 手机
```

比字节流过滤干净得多，且是 rmux 的设计意图。`agent_detect.rs` 的 OSC 解析状态机可大幅简化，但其**语义分类逻辑（哪个 agent、什么状态）应保留**——原因见下。

#### rmux 的 `foreground` 不能替代 agent_detect

`sdk.pane.foreground` capability 会报告前台进程组与可执行路径，但文档明确写着：

> The executable path is best-effort … **RMUX does not try to classify agent names.**

即 rmux 给的是**进程级事实**（"pane 里有个 claude 进程"），Terax 给的是**语义事件**（"claude 停下来等你确认权限"）。进程存在 ≠ 知道它在等你。AR 场景需要的恰恰是后者。

定位：`foreground` 作为廉价的粗粒度兜底信号（shell hook 未生效时至少知道进程存活），`@agent.state` 作为主信号。

**为什么这层对 AR 场景格外重要**：`AgentEvent` 是「AI 停下来等你」的信号源。AR 眼镜上你不会盯着终端等——你会把眼镜摘了。这条链路应作为 Phantom 的一等功能，优先级高于多数 IDE 特性。

### 6.4 rmux Web Share 的定位：参考，不复用

rmux 0.9.0 自带 `rmux web-share`——浏览器访问 pane，混合后量子 E2EE（X25519 + ML-KEM-768 / ChaCha20-Poly1305），前端为解耦的静态 HTML/JS/WASM，可自托管。乍看与 Phantom 重叠，但**不能直接充当 Phantom**：

1. **范围不同**：Web Share 只分享终端 pane，不含编辑器、git、LSP、文件树。Phantom 要的是完整 IDE
2. **架构冲突**：Web Share 的 E2EE 是浏览器 ↔ daemon 端到端，中继只见密文。而 Phantom 的 `phantom-server` 是**有状态中间层**——它必须看到明文才能做文件服务、LSP、AI 代理与 §6.3 的状态聚合。强行复用 E2EE 通道等于让中间层无法工作

**Phantom 的选择**：`phantom-server` 与 `rmux-server` **同机部署**，之间走本地 IPC（明文无风险）；浏览器 ↔ `phantom-server` 之间由 Phantom 自己负责 TLS + 设备 token（§11）。

仍应参考/复用的部分：

- `rmux-web-crypto` crate 的密钥协商实现，可用于强化 §11 的设备配对
- Web Share 前端的 xterm 集成与重连处理，作为 §5.1 降级链的实现参考
- 它证明了「daemon 不 serve web 资产、前端完全解耦」的部署形态可行

### 6.5 移动端后台策略

不做保活，做主动让位：

```ts
document.addEventListener("visibilitychange", () => {
  if (document.hidden) { send(Detach); ws.close(1000); }
  else { reconnect().then(() => send(Attach({ last_seq }))); }
});
```

pane 在服务端继续运行，输出进 ring buffer。回前台走 §6.2 resync。

---

## 七、AR + 移动输入层（本项目核心原创部分）

### 7.1 常驻修饰键条

底部固定，高 56px（AR 下需超过 48px 最小目标）：

```
[Esc] [Ctrl] [Alt] [Tab] [←][↓][↑][→] [⌘K] [🎤]
```

- **粘滞语义**：tap = 对下一键生效一次；double-tap = 锁定（高亮）；再 tap = 解除
- 长按 `Esc` = 发送 `Ctrl+C`（终端最高频组合）
- 键条状态需与 Terax `shortcuts` 模块的 keymap 状态机对接，走同一分发路径

### 7.2 手势：不新增，只保证不破坏系统手势

macOS 与手机系统已自带滚动、缩放、返回、切换应用等手势，**不重复实现**。

此处真正的工作量是**反向的**：xterm.js 与 CodeMirror 默认会对 touch 事件 `preventDefault`，反而破坏系统滚动与捏合缩放。约束清单：

- 终端与编辑器容器**不得**无条件 `preventDefault` touch 事件；仅在确需接管时（如选择模式）局部拦截
- 捏合缩放交给系统 / 已有 `useZoom.ts`，不自行实现
- 惯性滚动交给浏览器原生，不用 JS 模拟
- 检查 `touch-action` CSS：默认不设 `none`，按需用 `pan-y` 等精确值

若后续确需应用级手势（切 pane / 切 tab），一律用**双指**滑动——单指上下滑与文本选择、单指左右滑与系统返回手势都会冲突，误触率高。但优先级低：这些操作命令面板（§7.3）已完全覆盖，不必为此引入手势冲突。

### 7.3 命令面板是主入口，不是备选

Terax 已有 `modules/command-palette` 与 `modules/shortcuts`——**大概率已满足 keymap/action 分离**，需在 P0 核实。要求：

- 覆盖 100% 操作，移动端用户不应被要求记忆任何快捷键
- fuzzy 匹配、最近使用排序
- 分类前缀：`>` 命令、`@` 符号、`:` 行号、`#` 文件

若发现存在「仅快捷键可触发、无具名 action」的操作，须补齐 action 注册。

### 7.4 语音输入：交给系统输入法

**不自建语音链路。** 手机与 macOS 系统输入法自带语音按钮，只要 AI prompt 输入框是标准 `<textarea>`、且不干扰 IME（§5.2 已有要求），语音输入即可用，零开发成本。

不引入 Web Speech API，不引入服务端 whisper——那是需求蔓延。

唯一相关要求：prompt 输入框需易于聚焦（修饰键条上给一个直达按钮），因为语音输入前要先让输入法拿到焦点。

---

## 八、显示：把 AR 眼镜当作外接显示器

**不做"AR 模式"，不做专用主题，不做设计 token 规范。** AR 眼镜（Xreal/Rokid 等）的主流用法就是当一块虚拟大屏，其上运行的就是普通桌面浏览器。为它单独设计一套显示规范是过度工程。

Terax 已有 `modules/theme`（多套 CodeMirror 主题）与 `useZoom.ts`（字号缩放），用户自行调节即可，无需新增机制。

仅保留两条默认值建议，均为零开发成本：

- **默认暗色主题**。AR 光学为加色合成，黑色像素不发光；白底会全功率发光，刺眼且增加设备发热。开发者本就多用暗色，Terax 已内置多套，选一个作默认即可
- **手机窄屏下侧栏/状态栏默认折叠**。这是常规响应式处理，与 AR 无关

真正影响移动可用性的是输入（§7），不是显示。

---

## 九、多媒体上下文注入

按可靠性排序的降级链（移动端 clipboard image 权限极不稳定，不可作主路径）：

1. **PWA `share_target`** —— 手机截图后直接「分享到 Phantom」。**移动端主路径**
2. 文件选择器（`accept="image/*" multiple`，含直接调摄像头）
3. HTML5 拖拽（替换原 Tauri 原生 drop）
4. `paste` 事件（桌面主路径，移动端仅补充）

后端流程：

- 分块上传（`Blob` 帧 chunk/total），支持断点续传
- 落盘 `/tmp/phantom_workspace/<session>/`，返回绝对路径
- 缩略图随 `BlobReady` 回传（免二次请求）
- 经 rmux 向激活 pane 注入空格分隔路径文本
- 前端右下角半透明气泡，5s 自动消散
- **GC**：session 结束或 24h TTL + 配额上限，防 `/tmp` 撑满

---

## 十、AI 与 MCP

- Axum 代理全部 AI Provider 请求，API Key 仅存服务端（复用 `secrets` 模块，替换原 `keyring`）
- 流式响应经 `AiDelta` 下发，前端复用已有 `ai-elements` 组件渲染
- Axum 实现标准 **MCP Server**，向 pane 内 AI CLI 暴露：
  - `workspace_diagnostics`（复用 `lsp` 模块聚合）
  - `git_status` / `git_diff`（复用 `git` 模块）
  - `symbol_search`（复用 `fs/search`、`fs/grep`）
  - `read_blob`（读取 §9 落盘图片）
- 使「浏览器内 AI」与「终端内 AI」共享同一上下文视图

---

## 十一、安全模型

> Tauri 应用运行在本地，无鉴权概念。改为 Web 服务后**这是全新且必须的一层**。
> 仅绑内网 IP 不是鉴权——Headscale 网络内任一被攻陷设备都能直连拿到 PTY，等同远程 shell。**优先级等同 P1，不可后置。**

- **设备配对**：服务端 CLI 生成一次性配对码（5 分钟有效）→ 浏览器换取长期设备 token → 存 IndexedDB
- **连接鉴权**：WebSocket 握手必须携带 token；失败立即关闭，不回显原因
- **Token 作用域**：绑定 workspace 与允许的 pane 集合；支持吊销列表
- **凭据隔离**：AI API Key 经 `secrets` 模块存服务端，文件权限 0600，浏览器永不接触
- **传输**：Headscale 内网 + TLS（内网 CA 或 Tailscale 证书）；可选 mTLS
- **监听**：**绝不监听 `0.0.0.0`**，仅绑 Headscale 虚拟网卡 IP；`firewalld` 只放行该网段
- **CSP**：`default-src 'self'; script-src 'self'; connect-src 'self' wss://…`
- **审计日志**：设备接入、pane 创建、文件写入、AI 调用全部落盘
- **速率限制**：配对码尝试、AI 代理调用

---

## 十二、构建与部署

### 12.1 ⚠️ v1 的 MUSL 方案是错的

v1 称「CentOS 8 的 glibc 2.28 导致标准 Rust 动态编译产物无法运行」——**不成立**。Rust `x86_64-unknown-linux-gnu` 最低要求 glibc 2.17，CentOS 8 的 2.28 完全够用。

真正的问题是**构建机 glibc 比 2.28 新**，链接进了目标机没有的符号版本。

且 musl 静态对本项目**有害**：PTY 需 `forkpty`；musl 静态下 NSS 不可用，`getpwnam` 查不到用户的 shell 与 home，DNS 同样退化——会直接破坏会话启动与 §6.3 的 shell 初始化。

### 12.2 正确做法

```bash
# 方案 A（推荐）：cargo-zigbuild 锁定 glibc 版本
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu
cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.28

# 方案 B：直接在 CentOS 8 容器内构建
podman run --rm -v "$PWD":/src -w /src quay.io/centos/centos:stream8 \
  bash -c 'dnf -y install gcc && cargo build --release'
```

前端：

```bash
pnpm build                      # 复用 Terax 既有 Vite 配置
brotli -q 11 dist/assets/*.{js,css}   # Axum 托管预压缩产物
```

### 12.3 Systemd

```ini
[Unit]
Description=Phantom Headless AI Workstation
After=network-online.target rmux.service
Wants=network-online.target
Requires=rmux.service

[Service]
Type=simple
User=phantom
WorkingDirectory=/opt/phantom/workspace
Environment="PHANTOM_HOST=192.0.2.10"
Environment="PHANTOM_PORT=8080"
EnvironmentFile=/etc/phantom/secrets.env   # AI API Key，权限 0600
ExecStart=/opt/phantom/phantom-server
Restart=on-failure
RestartSec=5

# 加固
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=false                # 需与 rmux 共享 /tmp/phantom_workspace
ReadWritePaths=/opt/phantom/workspace /tmp/phantom_workspace
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

> `After=network-online.target`（非 `network.target`）：绑定具体 Headscale IP 时网卡未就绪会启动失败。
> `rmux.service` 需独立配置，且**不应**随 phantom-server 一起重启——这正是分离进程的意义。

### 12.4 终端环境依赖

```bash
sudo dnf install ncurses ncurses-compat-libs
```

---

## 十三、演进路线与验收标准

鉴权与输入层置于 P1——否则后续阶段无法真机试用，全程盲开发。

| Phase | 目标 | 验收标准（可测） |
| --- | --- | --- |
| **P0 可行性验证** ✅ **已完成** | ① rmux SDK 传 env/shell → 通过 · ② rmux 自带 DA 代答 → 确认，须删 `da_filter` · ③ Terax action registry → 已存在（`commands.ts` 具名 id 如 `settings.open`/`theme.pick`） | 全走 rmux 方案成立，无需退回分层方案。**遗留**：`commands.ts` 仅 8KB，覆盖 settings/theme/spaces 等；编辑器与终端操作是否已全部注册为 action 需在 P1 补齐（属补齐，非重构） |
| **P1 骨架 + 安全 + 输入** | transport 适配层、Axum 去 Tauri 化、设备配对鉴权、修饰键条、命令面板 | 手机浏览器完成配对→连接→终端内运行 `vim` 并用修饰键条退出；未鉴权连接 100% 被拒 |
| **P2 会话韧性** | rmux 全面接管、ringbuffer 接入、seq resync、后台 detach | 飞行模式开关 20 次输出零丢失；重启 phantom-server 后 AI pane 存活；回前台可交互 < 800ms |
| **P3 可观测性** | agent_detect/da_filter 下移为流过滤器、AgentEvent、Web Push | AI 任务完成 → 手机推送到达 < 5s；claude/codex/gemini/pi 四类 agent 检测准确 |
| **P4 移动适配** | PWA、响应式布局、默认暗色、不拦截系统手势 | 系统滚动/捏合在终端与编辑器内均正常（不被 preventDefault 破坏）；系统输入法语音可直接输入 prompt；xterm 渲染降级链在目标设备矩阵全绿 |
| **P5 上下文闭环** | 图片注入降级链、MCP Server | 手机截图经 share_target → AI 正确识别，全程 ≤3 次点击 |
| **P6 多设备** | 眼镜/手机/笔记本同工作区协同 | 文件租约无冲突丢失；跨设备切换 pane 状态一致 |

---

## 十四、风险登记

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| ~~rmux SDK 不支持自定义 env / shell 启动脚本~~ | — | ✅ **已解除**。`EnsureSession` 提供 `.environment()/.shell()/.command()/.working_directory()` |
| ~~rmux 自带终端仿真且已代答 DA/DSR~~ | — | ✅ **已确认为真**，处置明确：删除 Terax `da_filter.rs`。转为实施项，非风险 |
| rmux `@pane option` 状态通道的事件时延/丢失特性未测 | agent 完成推送延迟或漏报 | P3 实测 `state_events` 在高频输出下的时延与背压；保留 `foreground` 轮询兜底 |
| rmux 0.9.0 为较新版本，SDK API 可能仍在演进 | 升级破坏适配层 | 锁定版本；`Rmux::capabilities()` 做能力协商而非假定；适配层集中隔离 rmux 调用 |
| xterm WebGL addon 在 AR WebView 崩溃 | 终端不可用 | 显式降级链 WebGL→Canvas→DOM，启动探测；建立设备矩阵实机验证 |
| Terax 上游持续演进，Phantom fork 分叉 | 长期维护成本 | 适配层严格隔离（alias 而非改业务代码），保持可 rebase；定期同步上游 |
| 前端体积超标 | 移动网络首屏不可接受 | 沿用并收紧 `.size-limit.json`，CI 卡门禁；语言包懒加载 |
| xterm/CodeMirror 拦截 touch 事件 | 破坏系统滚动与捏合，移动端体验劣化 | P4 逐项排查 `preventDefault` 与 `touch-action`；真机验证系统手势未被吞掉 |
| 多设备并发编辑同文件 | 数据丢失 | 文件租约（lease）+ 版本号冲突检测；**不上 CRDT**（复杂度不成比例） |
| `/tmp` 被 blob 填满 | 主机故障 | session GC + 24h TTL + 配额上限 |

---

## 附录 A：相对 v1 的改动

1. **修正事实错误**：MUSL 静态编译前提不成立，且对 PTY 有害 → cargo-zigbuild 锁 glibc 2.28（§12.1）
2. **基于实际代码确定 Terax 形态**：Tauri 应用（64.5k TS + 13.7k Rust），非 Rust TUI（§2）
3. **否决 WASM 方案**：前端已是 Web 技术栈，WASM 延迟收益为零（§2 定论一）
4. **确定复用路径**：transport 适配层 + Vite alias，业务代码零改动；含 11 个 Tauri plugin 逐项处置表（§4）
5. **重连协议具体化**：复用已有 `BoundedRingBuffer`，取代 v1 的一句话快照（§6.2）
6. **识别并规划 AI 可观测性层迁移**：`agent_detect`/`da_filter`/`shell_init` 共 1877 LOC 下移为流过滤器；这是全走 rmux 方案的最大难点（§6.3）
7. **新增移动输入层**：修饰键条 + 命令面板为主入口（§7）——v1 完全缺失。语音交给系统输入法、手势只求不破坏系统手势，均不自建
8. **显示不做特殊设计**：AR 眼镜按外接显示器对待，仅保留默认暗色与响应式折叠两条零成本默认值（§8）
9. **剪贴板改为降级链**：share_target 为移动端主路径（§9）
10. **新增安全模型**：设备配对与 token 鉴权（§11）——v1 仅有网络隔离，等同无鉴权
11. **新增 P0 可行性验证阶段**，路线重排，每阶段给可测指标（§13）
12. **新增风险登记**（§14）
