# 实现控件树快照、失效与增量刷新

## 目标与产出

在 UIA Cache Request 和事件订阅基础上，维护跨工具调用的有界控件树快照，并在 UI 变化后局部刷新或重建。

## 设计

快照记录 WindowId、Tree View、属性集、快照代次、元素引用、父子关系和 dirty 状态。Cache Request 用于一次获取元素时预取属性/Pattern；Agentelier 快照是独立的应用侧结构。

事件失效规则：
- PropertyChanged：使对应节点的指定属性失效。
- FocusChanged：更新焦点投影。
- StructureChanged：根据事件 RuntimeId 和 ChangeType 刷新受影响子树。
- 通用 AutomationEvent：按订阅语义标记对应投影失效。
- 事件丢失、队列溢出或无法定位变更范围：整个窗口快照 dirty，下次读取完整重建。

局部刷新应保持未受影响节点的快照引用，但不能让过期快照覆盖实时状态。执行 Pattern 副作用前，重新查询目标元素并验证必要状态。

## Rhai API

提供 `uia_snapshot(window_ref, view, properties, max_nodes)`、`uia_snapshot_refresh(snapshot_id, scope)` 和 `uia_snapshot_status(snapshot_id)`。

## 范围

包含快照结构、代次、Cache Request 集成、事件驱动失效、局部刷新、增量合并、完整重建和游标失效。不引入独立 Benchmark 或缓存服务。

## 验收

- 能创建 Raw/Control/Content 三种视图的有界快照。
- 快照标明属性来源、代次和 dirty 状态。
- 属性事件只使相关属性失效。
- 结构事件使受影响子树失效；无法定位时整树失效。
- 队列溢出会触发完整刷新，不继续返回“看似新鲜”的旧结果。
- 增量更新保持父子关系和引用关系一致。
- Notepad 文本变化后属性/文本快照更新。
- File Explorer 展开文件夹、切换目录或新增临时文件后结构快照更新。
- Accessibility Insights 对照同一状态下的树和属性。
