# 实现三种视图的控件树遍历与有界继续读取

## 目标与产出

提供 Raw、Control、Content 三种 UIA 视图的树遍历、局部展开和有界分页，使模型可以逐步探索大型窗口，而不是一次性获取整棵树。

## 设计

使用 `get_raw_view_walker()`、`get_control_view_walker()` 和 `get_content_view_walker()`。每次请求明确视图、起始元素、最大深度和节点预算。

`UITreeWalker` 提供 parent、first child、next sibling、previous sibling 及 BuildCache 变体。它没有内建分页游标。Agentelier 在 Worker 中保存遍历位置，继续读取时使用同一快照代次和游标状态。

不要直接依赖 `UITreeWalker::get_children()` 完成严格遍历：该 helper 将遍历过程中的错误折叠为 `Option`。需要区分“没有子节点/没有下一个兄弟”与元素失效、Provider 错误。

## Rhai API

`uia_tree(element_ref, view, max_depth, max_nodes, cursor)` 返回节点摘要、父子关系、是否有下一页、继续读取游标和快照代次。

视图参数使用 `raw`、`control`、`content`。默认使用 Control View 和有限节点预算；局部展开只读取指定引用的子树。

## 范围

包含三种视图、父子/兄弟遍历、局部展开、深度和节点限制、继续读取游标、树变化导致的游标失效。不包含完整快照缓存和增量合并。

## 验收

- 三种视图均能独立读取。
- 同一窗口在三种视图下呈现不同的 UIA 投影时，结果与 Accessibility Insights UIA Tree 对应。
- 支持父、子、后代、前后兄弟遍历。
- 深度和节点预算严格生效。
- 继续读取不会重复已返回节点。
- 控件树变化后旧游标返回 `uia_tree_cursor_stale`。
- 中途元素失效时保留已读取部分并报告具体失败位置。
- Notepad 可识别编辑区域及外围控件；File Explorer 可识别导航树和文件列表。
