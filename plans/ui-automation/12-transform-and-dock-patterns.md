# 接入 Transform 与 Dock Pattern

## 目标与产出

支持 UIA 元素的移动、缩放、旋转及停靠状态操作。

## 设计

接入 Transform 和 Transform2 的现有 wrapper，以及 Dock Pattern。Transform2 扩展接口已由 `UITransformPattern` wrapper 内部处理，不另建重复的 Transform2 类型。

执行前查询 CanMove、CanResize、CanRotate 和 CanZoom 等能力；调用后重新读取边界或停靠状态。UIA 坐标/尺寸单位和现有截图像素坐标之间不做未经核对的直接换算。

## Rhai API

提供 `uia_transform_capabilities`、`uia_transform` 和 `uia_dock_position`，参数按对应 Pattern 的单位和范围表达。

## 范围

包含 Transform、Transform2 和 Dock。不包含通用鼠标拖动或窗口顶层激活。

## 验收

- 查询 Transform/Dock Pattern 可用性和能力状态。
- 只对 Provider 声明支持的动作调用 Pattern。
- 操作后读取实际位置、尺寸或 DockPosition。
- 不支持动作返回明确结果。
- 在 Notepad 和 File Explorer 中检查实际支持情况；若目标应用不暴露这些 Pattern，记录不支持，不虚构成功操作。
