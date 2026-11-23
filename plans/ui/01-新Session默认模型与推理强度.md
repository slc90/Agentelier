# 为新 Session 注入默认模型与推理强度

## 目标

将默认模型和默认推理强度纳入应用配置，并在创建新 Session 时写入其固定 `SessionSettings`。默认值分别为 `gpt-6.2-sol` 和 `high`。

## 设计

当前 `AppConfig` 已有 UI Scale、数据目录和 BRP 配置，并由 `ConfigStore` 统一保存。新增两个有默认值的字段：

- `default_model_slug = "gpt-6.2-sol"`
- `default_reasoning_effort = "high"`

旧版 `config/config.ron` 缺少字段时，反序列化使用默认值，不要求用户删除配置文件。配置仍通过现有 `ConfigStore` 的互斥更新、临时文件写入、同步和替换机制保存；完整配置保存成功后才更新内存状态。

创建新 Session 时，将这两个默认值传入 `agentelier_conversation::SessionSettings`。模型目录和推理等级使用 `agentelier_llm_model` 当前能力信息校验。已有 Session 的模型、推理强度、上下文限制和工作目录仍使用其存档快照；全局默认值变化不回写已有 Session。

D 区模型选择修改的是当前 Session 的模型设置，不修改全局默认值。推理强度本阶段不放入聊天输入区，`high` 作为新 Session 的默认设置。

## Agent 影响

- `crates/ui/application`：扩展 `AppConfig` 并暴露已提交的默认值。
- `crates/agent/conversation`：创建 Session 时接收并保存默认 `slug` / `effort`。
- `crates/agent/llm_model`：验证模型是否存在，以及是否支持 `high`。
- `crates/bridge/conversation`：只在现有 Session 创建命令中传递必要字段，不增加 UI 专属状态。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
- 只有涉及可观察 GUI 行为时才执行 BRP；BRP 不等价的 OS-level 行为使用真实 Windows 窗口验证。
## 完成条件与验收

1. 没有配置文件时，默认模型为 `gpt-6.2-sol`，默认推理强度为 `high`。
2. 读取缺少这两个字段的旧配置时，使用默认值，现有数据目录、BRP 端口和其他字段保持不变。
3. 创建新 Session 后，其 `SessionSettings` 中的模型和推理强度分别为 `gpt-6.2-sol` 与 `high`。
4. 新 Session 的上下文限制仍来自实际模型目录，不因默认字段而硬编码。
5. 当前模型目录不包含 `gpt-6.2-sol` 时，创建流程返回明确模型不可用状态，不静默换成其他模型。
6. 模型存在但不支持 `high` 时，返回明确推理等级错误，不自动降级到其他档位。
7. 修改全局默认值后，已有 Session 重新打开仍恢复原模型与推理强度。
8. 当前 Session 在聊天区切换模型后，后续请求使用新 Session 设置；其他 Session 和全局默认值不变。
9. 配置写入失败时，内存配置不先更新；再次打开设置仍显示最后一次成功保存的值。
10. 连续更新默认模型和推理强度时，完整配置中的其他字段不被旧快照覆盖。
11. 在真实应用中保存配置、重启应用并创建 Session，确认默认值经过真实持久化和创建路径生效。
