# 实现 interrupt_agent

## 目标与交付边界

请求中断指定 Agent，并将取消请求与真实执行收尾区分。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不实现 Root Session 全树关闭。

## 设计与行为

目标解析、CancellationToken、状态变化、停止回执、后代协调、失败/未知终态。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

取消请求不等于已停止；容量只在资源回收后释放；不能误停兄弟/Root；存在后代时按父子关系协调。

## 验收场景与通过标准

中断一个正在运行的子 Agent，确认其 Runner 收到取消并在真实资源回收后进入 Interrupted；同时让兄弟 Agent 与 Root 继续工作；对已完成/已卸载目标执行中断，返回语义明确且不产生假停止事件；模拟工具进程仍未退出，确认状态保持 Stopping/Unknown 且容量不提前释放；中断拥有后代的 Agent，确认后代无孤儿活动 Runtime；重复中断请求幂等。

## 验证方式

检查 CancellationToken 触发、Join 完成、工具进程回收和 Store 状态事件；确认状态回执与真实资源一致。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 中断范围和收尾

中断请求要携带调用者身份、目标身份和操作 ID。Agent Control 验证目标归属后向目标 Runner 取消；Runner 停止尚未启动的工作、协调在途工具、等待可回收资源并返回终态。对拥有后代的目标，先停止其继续派生，再按父子关系收尾后代，最后收尾父 Agent。

## 额外验收边界

- 中断 Waiting Agent 应快速进入可确认的 Interrupted，不需要等待不存在的模型请求。
- 中断 Running Agent 时，已完成并持久化的结果保留；未开始任务不启动；结果未知的副作用标记未知。
- 中断过程中重复请求返回同一收尾操作，不重复发起关闭。
- 目标 ID 已指向新 Runtime Instance 时，旧中断请求不能取消替代实例。
- 工具进程未退出或 Store 回执未确认时，状态保持收尾中，不向调用者报告“已停止”。
