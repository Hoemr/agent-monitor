# Bundled pet assets — provenance & licenses

| Pack | Kind | Source | License |
|---|---|---|---|
| `british-blue`（皮蛋） | state | [Familiar](https://github.com/Monster12138/familiar) · Familiar Studio | MIT OR Apache-2.0（随包 `pack.json.license`） |
| `nailong`（奶龙）, `doraemon`（哆啦 A 梦）, `tihun`（啼魂） | state | 内置 image_gen 参照现有三只猫生成的精细像素角色同人图，提示词见 [PROMPTS.md](../output/pet-art/PROMPTS.md) | 角色权利归各自权利方；非官方素材，不沿用猫咪包的开源许可 |
| `agent-cat`, `byte-slime`（旧版，不再列入选择面板） | shimeji | 本仓库 `scripts/make-pets.py` 生成 | 与本仓库一致 |
| `huajuan-cat`（花卷）, `douhua-cat`（豆花） | state | [Familiar](https://github.com/Monster12138/familiar) | MIT OR Apache-2.0 |

- Familiar 的猫以**用户数据目录包**形式安装
  （`~/Library/Application Support/agent-monitor/pets/<name>/`，pack.json 保留
  原始作者/许可/来源字段），`british-blue` 另有一份嵌入二进制作为内置默认。
- 状态图 PNG 为 2 帧 APNG（A/B 帧摆动），WebView 的 `<img>` 会自动播放。
- 三个新角色随二进制内嵌，每个有八种 512 × 512 透明状态图、统一基线、128 px 原生像素网格及逐像素呼吸动画；原始图保留在 `output/pet-art/`，由 `scripts/prepare-character-pets.swift` 切分。啼魂采用动画版绿毛蓝脸的小猴形态。
- 更新素材：替换对应目录的 PNG 后重启即可；`sha256` 字段仅 Familiar
  官方分发校验用，本地修改无需更新。
