# 开放 Bevy Event 与 Message 派发

## 目标与产出

让 Agent 能通过 Rhai 触发已注册的 Bevy Event / Observer，并向已注册的 Message 通道写入数据。完成后，Agent 不必为了执行一个正常业务动作而直接改写某个控件的内部 Component；它可以调用应用已经定义的事件和消息机制。

本增量复用既有 EcsAccess 请求通道，不建立第二套事件总线，也不开放任意 Rust 函数调用。

## 两种派发语义

| API | 用途 |
| --- | --- |
| `ecs_trigger(type, payload)` | 构造并触发已注册的 Bevy Event |
| `ecs_write_message(type, payload)` | 构造并写入已注册的 Bevy Message |

Event 通过 Bevy 的正常 Observer 路径派发。请求返回时，同步触发过程已经完成；Observer 的错误或 Panic 仍经过对应 ECS 执行边界和现有 Bevy 错误处理。

Message 写入 Bevy 的消息缓冲区后返回。消费它的 System 仍在正常 Schedule 中运行，因此写入成功表示消息已提交，不表示所有消费者都已经完成。Agent 可以在后续 Native API 调用中读取实际状态。

## 通过显式适配器支持动态类型

Rhai 只能传递类型名称和可序列化 payload，不能直接构造任意 Rust Event 或 Message。UI 宿主维护一份明确注册的派发适配器；每个适配器负责：

- 提供稳定的类型名称和用途描述。
- 将 Rhai 参数转换为对应 Rust Event / Message。
- 校验必要字段和目标 Entity。
- 调用 Bevy 的正常 trigger 或 write_message 入口。
- 将执行结果或结构化错误返回 Rhai。

初始 Event 适配器可以包含 Widgetry 的 `Activate`，使 Agent 能对指定控件触发正常激活事件。应用侧也可以按实际需要注册公开业务事件，例如 Chat UI 的 `OpenSettings`。Message 适配器同样只开放应用明确登记的类型。

这种机制让 API 保持通用，但不把 Reflect 误当成任意 Rust 行为调用能力。没有适配器的 Event / Message 返回 `unsupported_event` 或 `unsupported_message`。

## 与控件行为的关系

Agent 不通过 OS 鼠标输入来激活控件。它可以查询目标控件的 Entity，再通过 `ecs_trigger("Activate", #{ entity: button })` 触发 Widgetry 使用的激活事件。控件关联的 Observer 按正常路径执行。

直接触发 Event 不等于模拟一次物理点击。它验证的是应用事件和 Observer 契约；Pointer、Focus 或真实键鼠输入行为仍按各自的 GUI 验证规则处理。

## 完成条件与运行验证

- `ecs_trigger` 能通过注册适配器构造并派发 Activate。
- 对一个已存在的 Agentelier 控件触发 Activate，确认对应 Observer / 业务事件执行。
- 注册一个当前应用已有的公开业务 Event（例如 OpenSettings），Agent 能通过类型名触发，并通过 ECS State 或 BRP Screenshot 确认实际界面变化。
- `ecs_write_message` 能向已注册 Message 写入 payload，并由正常 System 在后续 Schedule 消费。
- 未注册 Event / Message、缺少字段和无效 Entity 返回结构化错误，不导致应用崩溃。
- Event / Message 的正常执行仍在 Bevy 主线程；Rhai 不直接持有 World。
- 使用实际 Agentelier Function Calling 和 BRP 状态 / 截图完成 GUI 验证；不编写测试代码、Benchmark 或 Example。

## 保持的边界

- 只开放已注册的 Event / Message 适配器，不通过字符串调用任意 Rust 函数。
- 不把 Message 写入成功解释成消费者已经处理完成；Agent 需要查询后续实际状态。
- Event / Message 不模拟鼠标、键盘或 OS 焦点。
- 继续复用 EcsAccess、AgentHostPlugin 和现有 Bevy 错误处理；不引入 BRP、HTTP、MCP 或 UIA 运行时通道。
