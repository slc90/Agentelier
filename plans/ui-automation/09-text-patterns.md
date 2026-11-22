# 接入文本与 TextRange Pattern

## 目标与产出

提供文本控件的全文读取、范围读取、选择、文本属性和编辑状态访问。

## 设计

接入 Text、Text2、TextEdit、TextChild 和 TextRange。

`UITextPattern` wrapper 已封装 TextPattern2 的扩展接口；TextRange 是 Text Pattern 返回的范围对象，不是独立 Pattern ID。TextRange 操作包含 GetText、FindText、端点移动/扩展、Select、ScrollIntoView 和文本属性读取。

Text Pattern 主要提供文本内容和范围导航，不应把 `TextRange.Select()` 描述为修改文本。文本修改使用目标元素实际支持的 Value Pattern；如果目标控件没有可写 Pattern，则明确返回不支持，模型可选择现有 Computer Use 输入方式。

## Rhai API

提供 `uia_text_document_range`、`uia_text_selection`、`uia_text_visible_ranges`、`uia_text_get`、`uia_text_find`、`uia_text_range_move`、`uia_text_range_select` 和 `uia_text_edit_state`。文本返回必须有字符预算和范围限制。

## 范围

包含 Text、Text2、TextEdit、TextChild 和 TextRange。不包含 Value Pattern 本身（由基础 Pattern 方案负责）。

## 验收

- 能查询文本相关 Pattern 实际支持情况。
- 能读取 DocumentRange 和选择范围。
- 能使用 FindText 定位子文本。
- 能移动/扩展 TextRange 端点并选择范围。
- 能读取可见文本范围和文本属性（Provider 支持时）。
- 文本返回受预算限制。
- 不支持 Text Pattern 时返回明确结果。
- 在 Notepad 临时文档中准备 `Alpha Beta Gamma`，读取全文，定位并选择 `Beta`，核对选择范围与文本内容。
- 若 Notepad 暴露可写 Value Pattern，设置新文本并回读；若未暴露，记录真实能力并通过现有 Computer Use 输入验证互补路径，不把键盘输入记作 UIA Pattern 成功。
- 使用 Accessibility Insights 对照文本元素属性、Pattern 和事件。
