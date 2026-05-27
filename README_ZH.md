# Agent Monitor

> 一个轻量级、始终置顶的桌面悬浮组件，实时监控所有项目中的 AI 编程助手会话。基于 Tauri 2.0 构建。

![Tauri 2](https://img.shields.io/badge/Tauri-2.0-24C8DB?logo=tauri&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-1.70+-000000?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)
![License](https://img.shields.io/badge/License-MIT-00ff88)

---

## Agent Monitor 是什么？

Agent Monitor 是一个**像素机械风格的悬浮组件**，始终置顶在你的桌面上，一屏纵览所有项目中的 Claude Code 会话——无论是 VSCode 还是 CLI。

Claude Code 很强大，但会话散落在各处：你可能同时开着三个 VSCode 窗口、每个都有各自的 Claude Code 对话，终端里还跑着另一个。Agent Monitor 把它们都汇总到一个可自由缩放的小窗口里，实时更新。

### 它能告诉你

- **哪些项目**正在运行 Claude Code 会话
- **每个会话在做什么** — 执行工具、思考、回复、还是等待你的输入
- **实时的工具详情** — 例如 `Running Bash // cargo build`、`Editing // lib.rs`
- **Token 用量**、会话运行时长、消息数、费用估算
- **来源** — VSCode 还是 CLI，模型简称（OPUS / SONNET / HAIKU / MIMO）

---

## 状态系统

Agent Monitor 使用一套类似终端的视觉语言，便于快速扫视：

### 会话状态

| 符号 | 状态 | 颜色 | 含义 |
|---|---|---|---|
| `[>]` | 工作中 | 琥珀色 `#ffaa00` | Claude 正在执行工具调用 |
| `[~]` | 思考中 | 紫色 `#aa66ff` | Claude 正在内部推理 |
| `[#]` | 回复中 | 青色 `#00ccff` | Claude 正在生成文本 |
| `[=]` | 空闲 | 绿色 `#00ff88` | 完成一轮回复，等待中 |
| `[v]` | 已完成 | 暗绿 `#006633` | 等待你的下一个指令 |

### 项目状态

| 徽章 | 含义 |
|---|---|
| `N LIVE` / `[+]` | 项目中有活跃会话 |
| `DONE` / `[v]` | 项目中所有会话均已完成 |

### 头部计数器

| 计数器 | 含义 |
|---|---|
| 绿色 `[NN]` | 所有项目中跟踪的会话文件总数 |
| 黄色 `[NN]` | 尚未完成的会话数（需要关注） |

### 智能状态判断

- **Agent 多步骤感知** — 工具调用间的短暂停顿不会被误判为"已完成"，Claude 仍在处理时保持"工作中"
- **静默超时** — 卡住的 responding/thinking 状态在文件 10 秒无变化后自动转为 "done"
- **长思考检测** — 1-2 分钟的思考阶段，即使 JSONL 文件尚未更新，也能通过 file-history 目录正确检测

---

## 工作原理

### 会话发现

```
~/.claude/
├── projects/                    # 所有 Claude Code 会话存放处
│   ├── d--dev-MyProject/        # 编码后的项目路径 (workspaceToProjectName)
│   │   ├── <uuid>.jsonl        # 会话记录文件
│   │   └── ...
│   └── ...
├── file-history/                # 按会话记录活跃状态
│   ├── <session-id>/           # 目录的 mtime = 最后活跃时间
│   └── ...
└── ide/                         # VSCode 集成
    ├── <PID>.lock              # 活跃 VSCode 窗口元数据
    └── ...
```

1. **发现** — 扫描 `~/.claude/projects/` 中所有会话记录（`.jsonl`）文件
2. **活跃检测** — 交叉比对 `~/.claude/file-history/` 目录的修改时间
3. **IDE 匹配** — 解析 `~/.claude/ide/*.lock` 文件，检查 PID 是否存活，编码工作区路径以匹配项目目录
4. **增量解析** — 追踪文件修改时间和字节位置，仅重新解析自上次扫描后有变化的文件
5. **实时更新** — `PollWatcher`（每 2 秒 mtime 轮询）+ 前端轮询作为后备

### 状态解析

逐行解析 JSONL，提取：
- **消息角色** — `user`（你的输入 + 工具结果）vs `assistant`（Claude 的回复）
- **内容块** — `tool_use`（工具调用）、`thinking`（推理）、`text`（可见文本）
- **停止原因** — `tool_use`（交给工具执行）vs `end_turn`（本回合结束）
- **元数据** — 模型名称、token 用量、费用、时间戳

### 性能优化

- **Mtime 缓存** — 文件仅在修改时间变化时才重新解析
- **PID 缓存** — IDE 进程的存活检测结果被缓存，避免重复调用 `tasklist`
- **增量读取** — 记录字节偏移量，定位到上次位置，仅读新数据

---

## 安装

### 直接下载（推荐）

从 [GitHub Releases](https://github.com/Hoemr/agent-monitor/releases) 下载 `agent-monitor.exe`（约 4.5MB）。

双击运行，无需安装，无额外依赖。

### 从源码构建

```bash
git clone https://github.com/Hoemr/agent-monitor.git
cd agent-monitor

npm install
npx tauri build
```

构建产物位于 `src-tauri/target/release/agent-monitor.exe`。

前置条件：[Rust](https://rustup.rs/) 1.70+、[Node.js](https://nodejs.org/) 18+。

---

## 使用说明

| 操作 | 方式 |
|---|---|
| 打开 / 关闭窗口 | 左键点击托盘图标 |
| 移动窗口 | 拖拽标题栏 |
| 缩放窗口 | 拖拽任意边缘或边角 |
| 展开 / 折叠项目 | 点击项目文件夹标题 |
| 在 VSCode 中打开会话 | 点击任意会话行 |
| 手动刷新 | 点击 `[R]` 按钮 |
| 右键菜单 | 窗口任意处右键 |
| 调整轮询速度 | 右键 → Poll: 3s / 5s / 10s |
| 隐藏到托盘 | 右键 → Hide Window |
| 退出 | 右键 → Quit 或托盘菜单 → Quit |

---

## 设计哲学

### 为什么是桌面悬浮组件？

网页仪表盘藏在浏览器标签页里——在几十个标签页中很容易丢失。终端工具如 `claude status` 每次都要敲命令。

悬浮的、始终置顶的小窗口**始终可见但绝不打扰**。你可以在写代码时瞥一眼，看到长时间思考的会话开始回复了，然后在最佳时机切回去。

### 为什么用 Tauri 2.0？

| | Tauri 2.0 | Electron |
|---|---|---|
| 二进制大小 | ~4.5MB | ~120MB+ |
| 内存（空闲） | ~15MB | ~80MB+ |
| 原生体验 | 是（系统 WebView） | 内嵌 Chromium |
| 启动速度 | < 1 秒 | 3-5 秒 |
| 捆绑运行时 | 否 | 是 |

### 像素机械风

暗色终端配色（`#0a0a0f` 背景、`#00ff88` 绿色、`#ffaa00` 琥珀色），Consolas 等宽字体，硬边框，发光状态指示器——这一切都在传达一个信号：**这是程序员为程序员打造的工具**。

---

## 项目结构

```
agent-monitor/
├── src/                        # 前端 (HTML/CSS/JS)
│   ├── index.html
│   ├── style.css               # 像素机械风主题
│   └── main.js                 # 渲染、右键菜单、轮询
├── src-tauri/                  # Rust 后端
│   ├── src/
│   │   ├── main.rs             # 入口
│   │   └── lib.rs              # 扫描、解析、监视、托盘
│   ├── Cargo.toml
│   ├── tauri.conf.json         # 窗口/托盘/安全配置
│   └── icons/                  # icon.ico + icon.png
├── gen-icon.js                 # 像素风图标生成器
├── README.md                   # 英文文档
└── README_ZH.md                # 本文档（中文）
```

---

## 平台支持

| 平台 | 状态 |
|---|---|
| **Windows 10/11** | 完整支持、已充分测试 |
| **macOS** | 理论上可用（基于 Tauri 2.0 跨平台） |
| **Linux** | 理论上可用（需 WebKitGTK） |

---

## License

MIT
