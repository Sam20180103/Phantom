# Phantom：下一代无头 AI 协作工作站实施方案

**Phantom (幻影)** 是一款专为无头（Headless）主机、私有 Mesh 网络以及移动 AR 办公场景设计的极简 AI 开发环境。

本方案旨在剥离传统桌面 IDE 的冗余负载，将 **Terax** 的纯键盘流、AI 审查前端与 **rmux** 的会话持久化引擎深度融合，构建一个极低资源占用、永不掉线的分布式 Web IDE。所有计算与上下文均由运行在 CentOS 8 上的核心守护进程承载。

---

## 一、 系统架构蓝图

Phantom 摒弃了 Electron/Tauri 等重型本地视窗框架，采用完全解耦的 B/S 分布式架构：

| 模块层级 | 技术栈选型 | 核心职责 |
| --- | --- | --- |
| **前端展现层** | React + xterm.js + CodeMirror | 终端渲染、毫秒级 Diff 审查视图、全局快捷键拦截、轻量级浮层 UI。 |
| **中间件与网关** | Rust (Axum) + Tokio | 托管前端静态资产、维持 WebSocket 双向通信、本地文件读写、MCP 协议代理、图像拦截落盘。 |
| **底层算力引擎** | Rust (`rmux-server` + `rmux-sdk`) | PTY 进程隔离与守护、AI 终端状态快照读取、跨设备会话 Attach/Detach 控制。 |

---

## 二、 核心工作流改造

### 1. 永不掉线的 AI 会话 (Session Persistence)

* **进程守护：** AI 命令行工具（如 Claude Code 或 OpenCode）不再作为临时子进程运行，而是托管在主机后台独立的 `rmux-server` 守护进程中。
* **断线重连：** 移动办公网络环境（如切换 Wi-Fi、AR 眼镜休眠）导致前端 WebSocket 断开时，Axum 仅销毁网络连接。后端的 `rmux` 自动挂起该 Pane。重新连入网络后，Axum 通过 `rmux-sdk` 的 `attach` 接口秒级恢复屏幕快照，确保长时间运行的代码重构任务不受网络抖动影响。

### 2. 丝滑的多媒体剪贴板闭环 (Clipboard Injection)

彻底打破终端无法直接粘贴图片的限制，实现“截图即上下文”的极客体验。

* **前端拦截并发：** 浏览器端监听 `Cmd+V` (或 `Ctrl+V`)，识别 `image/*` 类型，将其转为 Blob 数组，通过 WebSocket 并发传输。
* **后端落盘与注入：** Axum 接收流数据后，保存至 `/tmp/phantom_workspace/` 并生成绝对路径（如 `/tmp/.../clip_1.png`）。Axum 随后调用 `rmux-sdk` 向当前激活的终端句柄注入路径文本（以空格分隔），光标自动跟随。
* **视觉确认（微 UI）：** 路径注入的同时，前端右下角弹出半透明气泡，展示包含路径尾号的微缩略图，5 秒后自动消散，提供无干扰的视觉确认。

### 3. Git Diff 自动化审查 (Seamless Review)

* **状态监听：** 核心工作区由后端集成的 `git2-rs` 进行毫秒级监听，或通过 `rmux-sdk` 捕获 AI 工具的 `git apply` 动作。
* **前端联动：** 后端检测到文件变更后，通过 WebSocket 推送 `FileChanged` 事件。前端 CodeMirror 视图自动刷新 Diff，无需手动输入任何 Git 审查命令即可完成代码走查。

---

## 三、 CentOS 8 部署与静态编译规范

针对 CentOS 8 的底层系统环境，Phantom 必须采用极致隔离的构建与部署策略，以规避版本依赖陷阱。

### 1. 规避 GLIBC 陷阱：MUSL 静态编译

CentOS 8 的 `glibc` 停留在 2.28 版本，标准的 Rust 动态编译产物无法直接运行。Phantom 后端须采用纯静态构建：

```bash
# 1. 添加 musl 编译目标
rustup target add x86_64-unknown-linux-musl

# 2. 采用 musl target 静态编译 Phantom 核心
cargo build --release --target x86_64-unknown-linux-musl

# 3. 产物提取 (纯静态二进制，单文件部署)
cp target/x86_64-unknown-linux-musl/release/phantom-server /opt/phantom/

```

### 2. Systemd 守护进程配置

在 CentOS 8 上，将 Phantom 注册为标准的系统服务，确保开机自启与崩溃重启。创建 `/etc/systemd/system/phantom.service`：

```ini
[Unit]
Description=Phantom Headless AI Workstation
After=network.target

[Service]
Type=simple
User=phantom_user
WorkingDirectory=/opt/phantom/workspace
# 绑定私有网段，确保安全隔离
Environment="PHANTOM_HOST=100.64.0.x"
Environment="PHANTOM_PORT=8080"
ExecStart=/opt/phantom/phantom-server
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target

```

### 3. 私有组网与终端环境依赖

* **网络隔离绑定：** 强制 Axum 监听 Headscale 虚拟网卡分配的内网 IP（如 `100.64.x.x`），并在 CentOS `firewalld` 中只放行该虚拟网段对 8080 端口的 WebSocket 连接，实现公网隐身。
* **依赖补齐：** 确保 CentOS 8 宿主机安装了基础终端转义数据库，防止 `rmux` 运行 AI 工具时出现 ANSI 颜色丢失或乱码：
```bash
sudo dnf install ncurses ncurses-compat-libs

```



---

## 四、 演进路线

* **Phase 1 (基础骨架):** 跑通 Axum 托管、React 前端渲染与基础 xterm.js 的 WebSocket 通信。
* **Phase 2 (算力接管):** 引入 `rmux-sdk`，实现进程守护、多开终端、断线重连。
* **Phase 3 (体验跃升):** 闭环“多图片并发剪贴板 -> Axum 落盘 -> `rmux` 路径注入 -> AI 识别”的完整工作流，并实装前端气泡 UI。
* **Phase 4 (上下文引擎):** 将 Rust 后端扩展为标准的 MCP Server，使终端内的 AI 能够直接获取工作区的全局诊断与架构信息。