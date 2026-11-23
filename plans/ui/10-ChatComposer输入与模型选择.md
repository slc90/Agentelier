# 建立 ChatComposer 与当前 Session 输入控制

## 目标

交付 D 区底部输入组件，只保留多行输入、当前 Session 模型选择和发送 / 停止按钮。

## 输入组成

ChatComposer 只包含多行消息输入框、当前 Session 模型选择和发送 / 停止按钮。不加入附件 `+`、语音输入、文件上传或额外工具入口。空闲时显示发送；当前 Turn 正在生成时显示停止。推理强度不放入聊天输入区。

默认模型和推理强度来自应用配置：新 Session 使用 `gpt-6.2-sol` / `high`；已有 Session 使用自己的固定设置。模型选择器修改当前 Session 的模型，不修改全局默认值或其他 Session。

## 输入草稿与 Agent 接纳

草稿按 Session 隔离。发送请求在收到 Agent 接纳回执前不清空草稿；拒绝接纳时保留原文。接纳后用户继续输入的新文字不能被迟到回执清空。

发送、Steer、Queue、Retry、Stop 和待处理结果续接沿用 Conversation / Runtime 语义。停止按钮只提交当前 Session 当前 Turn 的合法停止请求，不停止其他 Session 后台任务。按钮状态由真实业务回执驱动。

## Widgetry 复用

TextField 负责多行编辑、IME、Caret 和 Selection；ComboBox 负责模型选择；AgentelierButton 负责发送与停止；Widgetry Disabled / Loading 表达真实控件状态。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
## 完成条件与验收

1. 输入区只有多行输入框、模型选择器和发送 / 停止按钮。
2. 没有附件 `+`、语音按钮、文件上传或额外工具入口。
3. 中文 IME 组合输入期间，预编辑文本不会被错误提交或提前清空。
4. 空闲时显示发送；当前 Turn 生成期间显示停止。
5. 发送后在 Agent 接纳回执前保留原草稿；拒绝接纳时原文仍在输入框中。
6. 接纳后用户继续输入的新文字不会被旧回执清空。
7. 快速连续点击发送不会产生重复提交；拒绝时显示真实错误。
8. 停止只影响当前 Session 当前 Turn，不影响其他 Session 后台任务。
9. 停止请求后，按钮状态按 Agent 回执和真实 Turn 终态更新，不因点击就伪装为已停止。
10. 模型列表来自当前模型目录；切换后提交给当前 Session，其他 Session 和全局默认值不变。
11. 当前 Session 重开后恢复其已保存模型；新 Session 使用默认模型 `gpt-6.2-sol` / `high`。
12. 模型不支持的推理档位不静默回退；聊天输入区不增加推理强度选择器。
13. 生成期间切换模型不改变已经发出的请求，只影响后续请求。
14. C 切换 Session 后草稿、模型选择和按钮状态绑定目标 Session；迟到回执不能清空另一个 Session 的草稿。
15. 使用真实 GUI 验收 IME、键盘提交、发送、停止和输入焦点，并核对 Agent 接纳与终态回执。
