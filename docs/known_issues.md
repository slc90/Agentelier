# 已知问题

## BRP 特殊按键缺少逻辑键值

当前锁定的 `bevy_brp_extras` 0.4.2（Git revision `341be542`）在键盘事件生成中，只为可映射字符填写 `logical_key`；Control、Backspace 等特殊键得到 `Key::Unidentified`。Bevy 0.20.1 文本输入使用逻辑键识别编辑与快捷键，因此标准 `send_keys` 的 Ctrl+A、Backspace 等操作不能等价于完整键盘输入。已在真实聊天输入中观察到 Ctrl+A 仅插入字符 a、Backspace 未删除，并核对上游 `keyboard/events.rs` 与 Bevy `text_input.rs` 确认原因。

验证此类输入时可使用标准 BRP `world.write_message` 提交完整的 `bevy_input::keyboard::KeyboardInput`，先通过 Pointer 建立焦点，再发送有正确物理键、逻辑键及窗口的 Press/Release。消息仍经过应用的输入、焦点和文本编辑系统，不修改编辑器组件或业务状态；该证据属于 BRP 键盘输入，不能视为原生 OS 或 IME 验证。当前不引入修改版上游源码。

## 切页后部分静态文字偶发未绘制

在 Bevy 0.20.1 / Widgetry 430ddb44 的真实窗口中，切换设置页再返回后，曾观察到部分账号静态文字未绘制；对应 Text、颜色、布局尺寸仍存在，诊断时改变 Text 可恢复绘制。账号凭据与聊天会话不受影响。原因尚未确认，可能涉及新建文本的布局或绘制时序，不能确定属于上游缺陷；目前不修改上游源码，也不通过逐帧重置文字掩盖现象。

## 当前订阅授权的接口能力边界

2026-11-17，同一 Agentelier 动态客户端授权访问公共 Responses 普通生成成功，显式 compaction_trigger 返回 HTTP 400、subscription_sharing_unsupported_capability；改发 Codex 后端普通生成返回 HTTP 401、no_matching_rule，补充常用 Codex 请求头仍被拒绝。当前实现使用公共 Responses 的 context_management Inline 压缩，不依赖上述被拒绝的入口或控制项。

当前已通过真实多压缩项响应、密文续接、工具往返、压缩阶段 Stop/Steer/Queue，以及恢复正式阈值后的检查点重启和队列续接验证。密文跨模型兼容标识变化或跨账号续接不作保证，服务拒绝仍保留已提交历史并显示真实失败；本次未切换账号验证。极低调试阈值不作为正式配置保留。

当前窗口布局下消息区域仍受其他区域挤压，完整旧历史的截图与滚动可见性尚未完成核对；本次仅验证压缩阶段与执行状态，不调整既有布局。
