# Phantom 任务清单

> 最后更新：2026-07-27
> 口径：以**前端实际调用的命令**为准 —— 前端共调用 78 个 `invoke` 命令，
> 后端实现一个，前端对应功能才算能用。这是唯一不会自欺的进度指标。

## 编译 / 运行（详见 [`crates/phantom-server/README.md`](crates/phantom-server/README.md)）

```bash
# 1. 前置依赖：安装 rmux 守护进程（推荐，实现终端会话跨服务重启保活与多端接管）
cargo install --git https://github.com/helvesec/rmux --rev b9b6b86e87792ad28bcdb3815ff08cce22400bb1 rmux

# 2. 构建：前端 dist + 服务端二进制
pnpm install
pnpm run build                             # -> dist/
cargo build -p phantom-server --release    # -> target/release/phantom-server

# 3. 运行（在仓库根目录，web root 默认 ./dist）；复用 code-server 的证书+密码
UNSET=$(for v in $(env | grep -oE '^CLAUDE_CODE_[A-Z_]+'); do printf ' -u %s' "$v"; done)
env $UNSET NO_PROXY=127.0.0.1,localhost,<大脑endpoint域名> \
  ./target/release/phantom-server
```

- **PTY 后端**：在 `~/.config/phantom/config.yaml` 中配置 `pty-backend: rmux`（缺省为 `local`）。`rmux` 模式下终端会话独立常驻，服务重启或网络闪断后刷新即可恢复，支持多端重新附加（Reattach）及 AI 状态侦测。
- 配置读取顺序：`$PHANTOM_CONFIG` -> `~/.config/phantom/config.yaml` -> `~/.config/code-server/config.yaml`。
- `NO_PROXY` 必带 `127.0.0.1,localhost`（本地模型）+ 编排大脑域名（国内端点直连，绕开抽风代理）。
- 启动前剥掉 `CLAUDE_CODE_*`（或别从 Claude Code shell 里起），否则会话里的 claude 不落盘。
- dist 改了热生效（服务读磁盘）；二进制改了要重启。

## 总览

| 指标 | 数值 |
| --- | --- |
| 前端调用的命令总数 | 78 |
| 后端已实现 | **73**（dispatch 分支 + `pty_write` 二进制帧 + `open_settings_window` 前端 shim） |
| 待实现 | 5（其中 2 个 Windows/WSL 专属，Linux 部署下不需要） |
| Linux 下真实待办 | **3** |
| Rust 测试 | 204 通过 |
| 前端测试 | 470 通过 |

进度：**73 / 76 ≈ 96%**（分母已剔除 WSL 两项）

---

## 一、已完成

### 1.1 架构地基

- [x] **Vite alias 替换 Tauri IPC** —— 9 个 alias 把 `@tauri-apps/api/*` 换成
      WebSocket shim。上游约 600 个文件零改动，fork 可持续 rebase。
- [x] **抽出 `phantom-core`** —— 无 Tauri 依赖，桌面版和服务端共用。
      已迁入：`fs/`、`git/`、`net`、`secrets`、`path`、`proc/`、`pty/`、`workspace`。
- [x] **`PtySink` / `FsEventSink` / `FileEventSink`** trait 缝，替代 Tauri
      `Channel` / `AppHandle::emit`。
- [x] **二进制线路协议** —— 终端 I/O 走 `[TAG][u32 id][payload]`，控制面走 JSON。
- [x] **流式协议** —— `ServerMsg::Stream { req, event }`，按 req 路由。
      复用 Channel 机制（pty 按 id、流式按 req）。

### 1.2 鉴权与传输

- [x] HTTPS + 密码登录，复用 code-server 证书和密码
- [x] **HTTP/2 Cookie 分片修复**（`get_all()`，curl 复现不出，带 2 回归测试）
- [x] 鉴权覆盖静态资源（`.layer()` 非 `.route_layer()`）
- [x] `Secure` cookie 仅 TLS 下设置
- [x] Service Worker 墓碑（自注销版，免鉴权）

### 1.3 终端 / rmux

- [x] rmux 后端 open/write/resize/close（detach 非 kill）
- [x] 会话活过 phantom-server 重启（PID 同一性验证）
- [x] rmux 后端跑 `AgentDetector`（此前 rmux 路径丢弃全部 OSC 777）
- [x] 会话名唯一化 + `create_only`（修复新标签页静默接管别人会话）
- [x] `pty_sessions` / `pty_attach` + `phantom-` 前缀校验
- [x] 接管时回放历史（`PaneOutputStart::Oldest`，上限约 4MB）
- [x] 重连 UI：横幅（flex 列不遮输出）+ 新建标签 "Reattach" 分区，共用轮询

### 1.4 文件 / 搜索 / Git

- [x] 文件树 + 13 个 fs 命令
- [x] **全局搜索 5 命令**（`fs_grep` / `fs_grep_interactive` / `fs_glob` /
      `fs_search` / `fs_list_files`）—— command palette 里 `#` 前缀，
      `Ctrl+Shift+P` 直达。generation 计数器 per-connection（两浏览器互不取消）。
- [x] **19 个 git 命令**（Source Control 面板）
- [x] `bootstrap_registry` 缺失修复（服务端注册表开机为空的 bug）

### 1.5 AI 面板 / 设置 / 交互

- [x] **secrets 4 命令** —— 0600 JSON 文件后端（headless 无 D-Bus keyring）
- [x] **lm_ping** —— 探测本地模型服务器（内建允许私网）
- [x] **ai_http_stream** —— AI HTTP 代理，流式，保留 SSRF 防护 + DNS-rebind pin
- [x] **open_settings_window** —— 浏览器标签页开 `settings.html`（具名，去重）
- [x] **shell 8 命令** —— one-shot（`shell_run_command`，超时强杀）、agent 持久
      session（`shell_session_open/run/close`，cwd 跨 run 持久）、后台进程
      （`shell_bg_spawn/logs/kill/list`）。整模块搬 core，desktop 薄包装。
- [x] **history 4 命令** —— `history_suggest/commands/list/record`。shell 历史
      （zsh/bash/fish）+ PATH 扫描建索引，共享（AppState）避免每连接重扫。
      command palette `>` 模式 + 终端 block 自动补全。
- [x] **agent hooks 2 命令** —— `agent_enable_hooks/hooks_status`。把 OSC 777
      notify hook merge 进 agent 配置（claude/codex/gemini/pi）。merge 保留
      用户已有 hook（tokless/codegraph），幂等。33 单测。
- [x] 截图粘贴 / 拖拽上传（`/blob`，32MB，24h GC）
- [x] 不安全上下文 polyfill

---

## 二、待开发

按「解锁价值 ÷ 工作量」排序。

### P1 — 多 CLI 编排（护城河，最大一块）

**目标**：多个订阅制 CLI（claude 后端、agy/antigravity 前端、opencode 测试，
可扩展）并行跑，互相协作。例：claude 写后端+接口 → agy 读接口写前端 →
opencode 跑 playwright → 结果回给两者。

**根本约束**：用户走订阅制 CLI，无 API key，所以面板内 AI 聊天（`run_subagent`
/ `spawnManagedAgent`，都要 API key）那条编排路走不通。编排者必须外置且可插拔。

**核心设计决策**：

1. **文件即协议** —— agent 间不建消息总线。约定 `.phantom/handoff/*.md`，
   CLI 用自己的文件工具读写。claude 写 `handoff/api.md`，agy 读它。coding
   agent 读写 markdown 是基本功，不需要 IPC。

2. **内容走文件，触发走注入** —— 别刮 agy 的 TUI 输出（全屏重绘 + ANSI，极脏）。
   让 agy 自己把报错写进 `handoff/error.md`，Phantom 只 `send_text` 一句
   "读 handoff/error.md 修一下" 到 claude 会话。长内容、可回溯，且注入的只是
   一行不怕格式乱。

3. **注入时机靠 hook** —— 往 claude 会话注入必须等它在 prompt 等输入，否则塞进
   正在跑的命令里乱套。agent hooks 的 `finished`/`attention` 状态正好告诉你
   何时空闲。封装进 `nudge()`，编排者不用管时机。

4. **编排者可插拔** —— 手动触发和自动编排调**同一个** `dispatch(agent, task)`
   / `nudge(agent, msg)`。手点 = 你调；以后云 API 或本地 Qwen 3.5 8B 当大脑 =
   它调同一函数。零代码改动，只换"谁按按钮"。

**配置**（加工具不改代码，一并解决 `AGENTS`/`DEFAULT_AGENTS` 硬编码）：

```yaml
# .phantom/agents.yaml
agents:
  claude:   { cmd: "claude",      role: "后端 + Python AI 引擎" }
  agy:      { cmd: "antigravity",  role: "前端" }
  opencode: { cmd: "opencode",    role: "playwright 单元测试" }
handoff_dir: ".phantom/handoff"
```

**底层原语现状**：

- 多 CLI 并行 —— **已有**（rmux 前后端分离，每 CLI 一会话）
- 每 agent 状态 —— **已有**（agent hooks）
- 跨会话注入 —— **已有**（`send_to_agent(agent, task)` 按名拿 pane 发文字，
  不占本地 pty；`agent_dispatch` 命令暴露给编排者）
- agent 间传消息 —— **不用建**（文件系统就是通信层）

**分步（每步独立验证）**：

| 步 | 内容 | 验证 |
| --- | --- | --- |
| 1 | `.phantom/agents.yaml` + 命名会话（注册/列出），检测器/hook 的 agent 列表改从 YAML 读 | 开两个命名 agent，`pty_sessions` 显示名字+角色 |
| ✅2 | `dispatch(agent, task)` —— 按名发文字到会话 | ✅ 发命令给 claude 会话，shell 实际执行（marker 文件验证），死会话拒绝不复活 |
| ✅3 | `nudge(agent, msg)` —— 等该 agent hook 空闲再 `send_text` | ✅ `working`/`started` = 忙则等（默认 30s，可传 `timeoutMs`），其余（含无 hook 的裸 shell）= 空闲立即发。实测：裸 shell 2ms 直发；busy 等满超时 1614ms 才发；命令结束（133;D）后又秒回 |
| ✅4 | 前端：agent 面板列各角色 + 状态 + 一键 A→B 转发 | ✅ 侧边栏 Agents 视图（Bot 图标）：列 yaml 里各 agent + 角色 + 运行绿点；Open=`agent_ensure`（建会话不 wire）+ 复用 attach 开终端 tab；每行 dispatch 输入框；底部 Forward `from ▾ → to ▾` 一键 nudge。控制面，非终端宿主，无悬挂 sink |
| ✅5 | 编排者接入（云 API 或本地 Qwen）——调同一 `dispatch`/`nudge` | ✅ **真机 GLM 5.2 E2E 通过**：目标→GLM 决策→dispatch 给 claude agent→裸 shell 执行→read_handoff 确认→finish，proof.md 实际写出 "ORCHESTRATED"。`orchestrator.rs`（config+调用+工具 schema）+ `run_orchestration` loop（finish/16 步/Stop）+ 前端目标框/步骤日志/Stop。**大脑复用 Models 面板 provider**（设一次），key 服务端从 secret 读、不过线；缺省回落 agents.yaml。封顶自动，approve 开关下一片 |

**已定**：`open_agent` 开**裸 shell**（决策 B，保灵活）——不自动跑 yaml 里的
`cmd`，由编排者 dispatch 启动 CLI。裸 shell 无 hook 信号 → nudge 视其为空闲直发；
一旦跑起 hooked CLI，其 OSC 777 working/attention/finished 驱动 nudge 的等待。

**待确认细节**：handoff 文件格式约定；antigravity/opencode 的 hook 机制
（各 CLI 配置格式不同，需逐个查）；agy 是否支持 OSC 777 或需别的 delivery。

### P3 — LSP（6 命令）

`lsp_spawn` / `send` / `kill` / `detect` / `resolve_root` / `host_pid`

1014 行，5 文件里 `mod.rs` + `session.rs` 碰 Tauri。`session.rs` 是事件 emit 路径，
需要和 `PtySink` 同样的 sink 缝。

**放最后是有意的**：LSP 吃内存，而 Phantom 相对 code-server 的卖点之一是轻。
建议做成配置项按语言开，别默认全开。

### N/A — WSL（2 命令）

`wsl_home`、`wsl_list_distros` —— Windows 专属，Linux 部署不会调用。可直接返回空。

---

## 三、踩过的坑 / 经验（接命令前必看）

| 坑 | 教训 |
| --- | --- |
| **camelCase 参数** | 前端照 Tauri 写，发 camelCase（`repoRoot`/`showHidden`）。dispatcher 手写的 arg struct 必须带 `#[serde(rename_all = "camelCase")]`，否则参数**静默丢失**（有 `default` 的字段取默认值，无 default 的报 missing field）。git 面板和文件树隐藏文件都栽在这。测试时必须用前端真实拼写，别用自己假设的 snake_case。 |
| **NO_PROXY** | `phantom-server` 部署需 `NO_PROXY=127.0.0.1,localhost`，否则本地模型服务器（Ollama/LM Studio）被 `HTTP_PROXY` 拦截返 503。桌面版同样行为。 |
| **rustls provider** | reqwest（aws-lc-rs）+ axum-server（ring）各带一个 crypto provider，rustls 0.23 没法自动选，启动即 panic。启动时 `rustls::crypto::ring::default_provider().install_default()`。 |
| **返回结构别猜** | 写测试断言前先读 core 的返回类型（`GrepResponse.hits` 不是 `matches`）。 |
| **别盲杀 multiplexer 会话** | 清理只杀本 session 建的会话，rmux 里跑着用户的活。 |

## 四、已知问题 / 技术债

| 问题 | 影响 |
| --- | --- |
| ~~刷新即漏会话~~ **已修**（方案 B，`7d87033`） | 每 leaf 建时给稳定会话名（`newLeafSession`），持久化进 pane 树。`pty_open` 带 `session` → create_or_reuse（回放 backlog），刷新接回同一 shell（含 scrollback），不再 orphan；无名则维持旧的 create_only。名字校验 `phantom-` 前缀、拒 agent 前缀、每 leaf 唯一。实测 4 次刷新会话数持平（原 +1/次）。另 `pty_kill` + 横幅 × 仍在（清历史孤儿） |
| session 存内存，服务重启即登出 | 每次重启用户需重新登录（浏览器拿旧 cookie 死重连） |
| rmux 回放上限约 4MB | 长时会话接回去只看到尾部 |
| `send_text` 要求合法 UTF-8 | 非 UTF-8 字节写入被拒（SDK 无 raw bytes API） |
| `git_checkout_branch` 只切不建 | 面板没法新建分支（上游设计） |
| `src-tauri` 本地无法验证 | 桌面版可能编译不过（无 webkit2gtk） |

## 五、安全事项

- 服务监听 `[::]:8099`，公网域名，仅 TLS + 密码
- 会话密码曾出现在开发对话记录，**建议轮换**
- `pty_attach` 有前缀校验，但任何登录用户能接管任何 Phantom 会话
  （单用户是特性，多用户需重新设计）
- secrets 存 `~/.local/share/phantom/secrets.json`（0600）

---

## 六、建议下一步

1. **多 CLI 编排**（P1，护城河）—— 从第 1 步命名会话开始。编排者可插拔，
   以后接云 API 或本地 Qwen。这是项目真正的差异化，命令覆盖到 96% 后的主线。
2. **LSP**（P3）—— 6 命令，需要 sink 缝（事件 emit）。做成按语言开的配置项。

> 命令覆盖已 73/76 ≈ 96%，剩下的 LSP 是补全，多 CLI 编排才是往前走的方向。
