# 移除 Project Skill 并收敛 Skill 来源

## 目标

将 Skill 来源收敛为用户目录 Skill 与唯一内置 `skill-creator`，彻底移除项目 `.agents/skills/` 的发现和使用。

## 来源规则

最终只保留用户 Skill（应用数据目录下 `skills/`）和内置 Skill（`skill-creator`）。

当前 `standard_roots` 会根据 Session 工作目录向上发现项目 `.agents/skills/`，并合并用户与内置目录。本增量移除项目目录链和 `Source::Project` 语义。项目中已有 `.agents/skills/` 文件保留在原位置，不由 Agentelier 删除或修改。

Skill 发现继续使用渐进式披露：目录阶段只读取名称、描述等有界元数据；完整 `SKILL.md`、references 和其他资源在实际使用时按需读取，不因 Skill 数量增加就把所有正文加入上下文。

## Agent 影响

主要修改 `crates/agent/skills` 的 Root、Source、Catalog、发现和诊断模型。Session 的 Skill 快照只包含用户 Skill 与内置 Skill。`project_context` 继续独立按 Session 工作目录发现 `AGENTS.md`，不承担 Skill 发现。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
## 完成条件与验收

1. Skill Catalog 只包含用户 `skills/` 与内置 `skill-creator`。
2. 项目 `.agents/skills/` 不再被扫描、合并或作为 Skill 来源。
3. 项目中已有 `.agents/skills/` 文件保持原样，不被删除、移动或覆盖。
4. 用户 Skill 仍能发现、解析元数据、选择并按需读取正文。
5. 内置 `skill-creator` 仍然存在，并作为只读内置来源。
6. 用户 Skill 与内置 Skill 的同名冲突按明确规则报告，不发生静默覆盖。
7. Skill 目录数量增加时，只增加必要的目录元数据；未选择 Skill 的正文和 references 不进入模型上下文。
8. Session 恢复和历史任务读取不再依赖项目 Skill 来源。
9. `project_context` 仍按 Session `working_directory` 发现 `AGENTS.md`，不因 Project 归属改变。
10. 搜索 `crates/agent/skills` 和资源引用，确认 Project Root 构造、发现、合并和来源诊断已移除。
11. 在真实项目目录保留 `.agents/skills/` 的情况下运行应用，确认它不出现在 Skill Catalog，文件内容保持不变。
