# 接入 Drag、DropTarget、CustomNavigation 与 SynchronizedInput

## 目标与产出

接入 Provider 暴露的拖放语义、自定义导航和同步输入 Pattern。

## 设计

`uiautomation` 已提供 `UIDragPattern`、`UIDropTargetPattern`、`UICustomNavigationPattern` 和 `UISynchronizedInputPattern` wrapper。

Drag/DropTarget 描述 UIA Provider 暴露的拖动源和放置目标能力；CustomNavigation 用于非标准元素导航；SynchronizedInput 用于 Provider 与客户端协调输入事件。它们与现有 Computer Use 物理鼠标轨迹不同：Pattern 操作必须使用目标元素真实支持的接口，不把鼠标轨迹包装成 Pattern。

## Rhai API

提供 `uia_drag_info`、`uia_drop_target_info`、`uia_custom_navigation` 和 `uia_synchronized_input`，每个 API 只开放 wrapper 已支持的动作和参数。

## 范围

包含上述四类 Pattern 的能力查询、操作和结果映射。不增加通用拖放框架，不替换 `drag()` 和 `trajectory()`。

## 验收

- 查询各 Pattern 的可用性。
- 支持的 Drag/DropTarget 操作返回 Provider 结果。
- CustomNavigation 能按 Provider 返回的导航关系定位元素。
- SynchronizedInput 能开始/结束指定输入监听并返回状态。
- 不支持时返回明确 Pattern 不支持结果。
- Notepad 与 File Explorer 的实际 Provider 支持情况逐项记录；无支持元素时不伪造应用级操作验收。
