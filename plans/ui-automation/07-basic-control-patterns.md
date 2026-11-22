# 接入基础交互 Control Pattern

## 目标与产出

让 Agent 能通过控件自身的语义接口执行常见操作，而不是统一退化成鼠标坐标点击。

## 设计

接入 Invoke、Value、Toggle、RangeValue、Selection、SelectionItem 和 ExpandCollapse。

操作前查询目标元素是否支持对应 Pattern。Pattern wrapper 使用 `UIElement::get_pattern<T>()` 获取；操作结果应返回实际状态，而不是只返回调用是否抛错。

- Invoke：调用完成不等于应用业务流程完成。
- Value：读取、设置和回读实际值；只读状态应明确报告。
- Toggle：返回 ToggleState。
- RangeValue：返回当前值、最小/最大值、步长和只读状态；设置前检查范围。
- Selection：返回当前选中元素。
- SelectionItem：区分 Select、AddToSelection、RemoveFromSelection。
- ExpandCollapse：返回实际展开状态。

## Rhai API

分别提供 `uia_invoke`、`uia_value_get`、`uia_value_set`、`uia_toggle`、`uia_range_value_get`、`uia_range_value_set`、`uia_selection`、`uia_select` 和 `uia_expand_collapse`。所有函数接收 ElementRef，不接收 COM 对象。

## 范围

只包含基础交互 Pattern。Scroll、Text、Grid、Window、Transform 和其他 Pattern 由后续方案负责。

## 验收

- 每个 Pattern 均能查询支持情况。
- 不支持时返回 `uia_pattern_not_supported`。
- Value/Toggler/RangeValue 操作后返回实际状态。
- Selection/SelectionItem 能读取和改变选择状态。
- ExpandCollapse 能返回实际 ExpandCollapseState。
- Notepad 和 File Explorer 中实际支持的按钮、编辑框、树项和列表项可以通过对应 Pattern 操作。
- 通过 Accessibility Insights 的 Pattern Actions 对照至少一项实际操作。
