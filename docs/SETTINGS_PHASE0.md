# Phase 0 简报 — 设置页与可配置化

日期：2026-10-21 · 基线：`ce5b89b`（47 Rust tests / 24 headless UI checks 全过）·
说明：仓库中**不存在** `AGENTS.md`，实际约定以 `README.md`、`docs/PHASE0.md`、
`docs/UI_CONTRACT.md` 为准（已读）。

## 1. 现有设置相关代码与缺口（全部为实测位置）

| 现状 | 位置 | 缺口 |
|---|---|---|
| 语言：前端 `I18N` 字典 + `localStorage['am-lang']`，默认跟随 `navigator.language`；footer 的 `lang-btn` 切换 | `src/main.js:40-139,503-511`、`src/index.html:32` | 只作用于桌宠；与托盘（`system_is_chinese()`，`lib.rs:588`）不一致；无 `system` 选项 |
| 刷新：worker 写死 `hidden/empty ? 30 : 5` 秒 | `src-tauri/src/lib.rs:552-586` | 不可配置；启动/变更无唤醒通道 |
| 行数：`PetPrefs.max_visible_rows` 已存在但**从未被使用**；高度按任务数算，`MAX_VISIBLE_ROWS=5` 写死 | `model.rs:348-367`、`pet.rs:18,66-69`、`lib.rs:157-168` | 设置字段未接线；`expanded_height_logical` 上限 5，需 1..=8 |
| 桌宠时序：`hover_expand_ms` / `collapse_delay_ms` 已存在，经 `PetStateView` 下发 | `model.rs:348-367`、`pet.rs:252-290`、`main.js:408-430` | 无 UI 可改；`snap_distance`、`hidden_sliver` 是 Rust 常量 |
| 保留期：Codex 6h、DSH 6h、ZCode 24h、Claude 6h/24h 写死在适配器 | `agents/codex.rs:24,89,282`、`agents/dsh.rs:25,193`、`agents/zcode.rs:21,172`、`agents/claude_code.rs:198-201` | 无 `ScanCtx` 注入；running/waiting 的保留豁免不统一 |
| Agent 开关：`build_registry()` 固定四项 | `agents/mod.rs:101-108`、`monitor.rs:18-38` | 无按设置重建 registry 的路径 |
| 外观：CSS `light-dark()` 跟随系统 | `style.css:14-51`、`tauri.conf.json` windowEffects | 无显式 light/dark 覆盖；`Window::set_theme` 已确认可用（tauri 2.11.2） |
| Dismiss：`Monitor.dismissed` 仅内存 | `monitor.rs:15,68-105` | 重启即忘；“清除已忽略任务”无持久化语义 |
| 旧 v0.3.1 设置：语言 / 轮询 3-5-10s / 窗口高度 s-m-l / 鼠标穿透 / 透明度 30-100% | `git show v0.3.1:src/main.js`、`src/index.html` | 无设置窗口、无 Rust 单一数据源 |

## 2. 本阶段文件清单（已核对存在性）

**新增**
- `src-tauri/src/settings.rs` — Settings 模型 / 校验 / 原子持久化 / 迁移
- `src/settings.html`、`src/settings.css`、`src/settings.js` — 独立设置窗口（620×560）
- `scripts/ui-selftest-settings.html`、`scripts/ui-test-settings.sh` — 设置页无头自检
- `docs/SETTINGS.md` — 每个 key 的契约

**修改**
- Rust：`src-tauri/src/lib.rs`（窗口/托盘/命令/worker/主题）、`model.rs`（PetPrefs 精简）、`pet.rs`（行数 1..=8、参数化 snap/sliver、PetStateView）、`monitor.rs`（registry 重建、保留期、dismissed 持久化）、`agents/mod.rs` + `agents/{codex,dsh,zcode,claude_code}.rs`（ScanCtx 注入）
- 前端：`src/index.html`、`src/main.js`、`src/style.css`（i18n/settings 事件/齿轮/Cmd+,/行数/动画）
- 配置：`src-tauri/capabilities/default.json`（windows 增加 `settings`）
- 脚本：`scripts/ui-test.sh`（聚合两套）、`scripts/ui-selftest.html`（新增设置相关桌宠检查）
- 文档：`README.md`、`docs/ACCEPTANCE.md`、`docs/UI_CONTRACT.md`、`docs/PERF.md`

**不改**：`jump.rs` 跳转分级本身、适配器数据源、既有 47 个测试（只增不改语义）。

## 3. 旧设置映射与取舍

| 旧设置 | 新方案 | 结论 |
|---|---|---|
| 语言 English/中文 | `language: system/en/zh`，统一驱动桌宠、设置窗口、托盘菜单/tooltip | **保留并扩展** |
| 轮询 3/5/10s | `refresh_interval: auto/3s/5s/10s`，隐藏或空仍 30s 下限，校验 ≥2s | **映射**；文件事件仍是主来源 |
| 窗口高度 默认/紧凑/扩展 | `max_visible_rows: 3/5/8`；高度 = 44 + rows×64 + 26，受 visible frame 钳制 | **映射**（旧值是任意像素，隐含 3/5/8 行） |
| 鼠标穿透 | P1“收起态点击穿透”（托盘 Show Pet 安全恢复） | **可选恢复**，P0 不启用 |
| 透明度 30–100% | P1“材质浓度”（仅 CSS tint 强度） | **不恢复窗口整体 alpha**：会破坏 NSVisualEffectView vibrancy 与 popover 视觉；如做只调 tint |
| 托盘 Preferences | 托盘“设置…” + Cmd+, + 桌宠 footer 齿轮 | **恢复并升级为独立设置窗口** |
| 旧 localStorage | `import_legacy_settings` 命令一次性迁移 lang/pollMs（其余项不迁移并说明） | **首次启动迁移，尽力而为** |

明确不迁移：旧 `opacity`/`clickThrough`（改为 P1 新语义）、旧 `size`（s/m/l 映射到 3/5/8 行）、旧的 OK/Cancel 交互（改为 macOS 即时保存）。

## 4. Settings 数据结构草案

`~/Library/Application Support/agent-monitor/settings.json`，`version: 1`，serde 缺省字段回退默认、未知字段忽略、原子写（tmp+rename）、损坏→备份 `.bak`+默认值。

| key | 类型 | 默认 | 范围/取值 |
|---|---|---|---|
| `version` | u32 | 1 | 固定 |
| `language` | enum | `system` | system/en/zh |
| `appearance` | enum | `system` | system/light/dark |
| `show_summary` | bool | true | — |
| `show_jump_level` | bool | true | — |
| `refresh_interval` | enum | `auto` | auto/3s/5s/10s（≥2s） |
| `retention_hours` | u32 | 6 | 1/6/24/72（就近吸附） |
| `enabled_agents` | struct | 全 true | codex/dsh/zcode/claude_code |
| `max_visible_rows` | u32 | 5 | 3/5/8（就近吸附，窗口支持 1..=8） |
| `hover_expand_ms` | u64 | 150 | 100..=1000 |
| `collapse_delay_ms` | u64 | 800 | 300..=2000 |
| `snap_distance` | u32 | 16 | 0..=40（0=不吸附） |
| `hidden_sliver` | u32 | 14 | 8..=24 |
| `edge_hide_enabled` | bool | true | false 时“隐藏”= 收起到托盘（窗口 `hide()`，托盘 Show Pet 恢复） |
| `animations` | enum | `system` | system/always/off |
| `toast_on_degraded` | bool | true | false 仅静默“降级”toast，**失败始终提示** |
| `auto_dismiss_invalid` | bool | false | 跳转判定目标失效后自动 dismiss，toast 提供“撤销” |
| `legacy_imported` | bool | false | 旧 localStorage 迁移标记 |

几何/位置仍由 `pet.json` 保存（高频状态，避免每次拖动重写配置）；`pet.json` 的
`hover_expand_ms/collapse_delay_ms/max_visible_rows` 迁移到 `settings.json` 后
旧字段被忽略（serde 未知字段容错），并明确记录“双文件、各司其职”。

**命令/事件**：`get_settings` / `set_settings(settings)` / `reset_settings(scope)` /
`get_settings_meta` / `clear_dismissed` / `import_legacy_settings` / `open_settings` /
`close_settings` / `restore_dismissed_task`；事件 `settings-changed(Settings)`。
设置窗口由 Rust 在 setup 创建（label `settings`，默认隐藏、单实例、关闭=hide）；
主/设置两个 webview 都监听事件，前端不轮询。

## 5. 测试与文档

- Rust：settings 单测（默认值、范围夹取、损坏回退 `.bak`、原子写、枚举/缺省/未知字段、保留期、行数、语言解析）、pet 行数 1..=8、monitor registry/保留期/dismissed 持久化、agents 保留豁免。
- 无头 UI：保留现有 24 项；新增设置页自检（分组、即时保存、校验、恢复默认二次确认、Cmd+,、语言切换、focus ring）与桌宠集成（行数 max-height、摘要/等级开关、齿轮入口、settings-changed 即时刷新）。
- 文档：新增 `docs/SETTINGS.md`；更新 README/ACCEPTANCE/UI_CONTRACT/PERF。
- 验收命令：`cargo test --release`、`cargo build --release`、`bash scripts/ui-test.sh`、
  `bash scripts/ui-test-settings.sh`、`--dump-tasks` 前后对比、`AGENT_MONITOR_DEBUG_GEOMETRY=1`、`npm run perf`。
