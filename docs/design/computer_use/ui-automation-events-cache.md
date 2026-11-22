# UIA 事件、条件等待与控件树缓存

UIA 事件让客户端知道界面发生了变化；Cache Request 减少重复读取属性和 Pattern 的跨进程调用。两者结合后，Agentelier 可以维护有界的树快照，并在变化后按需刷新。

## 事件订阅

`uiautomation` 0.25.3 的 `event` Feature 默认关闭，启用后提供：

- `add_automation_event_handler` / `remove_automation_event_handler`
- `add_property_changed_event_handler` / `remove_property_changed_event_handler`
- `add_structure_changed_event_handler` / `remove_structure_changed_event_handler`
- `add_focus_changed_event_handler` / `remove_focus_changed_event_handler`
- `remove_all_event_handlers`

订阅记录包含订阅 ID、事件类别、目标元素/窗口、TreeScope、属性过滤列表和 handler wrapper。注册与移除均由 MTA Worker 串行执行。Worker 必须持有用于移除订阅的 handler 和目标元素引用，直到 remove 调用完成。

关闭会话时逐一移除它拥有的监听。Windows 的 `RemoveAllEventHandlers` 会清除整个进程的注册，因此只在最后一个 UIA Worker 退出时调用。来源见 [Microsoft UI Automation 事件订阅](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-eventsforclients)。

会话最多保留 32 项公开订阅，事件队列最多 256 条。属性订阅的空过滤列表使用常用属性；`ValueValue` 可用于监听 Value Pattern 的值变化。自动化事件用 `automation:<EventId>`，其余使用 `property`、`structure`、`focus`。焦点使用 Windows 全局监听，默认在 Worker 查询当前焦点后按目标范围筛选，也可显式选择 `global`。

`uia_events_next` 当有可见事件、报告溢出或超时时返回，单次最多 64 条。事件中的 `ref` 指向订阅目标。焦点 ID 是 Worker 处理通知时的当前焦点；属性事件提供属性 ID、可转换值以及 `value_available`；结构事件提供变化类型和 Provider 给出的 RuntimeId。返回的 `overflowed` 是该批次丢过通知的订阅 ID。取消后丢弃该订阅尚未读取的通知。

## 事件回调只发通知

上游事件 handler 会把 sender 包装成 `UIElement` 后调用自定义回调。Agentelier 的回调不应在其中调用 UIA getter、遍历树或执行 Pattern，也不应把 sender COM 对象放进跨线程队列。

回调只将已提供的普通事件数据送入有界队列：

| 事件 | 回调记录 | 后续处理 |
|---|---|---|
| FocusChanged | 订阅 ID、事件类型 | Worker 查询当前焦点元素并更新焦点投影 |
| PropertyChanged | 订阅 ID、Property ID、可转换的新值 | 标记该元素对应属性失效；需要时重新读取 |
| StructureChanged | 订阅 ID、ChangeType、RuntimeId（若有） | 定位受影响子树；无法定位时整棵窗口树失效 |
| AutomationEvent | 订阅 ID、Event ID | 按订阅范围标记状态变化并重新读取必要信息 |

事件回调不能阻塞等待队列。队列满时，属性事件可以合并；结构事件应把受影响范围标记为需要刷新；无法判断丢失了哪些结构变化时，整棵窗口快照标记为 dirty。不能在队列溢出后继续把旧快照当作当前状态。

## 条件等待

条件等待支持：

- 等待元素出现或消失。
- 等待 IsEnabled 或其他属性达到指定值。
- 等待焦点变化。
- 等待控件树结构变化。
- 等待某个元素/属性条件成立。

安全的等待顺序是先注册相关事件，再读取一次当前条件；如果条件已成立立即返回，否则等待事件通知，再重新读取条件。这样避免“先检查、后注册”之间发生状态变化而丢失唤醒。

事件无法覆盖的条件使用有界检查间隔。等待必须同时受调用方 timeout 和当前 Rhai `Control.deadline` 约束，并响应取消。

等待在 Rhai 脚本内部执行，因此当前 `computer_execute` 的 `ToolAdmission::Exclusive` 在等待期间仍然被持有。这与现有 `wait(ms)` 一致。事件监听本身可以跨调用保持订阅，但被动事件接收不持有 ToolPermit，也不主动操作桌面。

`uia_wait` 以 `kind` 区分条件。`appears`、`disappears` 接收与 `uia_find` 相同的 `selector`，在 Control View 的 16 层、512 节点内检查；搜索被截断时不能确认元素已消失。`property` 接收 `name` 和 `equals`，`enabled` 和 `focused` 分别检查 `IsEnabled` 和 `HasKeyboardFocus`。超时为 1–30000 毫秒，并受脚本剩余时间限制。

`focus_changed` 比较当前桌面焦点与注册后的初始焦点，`structure_changed` 比较目标 Control View 中最多 128 个节点的 RuntimeId 及层级。临时订阅使用单独的唤醒通道。通知、或最长 100 毫秒检查间隔结束后，Worker 重读条件。完成、超时和取消均回收临时监听。

## 两种不同的缓存

### UIA Cache Request

`UICacheRequest` 是上游 UIA 的属性/Pattern 预取机制。它可以配置：

- 要缓存的 Property。
- 要缓存的 Pattern。
- TreeScope。
- TreeFilter（Raw/Control/Content 等视图条件）。
- AutomationElementMode。

`UIElement`、TreeWalker 和 Find 操作提供 BuildCache 变体；`UIElement::build_updated_cache` 可以刷新某个元素的缓存。Cache Request 不会自动持续监听变化，也不会自动生成增量 diff。

### Agentelier 控件树快照

Agentelier 另外维护面向模型的有界快照。快照包含：

- WindowId token。
- Tree View。
- 属性集。
- 快照代次。
- 节点父子关系。
- 元素逻辑引用。
- 缓存来源和读取时间语义。
- dirty 状态与失效范围。

UIA Cache Request 减少单次读取成本；Agentelier 快照负责跨多次工具调用的读取复用。两者不能混为一谈。

每个会话最多保留 8 个快照，每个快照最多记录 64 个节点。快照通过单元素 Cache Request 读取指定属性，记录选定视图范围内的前序节点及其父引用。状态包含 `dirty`、`rebuild_required`、`truncated` 和窗口可用性。刷新范围可选 `auto`、`full` 或快照内的节点引用。成功刷新后递增代次，未执行读取的 `unchanged` 保持原代次。Provider 没有发出预期通知时，可请求 `full`。

## 事件驱动失效与增量刷新

- PropertyChanged：使对应元素的对应属性失效；不需要重建整棵树。
- FocusChanged：刷新焦点投影；若快照包含 HasKeyboardFocus，更新相关节点。
- StructureChanged：根据 RuntimeId 和 ChangeType 定位受影响父节点/子树并刷新。
- AutomationEvent：按事件语义使相关投影失效。
- 事件丢失、队列溢出或无法定位变化范围：将窗口快照整体标记为 dirty，下次读取时完整重建。

增量更新不是 UIA Provider 自动给出的结果，而是 Agentelier 根据事件信号重新读取并合并自身快照。副作用操作前，必须重新验证目标元素的当前状态，不能只依赖旧快照。

快照使用逐节点的属性订阅识别变更归属。已知的 ChildRemoved RuntimeId 可定位父节点并刷新子树；Win32 的其他结构变化可能没有 RuntimeId，此时重建窗口投影。快照的 `ValueValue` 字段可由值属性变化和 TextChanged 通知触发回读。快照队列溢出令该窗口重建，公开事件队列溢出令会话快照重建。重建和结构刷新会使窗口分页游标失效。Provider 读取失败时保留上一次完成的快照和待重建状态。来源见 [Win32 结构事件参数](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationstructurechangedeventhandler-handlestructurechangedevent)。

## Accessibility Insights 对照

Accessibility Insights for Windows 的 Live Inspect 能显示 UIA Tree、元素属性和支持的 Pattern；Troubleshooting 中的 Events 视图可监听 UIA 事件。

未来验收时，对同一个窗口和相同 UI 状态：
1. 在 Live Inspect 选择 Raw/Control/Content 视图并查看目标元素。
2. 在 Properties 和 Patterns 区域记录属性及 Pattern。
3. 在元素菜单选择 Listen to Events，配置焦点、属性和结构事件并开始记录。
4. 在 Notepad 或 File Explorer 执行对应操作。
5. 将 Accessibility Insights 记录与 Agentelier 的事件队列和刷新结果对照。

来源：[Accessibility Insights Live Inspect 与 Event Monitoring](https://accessibilityinsights.io/docs/windows/getstarted/eventmonitoring/)。

## 来源

- [uiautomation events/mod.rs](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/events/mod.rs)
- [uiautomation events/handlers.rs](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/events/handlers.rs)
- [uiautomation core.rs：Cache Request 与事件 API](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/core.rs)
- [Microsoft UI Automation Events for Clients](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-eventsforclients)
- [Microsoft UI Automation Threading Issues](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)
