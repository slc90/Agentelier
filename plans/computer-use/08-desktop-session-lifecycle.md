# Computer Use 桌面独占与执行生命周期

## 目标与产出

让 Computer Use 在 Agentelier 现有工具执行体系中正确管理桌面控制权和输入状态。完成后，同一时刻只有一个已准入的 Computer Use 执行者能够操作当前 Windows 桌面；脚本结束、取消或应用关闭时，输入状态完成收尾，任务真实回收后才释放桌面独占许可。

本方案依赖持续 Rhai Runtime 和 Computer API。

## 桌面是独占资源

Windows 桌面是多个 Agent 和工具可能共同访问的外部状态。两个执行者同时注入鼠标和键盘，会使输入顺序、焦点和截图结果失去确定性。

Computer Use 工具使用现有 `ToolAdmission::Exclusive`。当前 `ToolRegistration::new/with_origin` 默认采用 Exclusive；实现时应明确保持该访问模式，不将 Computer Use 标记为 `SharedRead`。同一时刻只有一个 Computer Use 执行持有独占许可。

本方案不另建桌面锁管理器、排队系统或调度框架。工具准入仍由现有 ToolAdmission 决定，桌面 Driver 不自行创建第二套并发控制。

## 独占许可的持有范围

独占许可必须覆盖一次完整 Rhai 脚本执行，包括脚本内部的等待、截图、鼠标和键盘操作，以及执行结束后的输入状态收尾。

不能在脚本刚返回结果、但鼠标按钮仍处于按下状态时提前释放许可。也不能在上层只发出取消令牌后就释放许可，因为底层脚本可能仍在执行。

当工具执行器返回最终 ToolResult 时，脚本必须已经退出，Driver 已完成本次输入收尾，执行任务已经真实回收。许可随后随现有 ToolPermit 生命周期释放。

## 取消与输入收尾

取消可能来自用户 Stop、任务终止或应用关闭。取消流程需要贯穿 Runtime、Rhai 执行和 Windows Driver：

1. 现有 Runtime 发出任务取消信号。
2. Rhai 执行停止后续脚本语句。
3. 正在进行的 Driver 操作结束或退出其可取消边界。
4. Driver 释放本次执行按下但尚未释放的鼠标按钮和键盘按键。
5. Rhai 执行上下文退出当前调用。
6. Runtime 等待任务真实退出并回收。
7. 工具执行结束，释放桌面独占许可并报告终态。

Driver 的输入状态清理只处理本次执行持有的输入，不盲目释放用户原本持有的物理输入状态。若底层同步 Windows 调用无法立即响应取消，Runtime 必须等待实际调用退出，不能将“取消请求已发出”当作“任务已回收”。

## 与现有 Agentelier Runtime 的关系

`agent/runtime` 继续持有 Tokio 任务、取消令牌和 JoinHandle，并负责真实回收。`agent/tools` 的 Computer Use 执行器持有 ToolPermit，并在脚本及输入收尾结束后归还结果。

Widgetry JobManager 不参与 Agent 工具执行。UI 不管理桌面锁，也不直接释放输入状态。应用关闭沿用现有 Runtime 关闭流程取消和回收任务。

## 完成条件与运行验证

- 两个并发 Computer Use 请求不能同时操作同一桌面。
- 第一个脚本执行期间，第二个请求按现有 ToolAdmission 语义等待或被拒绝，不会注入鼠标键盘。
- 脚本正常结束后，鼠标按钮和键盘按键均处于释放状态，桌面独占许可释放。
- 在鼠标拖动或连续轨迹执行中取消，后续轨迹停止，按钮释放后任务才报告终态。
- 在组合键执行中取消，修饰键释放后任务才报告终态。
- 取消后 Runtime 等待真实执行退出，不因仅发出令牌就提前报告回收。
- 应用关闭时 Computer Use 任务被取消并回收，不留下后台桌面输入。
- 通过真实应用和多任务运行验证，不编写测试代码、Benchmark 或 Example。

## 与下一方案的关系

本方案完成桌面操作的准入和收尾边界。下一方案将 `computer_execute` 注册到固定工具目录，通过现有 Function Calling 让模型调用，并执行最终 Krita 绘画验收。
