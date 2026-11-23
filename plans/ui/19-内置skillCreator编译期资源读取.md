# 让内置 skill-creator 直接读取编译期嵌入资源

## 目标

移除 `skill-creator` 的运行时目录导出。内置 `SKILL.md`、references、图标和许可材料直接从编译期嵌入资源读取。

## 当前资源模型与变化

当前 `crates/agent/skills/src/builtin.rs` 已使用 `include_bytes!` 将资源编译进程序，但 `root()` 会计算内容哈希并创建 `%LOCALAPPDATA%/Agentelier/skills/.system/<hash>/skill-creator/`，`install()` 再把嵌入文件导出到磁盘并验证只读状态。

新模型保留编译期嵌入，但移除 `root()`、`install()`、磁盘验证和运行时导出目录。内置资源使用明确的内存资源身份和相对资源路径；用户 Skill 继续使用文件系统路径。两种来源共享 Catalog、元数据和按需读取语义，但不再强制内置资源伪装成 `PathBuf`。

模型读取内置 Skill 正文和 references 时，通过 Agent Skills 提供的受控资源读取能力完成。读取能力只允许访问当前内置 Skill 的嵌入资源，路径相对于内置资源根目录解析，不向模型提供一个不存在的磁盘路径。

## Agent 影响

- `crates/agent/skills`：增加嵌入资源引用和统一资源读取入口，调整 `Skill` / `SelectedSkill` 的来源表示。
- `crates/agent/tools` 或现有 Extension Tool 注册边界：提供读取内置资源的受控能力，沿用现有工具执行器。
- `crates/agent/turn_runner`：在 Skill 选择与资源读取时保留资源来源身份。
- `crates/agent/session_store` / `conversation`：历史任务快照不再依赖 `.system/<hash>` 导出路径。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
## 完成条件与验收

1. 内置 `skill-creator` 的 `SKILL.md` 与 references 仍通过 `include_bytes!` 或等价编译期嵌入进入程序。
2. 运行应用、发现 Skill、选择内置 Skill 和读取正文时，不创建 `.system/<hash>` 或其他运行时导出目录。
3. 内置资源可以通过 Agent 侧受控读取能力按相对路径读取，内容与编译期资源一致。
4. references 可按需读取；不存在、越界或路径穿越的资源请求被拒绝。
5. 模型不再收到内置 Skill 的虚构磁盘路径，也不需要通过 `exec_command` 读取内置资源。
6. 用户 Skill 仍从应用数据目录读取真实文件；用户目录与内置资源的读取边界清晰。
7. Skill Catalog 和显式选择快照能区分资源来源；Session 恢复后内置资源不依赖磁盘缓存。
8. 运行时不会为了验证嵌入资源而重新导出到 `%LOCALAPPDATA%`。
9. 搜索代码和资源清单，确认 `root()` / `install()` 的运行时导出路径和脚本资源引用已移除。
10. 保留上游来源说明与 Apache-2.0 许可。
11. 在真实应用中读取内置 SKILL.md 与至少一个 reference，并检查数据目录没有新增导出缓存。
