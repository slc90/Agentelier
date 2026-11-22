# 接入 LegacyIAccessible 与元数据 Pattern

## 目标与产出

接入传统 MSAA 兼容信息，以及多视图、注释和样式等元数据 Pattern。

## 设计

接入 `UILegacyIAccessiblePattern`、`UIMultipleViewPattern`、`UIAnnotationPattern` 和 `UIStylesPattern`。

- LegacyIAccessible：读取 MSAA role、state、name、value、description、keyboard shortcut 等信息，并调用其实际支持的旧式操作。
- MultipleView：枚举可用视图、读取当前视图并切换。
- Annotation：读取注释类型、作者、日期和目标等 Provider 暴露信息。
- Styles：读取填充、形状、样式和颜色等 Provider 暴露信息。

这些 Pattern 的属性和操作不是所有控件都提供。结果中必须区分 Pattern 不支持、属性缺失和 Provider 错误。

## Rhai API

提供 `uia_legacy_accessible`、`uia_multiple_view`、`uia_annotation` 和 `uia_styles`，输出普通数据，不返回 COM wrapper。

## 范围

只包含上述四类 Pattern，不包括 ObjectModel（由下一方案负责）。

## 验收

- 能查询 Pattern 支持情况。
- LegacyIAccessible 能读取可用的 MSAA 属性和操作结果。
- MultipleView 能读取和切换实际可用视图。
- Annotation/Styles 只返回 Provider 实际提供的属性。
- 不支持时返回明确结果。
- Notepad 和 File Explorer 的实际支持情况记录在 Pattern 矩阵中，不把无支持控件记作实现失败。
