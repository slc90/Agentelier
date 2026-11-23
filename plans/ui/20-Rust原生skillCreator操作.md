# 将 skill-creator 的确定性操作改为 Rust 原生能力

## 目标

移除 skill-creator 对 Python、PyYAML 和运行时脚本的依赖，由 Rust 完成目录初始化、元数据生成和结构验证；模型仍负责 Skill 内容设计与 Markdown 创作。

## 职责划分

模型负责理解用户需求、设计 Skill 工作流、撰写 `SKILL.md`、决定是否需要 references / scripts / assets，并根据反馈修改内容。

Rust 负责确定性操作：

- 名称规范化与合法性检查。
- 初始化 Skill 目录和必需模板。
- 按元数据生成 `agents/openai.yaml`。
- 验证 frontmatter、YAML、目录名称、文件结构和资源引用。
- 检查路径边界，避免路径穿越和意外覆盖。

将 `init_skill.py`、`generate_openai_yaml.py`、`quick_validate.py` 的职责迁移到 Agent Rust 原生能力。当前 Skills Crate 已使用 `serde_yaml`，优先复用现有解析能力，不引入 PyYAML 或脚本运行时。

初始化不得覆盖已有 Skill。更新现有 Skill 时，未被用户要求修改的 references、assets 和 scripts 保持不变。用户没有要求时不生成无用目录、占位脚本或 Example 程序。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
## 完成条件与验收

1. Rust 能规范化合法 Skill 名称，并拒绝空名称、非法字符、保留名称和路径穿越。
2. 初始化前检查目标是否存在；已有 Skill 不被静默覆盖。
3. 初始化生成符合当前规范的 `SKILL.md` frontmatter 与正文模板。
4. 用户未要求 references / scripts / assets 时，不创建无用目录或占位文件。
5. 用户明确要求资源目录时，Rust 创建对应目录，不生成 Python 脚本或 Example 程序。
6. `agents/openai.yaml` 能按支持字段正确生成；Unicode、换行、引号和特殊字符序列化正确。
7. YAML 缺字段、类型错误、重复字段或非法值时返回明确错误，不生成半份有效文件。
8. Skill 目录名与 frontmatter 名称不一致时验证失败并指出问题。
9. `SKILL.md` 缺少必需 frontmatter、名称或描述无效时验证失败。
10. 资源引用能在 Skill 根目录内解析；越界、路径穿越、无效路径或不存在的目标被拒绝。
11. 验证器检查当前规范要求的未完成占位标记，不把普通正文误判为 TODO。
12. 修改已有 Skill 时，未被用户要求修改的 references、assets 和 scripts 保持不变。
13. 写入失败、权限不足、目标并发创建或文件已变化时，不覆盖原文件，并返回可读错误。
14. 真实运行中不启动 Python 解释器、不调用 PyYAML。
15. 移除三个 Python 脚本及其运行依赖和引用。
16. 模型仍负责创作 `SKILL.md`；Rust 只完成初始化、生成和验证。
17. 在真实应用中创建新 Skill、修改已有 Skill、验证无效 Skill，并检查磁盘结构。
18. 保留上游来源说明与 Apache-2.0 许可。
