# 交付用户 Skill 列表、导入与删除窗口

## 目标

完成 B 区 Skill 按钮打开的管理窗口，展示用户 Skill 与内置 `skill-creator`，提供添加和删除操作。

## 窗口行为

Skill 窗口只显示两类来源：

- 用户 Skill：来自应用数据目录 `skills/`，可以添加和删除。
- 内置 Skill：只有 `skill-creator`，显示为只读内置项，不允许删除。

添加操作通过 FileDialog 选择一个已有 Skill 文件夹，验证 `SKILL.md`、frontmatter 和资源结构后导入到用户 Skill 根目录。目标名称冲突时拒绝覆盖。

删除操作只删除用户 Skill 根目录下对应目录，不允许通过链接或路径穿越删除目录外文件。窗口不提供 Skill 正文编辑器、启用开关或商店。用户 Skill 内容由外部编辑器或聊天中的 skill-creator 工作流维护。

## Agent / Host 影响

`crates/agent/skills` 是 Skill Catalog、来源和目录操作的权威。添加与删除通过 Agent 侧受控操作完成：添加时校验并导入目录，删除时确认目标位于用户 Skill 根目录内。UI 不直接扫描文件系统后维护第二份 Catalog，也不直接删除任意路径。

`agent_host` 负责转发 Skill 管理请求和 Catalog 更新事件。若现有 AgentHost 能力已足够，沿用现有消息入口，不建立专用 Bridge Crate。

## Widgetry 复用

Window、ListView、ScrollArea、FileDialog、MessageBox、AgentelierButton 和只读 Text 展示。Skill Catalog 由 `agent/skills` 提供，UI 不自行扫描并建立第二份目录。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
## 完成条件与验收

1. 点击 B 区 Skill 按钮打开管理窗口。
2. 列表显示应用数据目录 `skills/` 下的用户 Skill 和内置 `skill-creator`。
3. 内置 Skill 标记为只读，不显示删除入口。
4. 用户 Skill 显示名称和描述；没有正文编辑器、启用开关或 Skill 商店。
5. 点击添加打开目录选择器；取消选择不改变 Catalog。
6. 有效 Skill 导入到用户 `skills/`，完成后列表更新。
7. 缺少 `SKILL.md`、frontmatter 无效、目录名非法或资源结构无效时拒绝导入并显示原因。
8. 目标名称已存在时拒绝覆盖，不产生半份导入目录。
9. 删除用户 Skill 前显示确认；取消确认不修改目录。
10. 删除后只移除对应用户 Skill；目录外文件、其他 Skill 和内置 Skill 保持不变。
11. 路径穿越、异常目录和链接不能使删除越出用户 Skill 根目录。
12. 项目 `.agents/skills/` 即使存在也不会出现在列表中。
13. 外部编辑器或助手修改用户 Skill 后，重新打开窗口能读取最新 Catalog。
14. 用户 Skill 数量增加时列表可滚动，不把所有正文载入 UI 或模型上下文。
15. 通过 BRP 验收目录选择、添加、删除、取消、确认和错误状态；检查应用数据目录真实结果。
