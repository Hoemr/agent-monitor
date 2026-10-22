# 自定义图标框架（Custom icon framework）

Agent Monitor 的收起态与边缘隐藏态不再只能是内置的三条杠图案：只要按下面的规格
制作图片并在 **设置 → 桌宠 → 图标** 里上传，就能换成任意图案。

## 槽位（slots）

| 槽位 | 用于 | 显示尺寸 | 建议源图 |
|---|---|---|---|
| `collapsed` | 收起/吸附后的桌宠（默认 72 px，可在 48–96 px 调节） | 48–96 pt | **144×144 px** PNG（2×） |
| `hidden` | 贴边隐藏后的小方块标签（8–24 pt，默认 14 pt） | 8–24 pt | **64×64 px** PNG（2×），图案极简 |

`hidden` 槽位可留空：留空时该状态回退使用 `collapsed` 图标；两个都为空则使用内置图案。

## 图片要求

* **正方形**（宽高比 1:1）。非正方形会按 `object-fit: contain` 居中缩放，四周留白。
* 格式：**PNG / JPEG / WebP**（按文件头真实识别，不信任扩展名）。
* 大小：**≤ 2 MB**（解码后）。
* 背景：**透明** 效果最好（窗口本身是圆角/材质，图像不要自带底色方块）。
* 安全边距：四周留约 **8%** 空白，避免贴边被圆角裁切。
* 深浅色：图像会同时出现在浅色与深色外观下。单色 + 透明背景（如白色/黑色描边）兼容性最好；
  若用彩色图案，请保证两种外观下都清晰。
* 不要在图内画文字或极细线条：`hidden` 态只有十几像素，细节会糊。
* 圆角、阴影、悬停/聚焦高亮由应用负责，图片本身不需要带。

## 上传与生效

1. 打开 **设置 → 桌宠 → 图标**。
2. `图标样式` 选 **自定义（Custom）**（选“内置”则始终显示内置图案）。
3. 分别上传 `收起态图标`、`边缘隐藏标签图标`（后者可选）；可随时 **替换** 或 **移除**。
4. 保存即时生效：桌宠窗口通过 `icons-changed` 事件立即重绘，无需重启。

## 存储与安全

* 文件位置：`~/Library/Application Support/agent-monitor/icons/{collapsed,hidden}.img`
  （目录可用 `AGENT_MONITOR_DATA_DIR` 覆盖）。
* 图片**不写入** `settings.json`，设置文件只保存 `icon_style`。
* 校验在 Rust 侧完成：文件头嗅探（仅 PNG/JPEG/WebP）+ 2 MB 上限 + 原子写入
  （`*.img.tmp` → rename）；非法载荷直接拒绝，错误会显示在设置页保存栏。
* 移除：设置页 **移除** 按钮，或直接删除对应文件。
* 本地文件、无上传、无网络请求。

## 命令 / 事件契约

| 命令 | 参数 | 返回 |
|---|---|---|
| `get_custom_icons` | — | `{ collapsed: dataUrl \| null, hidden: dataUrl \| null }` |
| `save_custom_icon` | `slot: "collapsed" \| "hidden"`, `dataUrl: string` | `{ slot, mime, bytes, updated_at }` 或错误字符串（前端同时兼容 `updatedAt`） |
| `clear_custom_icon` | `slot` | `()` 或错误字符串 |

事件 `icons-changed` 携带与 `get_custom_icons` 相同的结构，广播给桌宠与设置窗口。

## 扩展新的槽位

框架是通用的：在 `src-tauri/src/icons.rs` 的 `SLOTS` 增加槽位名、在设置页增加一个上传卡片、
在桌宠侧决定何时使用该槽位即可；存储、校验、事件、预览都不需要改。
