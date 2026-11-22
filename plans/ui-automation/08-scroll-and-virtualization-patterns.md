# 接入滚动与虚拟化 Control Pattern

## 目标与产出

支持列表、树和文件容器中的滚动、项目定位和虚拟化元素实例化。

## 设计

接入 Scroll、ScrollItem、ItemContainer 和 VirtualizedItem。

- Scroll：读取水平/垂直滚动状态、滚动百分比和可滚动方向；执行指定滚动量。
- ScrollItem：请求将目标元素滚入可见区域。
- ItemContainer：在容器中按属性查找项目，适合大型项目集合。
- VirtualizedItem：对尚未完全实例化的项目执行 Realize；完成后重新获取属性和 Pattern，不能假设原引用已经变成完整可用对象。

列表读取必须有界。对于大量文件或列表项，优先使用 ItemContainer/VirtualizedItem 和分页读取，而不是一次性展开全部后代。

## Rhai API

提供 `uia_scroll`、`uia_scroll_item`、`uia_find_item` 和 `uia_realize`，返回实际滚动状态、定位结果和 Realize 后的新引用状态。

## 范围

包含 Scroll、ScrollItem、ItemContainer、VirtualizedItem 及其结果契约。不包含 Grid/Table 的行列语义。

## 验收

- 支持的容器能读取滚动状态并执行滚动。
- ScrollItem 能使目标元素进入可见区域。
- ItemContainer 能按属性查找容器项目。
- VirtualizedItem Realize 后重新读取元素属性。
- 不支持时返回明确 Pattern 不支持结果。
- File Explorer 在包含足够多临时文件的目录中验证滚动、项目定位和虚拟化行为。
- Accessibility Insights 的 Patterns 区域与 Agentelier 查询结果一致。
