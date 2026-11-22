# 实现 UIA 事件订阅与回收

## 目标与产出

接入 Automation Event、PropertyChanged、StructureChanged 和 FocusChanged 事件，并使订阅可跨多次 `computer_execute` 调用保持。

## 设计

启用 `uiautomation` 的 `event` Feature。MTA Worker 独占事件 handler 和订阅表；注册/移除都通过 Worker 命令串行执行。订阅记录包含订阅 ID、窗口/元素引用、事件类别、TreeScope 和属性过滤条件。

回调只复制普通事件数据并非阻塞投递，不调用 UIA getter，不保留 sender `UIElement`。事件队列有界；重复属性事件可合并，结构事件无法定位时标记窗口快照需要完整刷新。

被动事件订阅在 `computer_execute` 返回后可继续存在，不持有 ToolPermit。取消订阅和 Worker 关闭时，在 MTA Worker 内调用对应 remove API；退出时再调用 `remove_all_event_handlers()` 作为收尾。

## Rhai API

提供 `uia_subscribe(element_ref, event_spec, scope, properties)`、`uia_unsubscribe(subscription_id)` 和 `uia_events_next(timeout_ms, max_events)`。

## 范围

包含四类事件、注册、过滤、事件 DTO、队列、取消和清理。不包含条件等待和快照增量合并。

## 验收

- 能注册和移除四类事件。
- PropertyChanged 能返回属性 ID 和可转换的新值。
- StructureChanged 能返回 ChangeType 和 RuntimeId（Provider 提供时）。
- FocusChanged 能唤醒后续焦点查询。
- 回调不跨线程传递 COM 对象、不执行长时间 UIA 调用。
- 队列有界，溢出后相关缓存被标记为 dirty。
- 取消订阅后不再收到该订阅事件。
- ScriptSession 关闭时全部订阅移除。
- Accessibility Insights Events 视图记录同一操作产生的事件，并与 Agentelier 事件类型对照。
