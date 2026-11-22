# 建立会话级 UIA MTA Runtime 与 COM 生命周期

## 目标与产出

在 `agent/tools` 中建立独立的 UIA Runtime，使 UIA COM 对象始终由固定 MTA Worker 创建、使用和释放。Runtime 与现有 `ScriptSession` 同生命周期，跨同一会话的多次 `computer_execute` 调用复用，不跨会话共享。

## Runtime 归属

当前 `runtime/tool_jobs.rs` 的会话工具工作者在 `Execution` 中创建 `ScriptSession::default()`，并在工具调用之间持续持有。UIA Runtime 作为 ScriptSession 的可选内部状态，第一次执行 UIA API 时延迟启动。

- 每个 ScriptSession 最多拥有一个 UIA Worker。
- Worker 生命周期覆盖该 ScriptSession 的连续工具调用。
- Rhai 的 Engine、Scope 和 AST 仍由现有 ScriptSession 管理。
- Rhai 只持有可发送的 UIA Facade，不持有任何 COM wrapper。
- 元素引用表、订阅表和快照缓存由 Worker 持有。

## COM 初始化

`uiautomation::UIAutomation::new()` 会自行调用 `CoInitializeEx` 并请求 MTA；`new_direct()` 不初始化 COM。当前 wrapper 没有负责 `CoUninitialize` 的 Drop 实现。

Worker 在线程启动时显式调用 `CoInitializeEx(COINIT_MULTITHREADED)`，成功后使用 `UIAutomation::new_direct()`。线程退出时在同一线程释放 UIA 对象并配对 `CoUninitialize`。如果初始化返回不兼容 apartment 错误，Runtime 启动失败并返回明确错误。

## 请求模型

UIA Facade 通过有界请求通道向 Worker 发送普通数据请求。请求包含操作类型、目标 WindowId/ElementRef、参数、取消状态和截止时间。Worker 串行执行 COM 调用，返回普通 DTO。

队列满时返回 `uia_queue_full`。取消可阻止尚未开始的请求；正在执行的 Provider COM 调用返回后再检查取消，不能假设任意 COM 调用都能被强制中断。

## ScriptSession 与 ToolAdmission

`ScriptSession::run` 将 Rhai 执行放在 `spawn_blocking` 中，不保证不同次调用使用同一线程。UIA Worker 与 Rhai 阻塞线程必须分离。

`computer_execute` 仍由现有 `ToolAdmission::Exclusive` 保护。UIA 查询、Pattern 和等待都属于同一脚本执行期间的桌面操作，不增加独立锁。事件监听可以在工具调用结束后继续被动接收通知，但不持有 ToolPermit。

## 关闭

会话工具工作者 Cleanup 时，先停止新调用、取消在途调用并等待工具 JoinSet 清空；随后调用 ScriptSession 的显式 UIA 关闭入口。Worker 停止接收请求、移除事件、清空引用与缓存、释放 COM 对象、调用 `CoUninitialize` 并退出。关闭完成前不能报告资源已回收。

## 范围

包含 UIA Runtime、MTA Worker、请求通道、会话级所有权、COM 初始化/退出、取消边界和显式关闭。窗口根元素和具体 Pattern 在后续方案接入。

## 验收

- 首次 UIA 请求能延迟启动一个 MTA Worker。
- Worker 内创建 `UIAutomation::new_direct()`，不在 Rhai `spawn_blocking` 线程创建 COM 对象。
- 所有 UIA COM wrapper 仅由 Worker 使用和释放。
- 同一 ScriptSession 的连续工具调用复用同一个 Worker。
- 不同 ScriptSession 不共享元素引用和订阅。
- 队列有界，满载返回明确错误。
- 排队请求可取消；在途 COM 调用返回后不再继续后续脚本操作。
- Session Cleanup 等待 Worker 清理和线程退出。
- 不使用 Unit Test、Integration Test、Benchmark 或 Example；通过真实 Agentelier 运行、日志和调试信息验证。
