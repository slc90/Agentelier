# Rhai UIA API 与结构化结果契约

UIA 能力通过现有 `computer_execute` 暴露给模型。Rhai 继续负责组织多步桌面任务；UIA API 只接受普通参数、逻辑元素引用并返回普通数据。底层 COM 对象完全留在 MTA Worker。

## API 分组

所有新增函数使用 `uia_` 前缀，避免与现有 `windows()`、`window()`、`click()`、`wait()` 等 Computer API 名称冲突。

| 能力 | 建议 API | 行为 |
|---|---|---|
| 根元素 | `uia_desktop_root()` | 获取桌面根元素引用 |
| 窗口根元素 | `uia_window_root(window_id)` | 从现有 WindowId 获取 UIA 窗口根元素 |
| 根元素摘要 | `uia_root_info(element_ref)` | 重新读取根元素并核对 Provider 和窗口身份 |
| 控件树 | `uia_tree(element_ref, view, depth, limit, cursor)` | 获取有界树页 |
| 树导航 | `uia_tree_related(element_ref, view, relation)` | 查询父、首子、前一兄弟或后一兄弟 |
| 元素属性 | `uia_properties(element_ref, property_names)` | 读取指定属性 |
| 属性缓存 | `uia_prefetch(element_ref, property_names, pattern_names, view, scope)` | 按选定视图和范围批量预取属性与 Pattern |
| 读取缓存 | `uia_cached_properties(element_ref, property_names)` | 读取已预取的属性 |
| 元素查找 | `uia_find(root_ref, conditions, view, scope, max_depth, max_nodes, max_results)` | 在节点和候选预算内按组合条件查找 |
| Pattern 能力 | `uia_patterns(element_ref)` | 返回当前元素支持的 Pattern |
| 引用状态 | `uia_ref_info(element_ref)` | 查看引用是否有效、代次以及能否尝试重定位 |
| 元素刷新 | `uia_refresh_element(element_ref)` | 重新读取当前元素状态 |
| 基础 Pattern | `uia_invoke`、`uia_value_get`、`uia_value_set`、`uia_toggle`、`uia_range_value_get`、`uia_range_value_set`、`uia_selection`、`uia_select`、`uia_expand_collapse` | 执行对应语义操作 |
| 滚动与虚拟化 | `uia_scroll`、`uia_scroll_item`、`uia_find_item`、`uia_realize` | 滚动并回读、容器定位和项目实例化 |
| 文本 | `uia_text_document_range`、`uia_text_selection`、`uia_text_visible_ranges`、`uia_text_get`、`uia_text_find`、`uia_text_range_move`、`uia_text_range_select` 等 | 读取和操作文本范围 |
| 结构化数据 | `uia_grid`、`uia_grid_item`、`uia_table`、`uia_table_item`、`uia_spreadsheet` | 读取行列和单元格 |
| 事件 | `uia_subscribe`、`uia_unsubscribe`、`uia_events_next` | 管理订阅并读取事件 |
| 等待 | `uia_wait` | 等待元素/属性/焦点/结构条件 |
| 快照 | `uia_snapshot`、`uia_snapshot_refresh` | 读取和刷新窗口快照 |

具体参数和返回字段在实现方案中保持一致；不要为每个 Pattern 创建重复的元素查找接口。Pattern 操作接收 `element_ref`，由 Worker 解析实际 Pattern wrapper。

## 元素引用

模型看到的引用是 opaque string。它只在所属 ScriptSession 内有效，不能跨会话复制使用。引用绑定 WindowId token、引用代次和 Worker 内部元素记录。

Rhai 可以将引用保存在持久变量中，但变量本身不持有 COM 对象。Worker 根据引用 ID 找到实际 UIElement，并在操作前检查窗口和元素状态。

## 结构化结果

所有 UIA 函数使用统一外层结构：

| 字段 | 含义 |
|---|---|
| `ok` | 本次 UIA 请求是否成功 |
| `data` | 成功时的普通结构化结果 |
| `error` | 失败时的稳定错误代码和消息 |
| `source` | `live`、`cache` 或 `snapshot` |
| `generation` | 对树快照有意义时的快照代次 |

元素摘要包含 `ref`、`name`、`automation_id`、`control_type`、`class_name`、`bounds`、`enabled`、`offscreen`、`focused`、`framework_id` 和实际支持的 `patterns`。按需属性读取不能强制每个节点都包含所有扩展属性。

树结果包含视图、根引用、节点列表、父子关系、节点数、是否还有后续页、继续读取游标和快照代次。查找结果包含匹配数、候选元素摘要及是否存在歧义。

Pattern 操作结果包含目标引用、Pattern 名称、操作名、实际返回值和必要的操作后状态。Invoke 只报告 Pattern 调用结果，不推断应用业务流程已经完成。

## 错误代码

| 错误代码 | 含义 |
|---|---|
| `uia_runtime_unavailable` | Worker 未启动或已退出 |
| `uia_com_initialization_failed` | MTA/COM 初始化失败 |
| `uia_queue_full` | 请求队列达到容量 |
| `uia_window_unavailable` | WindowId 已失效 |
| `uia_provider_unavailable` | Provider 无法访问 |
| `uia_element_not_found` | 查找没有匹配项 |
| `uia_element_unavailable` | 逻辑引用对应元素已失效 |
| `uia_ambiguous_target` | 重新定位得到多个候选 |
| `uia_pattern_not_supported` | 元素不支持请求的 Pattern |
| `uia_invalid_argument` | 参数不符合 Pattern 契约 |
| `uia_provider_error` | Provider 返回其他错误 |
| `uia_tree_cursor_stale` | 树变化导致继续读取游标失效 |
| `uia_timeout` | 等待或请求超过期限 |
| `uia_cancelled` | 请求已取消 |

错误映射应保留底层 HRESULT/错误分类的诊断信息，但不把 COM wrapper 或原始接口指针返回给 Rhai。

## 与 ToolPayload 的关系

UIA 返回的树、属性和 Pattern 结果是普通文本/JSON，不创建新的 ToolPayload 类型。截图继续通过现有 Computer API 返回 `ToolPayload::Image`。一次 `computer_execute` 仍沿用现有工具调用 ID、文本输出预算、图片预算和协议历史。

## 示例工作流（行为层面）

模型先枚举窗口并取得目标窗口 ID，再请求 UIA 根元素和有限层级控件树；根据 Name、AutomationId、ControlType 和 Pattern 能力定位目标元素；执行 Pattern 后读取操作结果和必要属性；若界面变化则读取事件或刷新快照。对于没有可用 UIA 控件的区域，模型调用原有截图和鼠标键盘 API。

## 来源

- [Agentelier Computer Use 设计](computer-use.md)
- [Agentelier ScriptSession](../../../crates/agent/tools/src/script.rs)
- [Agentelier Computer API](../../../crates/agent/tools/src/script/computer.rs)
- [Agentelier Tool Catalog](../../../crates/agent/tools/src/catalog.rs)
