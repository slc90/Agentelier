# 持续 Rhai Runtime

## 目标与产出

在 Agentelier 进程内嵌入 Rhai，建立一个持续存在的 Computer Use 脚本执行环境。模型能够提交 Rhai 脚本，在一次脚本中使用变量、函数、条件和循环组织多步桌面任务，并在同一 Computer Use 会话的多次工具调用之间保留脚本上下文。

本方案只建立 Computer Use 所需的一个 Rhai Runtime，不建立 Application Runtime、通用 Code Mode、Python/Node REPL 或脚本插件系统。

## Runtime 与上下文

Rhai Engine 在 Computer Use 使用期间持续存在。脚本上下文保存模型后续调用需要复用的变量和函数定义，避免每次 Function Calling 都从空上下文开始。

持续上下文与桌面状态分开：

- Rhai 上下文保存脚本定义和变量。
- Windows Driver 持有桌面操作能力，并在每次调用时读取真实窗口和屏幕状态。
- 窗口焦点、应用内容和坐标不会因为 Rhai 变量仍然存在就被视为未变化。

脚本上下文应关联到 Agentelier 的会话身份，确保同一会话的连续调用可以复用状态，不同会话不会意外共享变量。上下文的具体创建、恢复和销毁时机应与现有 Conversation/Session 生命周期一致，不另建会话管理体系。

## 脚本执行

一次脚本调用可以组合多步操作，例如：

1. 读取当前窗口列表。
2. 选择并激活目标窗口。
3. 获取截图。
4. 根据已知坐标执行输入。
5. 等待界面响应。
6. 再次获取截图并返回。

脚本支持变量、函数、循环和条件判断。模型可以把重复的鼠标操作、等待和坐标计算放在一次调用内，减少模型与工具之间的往返。脚本执行完成后，返回文本结果以及可选图片附件；模型根据结果决定是否发起下一次脚本调用。

本方案只交付脚本执行环境，不在这里决定 Computer API 的具体函数名称和参数。下一份方案负责将已实现的 Windows Driver 能力注册到 Rhai。

## 与 Tokio Runtime 的协作

Agentelier 的 Agent Runtime 使用 Tokio current-thread Runtime 管理任务。Rhai 脚本可能包含计算、等待和连续桌面操作，不能在 Tokio current-thread 上同步执行到长时间不返回，否则会阻塞其他 Agent 任务、取消消息和执行收尾。

Rhai 执行需要放在可管理的执行上下文中，并由现有 `agent/runtime` 管理任务句柄、取消和真实回收。具体采用哪种阻塞任务承载方式由实现时依据 Rhai 类型的线程约束和 Windows Driver 的调用方式确定；不能为了方便额外创建第二套 Agent Runtime。

取消需要能够停止后续脚本语句，并让正在进行的桌面输入进入收尾流程。脚本取消令牌已发出不等于执行已经退出；上层必须等待实际执行结束和资源回收后才能报告终态。

## 执行结果

Rhai 执行结果需要区分正常返回、脚本错误、取消和执行环境错误。错误信息应能帮助模型理解脚本在哪类操作上失败，但不把完整内部状态或原始 Windows 对象暴露给脚本。

图片不能被序列化为普通脚本字符串。Runtime 应允许 Computer API 将截图作为独立图片附件交给外层执行器，文本结果和图片数据分别表达。

## 与现有工程的关系

Rhai Runtime 属于 `agent/tools` 的 Computer Use 实现。现有 `agent/runtime` 继续管理 Tokio 任务、取消和回收；Widgetry JobManager 不承接 Agent 工具执行。Rhai 不放入 UI crate，也不依赖 Bevy。

Rhai 作为当前明确需求的外部依赖，版本统一由根 `Cargo.toml` 的 `[workspace.dependencies]` 管理。只启用本次需要的功能，不增加未来用途的扩展层。

## 完成条件与运行验证

- Windows x64 构建成功，Rhai 在 Agentelier 进程内运行，不启动 Python 或 Node.js。
- 多次脚本执行能够在同一会话内复用变量和函数。
- 不同会话之间不会意外共享脚本上下文。
- 单次脚本能够使用变量、函数、条件和循环组合多步操作。
- 脚本执行期间 Tokio Runtime 仍能处理其他任务和取消命令。
- 取消脚本后不会继续执行后续语句，执行任务真实退出并回收。
- 脚本错误能够返回可读错误，不破坏后续脚本上下文。
- 通过实际运行应用验证，不编写测试代码、Benchmark 或 Example。

## 与下一方案的关系

本方案提供持续 Rhai 执行环境。下一方案把窗口、截图、鼠标、键盘、等待和轨迹能力注册为 Computer API，使模型生成的 Rhai 脚本能够实际操作桌面。
