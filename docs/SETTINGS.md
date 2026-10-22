# 设置（Settings）契约

Agent Monitor 的配置只有一个数据源：Rust 的 `Settings`（`src-tauri/src/settings.rs`），
存于 `~/Library/Application Support/agent-monitor/settings.json`。桌宠窗口、设置窗口、
托盘菜单/tooltip 都渲染同一份值，设置变更通过事件即时下发，前端从不轮询。

## 数据文件

| 文件 | 内容 | 写入时机 | 说明 |
|---|---|---|---|
| `settings.json` | 用户配置（下表的全部 key） | 每次修改即时写 | 原子写（`settings.json.tmp` → rename）；损坏时改名为 `settings.json.bak` 并回退默认值，应用仍可启动 |
| `pet.json` | 桌宠位置与隐藏状态 | 拖动/吸附/隐藏/展开后 | 高频状态，与配置分离，避免每次拖动重写设置；旧版字段（`hover_expand_ms` 等）被忽略 |
| `icons/collapsed.img`、`icons/hidden.img` | 自定义图标原图 | 上传/移除时 | 只存二进制，设置文件仅存 `icon_style`；Rust 侧按文件头校验、≤2MB、原子写 |
| `dismissed.json` | 已忽略任务 `id → last_activity_secs` | 忽略/撤销/清除时 | 跨重启保留；任务出现更新活动后自动恢复显示 |

数据目录可用环境变量 `AGENT_MONITOR_DATA_DIR` 覆盖（仅测试/验收用，正常运行不设置）。
设置**不含**任何正文、路径、账户或遥测数据。

## 设置项

“生效时机”一栏中“即时”表示保存后立即作用于桌宠/设置窗口/托盘，无需重启。
P0 的全部设置项都不需要重启。

### 通用 / 外观

| key | 类型 | 默认 | 范围 | 生效 |
|---|---|---|---|---|
| `language` | `system \| en \| zh` | `system` | — | 即时：桌宠 UI、设置窗口、托盘菜单/tooltip、`documentElement.lang` |
| `appearance` | `system \| light \| dark` | `system` | — | 即时：`Window::set_theme`（macOS 为 app 级）+ 页面 `color-scheme`，vibrancy 材质同步 |
| `show_summary` | bool | `true` | — | 即时：桌宠行内第二行摘要显隐 |
| `show_jump_level` | bool | `true` | — | 即时：行尾 L3/L2/L1/L0 显隐 |
| `new_badge` | bool | `true` | — | 即时：收起/隐藏时显示“自上次查看后新完成”的绿色角标与数量（0 时自动隐藏） |

### 监控

| key | 类型 | 默认 | 范围 | 生效 |
|---|---|---|---|---|
| `refresh_interval` | `auto \| 3s \| 5s \| 10s` | `auto` | 自动 = 可见 5s / 隐藏或空 30s；显式值最低 3s（硬下限 2s，任何输入不会更小） | 即时：唤醒调度线程，下一轮按新间隔执行；隐藏/空闲仍保持 30s 下限 |
| `retention_hours` | u32 | `6` | `1 / 6 / 24 / 72`（就近吸附） | 即时：下一轮扫描传入所有适配器；`running` / `waiting_input` / `waiting_approval` 永不因保留期隐藏 |
| `enabled_agents` | 对象 | 全 `true` | `codex` / `dsh` / `zcode` / `claude_code` 布尔 | 即时：重建 registry，关闭的适配器**不再采集**（不是采集后过滤） |
| `max_visible_rows` | u32 | `5` | `3 / 5 / 8`（就近吸附；几何层支持 1..=8） | 即时：展开窗口高度 = `44 + rows×64 + 26`，列表 max-height 同步，仍受屏幕 visible frame 钳制 |

### 桌宠

| key | 类型 | 默认 | 范围 | 生效 |
|---|---|---|---|---|
| `hover_expand_ms` | u64 | `150` | `100..=1000` | 即时：悬停展开延迟 |
| `collapse_delay_ms` | u64 | `800` | `300..=2000` | 即时：离开后收起延迟 |
| `snap_distance` | u32 | `16` | `0..=40`（0 = 不吸附） | 即时：拖拽结束吸附判定（逻辑点 × 屏幕缩放） |
| `hidden_sliver` | u32 | `14` | `8..=24` | 即时：边缘隐藏后露出的小方块边长（该方块贴在屏幕边上：吸附的那条边完全不可见，垂直两条边只露出这么多）；已隐藏时重新贴边 |
| `collapsed_size` | u32 | `72` | `48..=96`（px） | 即时：收起/吸附后桌宠的边长；窗口尺寸与 CSS 同步（实测 48/72/96 → 窗口 48/72/96，右锚定） |
| `auto_hide_after_s` | u32 | `0`（关闭） | `0..=60`（秒） | 即时：吸附到屏幕边缘后，空闲该时长即自动隐藏（跟随 `edge_hide_enabled`：贴边半隐藏或收起到托盘）。悬停、拖拽、展开会取消；未吸附（`edge=none`）不触发；单次计时器，无轮询 |
| `edge_hide_enabled` | bool | `true` | — | 即时：关闭后“隐藏”= **收起到托盘**（`hide()`，不移动窗口）；托盘“显示桌宠”/再次启动恢复；正在边缘隐藏时切到托盘隐藏 |
| `icon_style` | `builtin \| custom` | `builtin` | — | 即时：使用内置三条杠图案，或使用上传的自定义图片（未上传则回退内置）；见 [`docs/ICONS.md`](ICONS.md) |
| `animations` | `system \| always \| off` | `system` | — | 即时：`off` 关闭桌宠窗口几何动画与两个窗口的 CSS 过渡；`always` 忽略 `prefers-reduced-motion`；`system` 尊重系统减弱动态效果 |
| 位置重置 | 动作 | — | — | 设置页“重置桌宠位置”调用既有 reset 逻辑（右侧 45% 高度） |

### 跳转

| key | 类型 | 默认 | 范围 | 生效 |
|---|---|---|---|---|
| `toast_on_degraded` | bool | `true` | — | 即时：关闭后仅静默“降级/未确认”提示；**失败始终提示**（不允许完全静默） |
| `auto_dismiss_invalid` | bool | `false` | — | 即时：跳转判定目标失效（`target_invalid` / `task_gone` / `workspace_missing` / `rollout_missing` / `no_target` / `app_not_found`）后自动忽略该任务，并弹出可“撤销”的 toast |

### 数据 / 恢复

- 每个分组有“恢复本组默认值”，“数据”页有“全部恢复默认设置”（二次确认）。
- 只读展示数据目录、`settings.json`、`pet.json`、`dismissed.json` 路径，可“打开数据目录”。
- “清除已忽略任务”恢复被忽略的会话（含跨重启保留的记录）。
- 没有账号、上传、遥测、云同步。

### 内部字段

| key | 说明 |
|---|---|
| `version` | 设置格式版本，当前 `1` |
| `legacy_imported` | v0.3.x `localStorage`（`am-lang`、`am-s.pollMs`）一次性迁移标记 |

## 命令 / 事件

| 命令 | 参数 | 返回 | 说明 |
|---|---|---|---|
| `get_settings` | — | Settings | 设置窗口/桌宠启动时读取 |
| `set_settings` | `settings: Settings` | Settings（已夹取） | 全量保存并应用；服务端再次 clamp |
| `reset_settings` | `scope: all\|general\|monitor\|pet\|jump` | Settings | 分组恢复默认 |
| `get_settings_meta` | — | 见下 | 数据页只读信息 |
| `clear_dismissed` | — | number | 清除已忽略任务数量 |
| `get_custom_icons` | — | `{collapsed, hidden}` | 自定义图标 data URL；见 [`docs/ICONS.md`](ICONS.md) |
| `save_custom_icon` | `slot`, `dataUrl` | `{slot,mime,bytes,updatedAt}` | 校验并原子保存；广播 `icons-changed` |
| `clear_custom_icon` | `slot` | `()` | 删除槽位并广播 `icons-changed` |
| `restore_dismissed_task` | `taskId` | bool | 撤销单条忽略（自动清理的 Undo） |
| `import_legacy_settings` | `lang?`, `pollMs?` | Settings \| null | 一次性迁移，已迁移返回 null |
| `open_settings` / `close_settings` | — | — | 单实例显示/隐藏设置窗口 |
| `open_data_dir` | — | `Result<(), String>` | Finder 打开数据目录 |
| `reset_pet_position_command` | — | — | 复用桌宠 reset |

事件 `settings-changed` payload 为完整 `Settings`，广播到全部窗口；`pet-state` payload
中的 `settings` 也内嵌完整设置，保证首次渲染不闪默认值。

`get_settings_meta`:

```jsonc
{
  "data_dir": "…/Library/Application Support/agent-monitor",
  "settings_path": "…/settings.json",
  "pet_path": "…/pet.json",
  "dismissed_path": "…/dismissed.json",
  "version": 1,
  "app_version": "0.4.1",
  "system_language": "zh",
  "legacy_imported": true,
  "corrupt_backup": null
}
```

## 校验与容错

* 载入与保存都执行 `normalize()`：数值 clamp / 就近吸附到合法选项；
* 缺字段 → 默认值；未知字段 → 忽略（向前向后兼容）；
* 非法 JSON → 备份为 `.bak` + 默认值 + 应用继续启动；
* 自定义图标单独校验：仅按文件头接受 PNG/JPEG/WebP、解码后 ≤2 MB、原子写入，非法载荷拒绝；
* 写失败只记录日志，不阻塞 UI；下一次修改会重试；
* `--dump-tasks` / `--dump-agents` / `--jump` 会读取 `settings.json`（Agent 开关与保留期），
  但不加载 `dismissed.json`，所以可能显示 GUI 里已忽略的任务（无头验收需要看到原始采集结果）。

## 迁移

| 旧数据 | 新方案 |
|---|---|
| `localStorage['am-lang']` (`en`/`zh`) | 首次启动由桌宠 webview 读取并 `import_legacy_settings`，映射到 `language`；之后清空旧 key |
| `localStorage['am-s'].pollMs` (3000/5000/10000) | 映射到 `refresh_interval` (`3s`/`5s`/`10s`)；实测本机旧数据只有 `am-lang=zh`，因此迁移后 `language:"zh"`、`refresh_interval:"auto"`、`legacy_imported:true` |
| 旧窗口高度 s/m/l | 映射到 `max_visible_rows` 3/5/8（旧版是像素预设，本版按行数建模） |
| `pet.json` 的 position/hidden | 原样保留；旧 `hover_expand_ms`/`collapse_delay_ms`/`max_visible_rows` 字段被忽略，值改用 `settings.json` 默认 |
| 旧 `opacity` / `clickThrough` | 不迁移：窗口整体 alpha 会破坏 NSVisualEffectView vibrancy；点击穿透改为 P1 独立开关（默认关闭） |

## P1 状态（不在 P0 交付范围）

| 项 | 状态 | 理由 |
|---|---|---|
| 收起态点击穿透（含托盘安全恢复） | 未实现 | 需额外的托盘/快捷键恢复路径与无头测试，P0 之后再做 |
| 材质浓度（仅 CSS tint） | 未实现 | 只调 tint，不做窗口 alpha；先验证 P0 视觉与性能 |
| 登录时启动 | 未实现 | 需要 LaunchAgent/autostart 与卸载路径，默认关闭 |
| 全局快捷键 | 未实现 | 需新依赖 `tauri-plugin-global-shortcut`，注册失败提示与权限路径需单独验收 |
| 诊断导出 | 未实现 | 需脱敏快照格式与字段白名单 |
| 按 Agent 单独配置 | 未实现 | 会扩大设置模型为矩阵，P2 |

## 验证

```bash
source ~/.cargo/env
cargo test --release --manifest-path src-tauri/Cargo.toml      # 含 settings/pet/monitor 单测
cargo build --release --manifest-path src-tauri/Cargo.toml
bash scripts/ui-test.sh              # 桌宠（69 项）+ 设置窗口（45 项），聚合退出码
bash scripts/ui-test-settings.sh     # 只跑设置窗口
src-tauri/target/release/agent-monitor --dump-tasks   # 打印生效的 enabled/retention/rows
AGENT_MONITOR_DEBUG_GEOMETRY=1 src-tauri/target/release/agent-monitor
swift scripts/wins-probe.swift <pid>  # 真实窗口 bounds + activationPolicy
```

调试辅助（仅验收用，正常启动不设置）：`AGENT_MONITOR_START_MODE=expanded|edge_hidden|tray_hidden|collapsed`、
`AGENT_MONITOR_OPEN_SETTINGS=1`（启动即打开设置窗口，便于探测窗口与 accessory 状态）、
`AGENT_MONITOR_DEBUG_GEOMETRY=1`、`AGENT_MONITOR_DEBUG_TRAY=1`。

设置文件损坏回退验证：

```bash
cp ~/Library/Application\ Support/agent-monitor/settings.json /tmp/settings.ok.json
printf '{broken' > ~/Library/Application\ Support/agent-monitor/settings.json
src-tauri/target/release/agent-monitor --dump-tasks   # 仍输出快照；生成 settings.json.bak
mv /tmp/settings.ok.json ~/Library/Application\ Support/agent-monitor/settings.json
```
