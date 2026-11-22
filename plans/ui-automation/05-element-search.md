# 实现多条件元素查找

## 目标与产出

提供限定窗口或父元素范围的 UIA 查找，支持 AutomationId、Name、ControlType、ClassName 和多条件组合。

## 设计

优先使用 UIA 原生 `UICondition`：`create_property_condition()`、`create_and_condition()`、`create_or_condition()` 和 `create_not_condition()`，再通过 `UIElement::find_first()` / `find_all()` 查找。

`UIMatcher` 支持 Name、ClassName、ControlType、自定义过滤函数、视图、深度和超时，但当前没有 AutomationId builder。AutomationId 使用 `UIProperty::AutomationId` 条件，不能假设 `UIMatcher.automation_id()` 存在。

`FindAll` 会一次返回所有匹配元素；对可能产生大量结果的范围，使用 Agentelier 有界 TreeWalker 搜索并设置最大节点数和结果数。不要只在序列化时截断结果，却让底层查找无界运行。

AutomationId 可能缺失或重复，Name 可能动态变化或本地化。默认返回所有有界候选；只有调用明确指定 first-match 时才返回第一个结果。

## Rhai API

`uia_find(root_ref, conditions, view, scope, max_depth, max_nodes, max_results)` 返回匹配数量、候选元素摘要、引用和是否存在歧义。

## 范围

包含单条件/组合条件、搜索范围、视图、结果预算和查找错误。不包含逻辑引用的长期失效恢复。

## 验收

- AutomationId、Name、ControlType、ClassName 单条件查找正确。
- 多条件 AND/OR/NOT 组合正确。
- 可限定窗口根元素或某个父元素的后代。
- 无匹配返回 `uia_element_not_found`。
- 多匹配返回候选和歧义信息，不自动选择不确定目标。
- 大型搜索受节点和结果预算限制。
- Notepad 能定位实际编辑控件；File Explorer 能定位导航树和文件列表中的目标元素。
