# Function Calling 工具接入

## 目标与产出

将持续 Rhai Runtime 和 Computer API 接入 Agentelier 现有固定工具目录，让模型能够通过普通 Responses Function Calling 提交 Rhai 脚本并取得执行结果。完成后，模型可以在现有 Turn Runner 循环中调用 Computer Use，不需要新的模型请求循环或独立 Agent Runtime。

本方案依赖[持续 Rhai Runtime](06-persistent-rhai-runtime.md)、[Rhai Computer API](07-rhai-computer-api.md)和[桌面独占与执行生命周期](08-desktop-session-lifecycle.md)。

## 工具声明与参数

在 `agent/tools` 固定工具目录中登记一个内置 Function Tool，名称使用 `computer_execute`。工具参数包含模型提交的 Rhai 脚本字符串。工具描述需要说明这是持续脚本上下文中的 Computer Use 执行入口，脚本可调用哪些类别的 Computer API，以及如何通过结果判断执行成功或失败。

工具声明沿用当前固定目录和 strict Function Calling 规则。参数 schema 应明确要求脚本字段并关闭额外属性；执行侧仍需检查参数类型和脚本内容是否存在，不能把模型输出通过 JSON 解析视为脚本已经合法。

本工具不按鼠标、键盘、截图等底层动作拆成多个 Function Tool。模型一次调用提交一段脚本，由 Rhai 在本地组合多个操作，减少每个动作都往返模型的开销。

## 执行派发

工具执行器从当前工具参数中取得脚本，使用当前任务的执行上下文调用 Rhai Runtime。Runtime 使用持续的 Computer Use 会话上下文执行脚本，并通过 Computer API 操作 Windows Driver。

工具调用沿用现有 `call_id`、工具名称和参数身份。Turn Runner 仍负责完整模型响应接纳、工具派发、结果关联和下一次模型请求。工具执行器不直接发起模型请求，也不自行推进 Agent 循环。

脚本执行结果先以文本摘要形式返回。成功、脚本错误、取消和底层操作错误需要有清晰的结果类别，避免把脚本异常伪装成成功。当前阶段先完成文本结果闭环；图片附件反馈在下一份方案中接入。

## 独占访问与任务收尾

Computer Use 会改变同一真实桌面的焦点、鼠标和键盘状态，工具注册必须使用现有 `ToolAdmission::Exclusive`。不得将它标记为 `SharedRead`。独占许可覆盖整个脚本执行和输入状态收尾，直到 Rhai 任务真实退出后才释放。

用户 Stop、任务取消和应用关闭沿现有 Runtime 取消与回收路径传递。取消后 Rhai 不再执行后续脚本语句；Driver 完成鼠标按钮和键盘按键收尾；Runtime 等待任务真实退出，再释放许可并报告终态。不能仅因取消令牌已发出就假定任务已回收。

## 与现有工程的关系

- `agent/tools`：登记 `computer_execute`，解析脚本参数，调用 Rhai Runtime，封装文本结果。
- `agent/turn_runner`：沿现有 Function Calling 循环派发工具并按 `call_id` 关联结果。
- `agent/runtime`：继续管理 Tokio 任务、取消令牌和 JoinHandle 回收。
- `agent/conversation`：继续保存原始 Function Call 与 Function Call Output。
- `ToolAdmission`：复用现有 Exclusive 访问语义。
- `Widgetry JobManager`：不参与 Agent 工具执行。

当前 `ToolRegistration::new/with_origin` 默认采用 Exclusive，并默认允许图片结果；本方案仍应明确确认注册行为与期望一致。工具执行器持有 ToolPermit，不能在脚本仍可能访问桌面时提前丢弃许可。

## 完成条件与运行验证

- 当前 OAuth + Codex Responses WebSocket 路径能够声明普通 `computer_execute` Function Tool。
- 模型返回的 `function_call` 能被现有协议层识别并派发。
- 工具执行器能读取脚本参数并调用持续 Rhai Runtime。
- Rhai 脚本能够通过 Computer API 完成至少一组真实桌面操作。
- 工具结果与原始 `call_id` 正确关联，模型能够读取结果并继续回答或发起下一次调用。
- 连续多次调用复用同一 Computer Use 会话的脚本上下文。
- 同一桌面不会被两个并发 Computer Use 调用同时控制。
- Stop 和应用关闭能够取消脚本、完成输入收尾、等待实际回收并释放独占许可。
- 通过实际 Agentelier、当前 OAuth 会话和真实 Windows 应用验证，不编写测试代码、Benchmark 或 Example。

## 与下一方案的关系

本方案完成模型到 Rhai 的 Function Calling 执行闭环，并返回文本结果。下一方案把截图作为图片附件反馈给模型，再通过 Krita 绘画复刻检验视觉观察、连续笔画和多轮修正能力。
