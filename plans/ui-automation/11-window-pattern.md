# 接入 Window Control Pattern

## 目标与产出

为 UIA 暴露的窗口元素提供标准窗口状态和操作能力。

## 设计

接入 `UIWindowPattern`，覆盖窗口可视状态、是否模态、是否顶层窗口、交互状态和关闭操作。操作必须针对元素当前支持的 Window Pattern，而不是把 HWND 的现有激活接口混入 Pattern。

现有 `desktop::activate_window()` 仍负责顶层窗口激活和前台状态核对；Window Pattern 负责 Provider 暴露的窗口语义。两者职责不同，不相互替代。

## Rhai API

提供 `uia_window_state(element_ref)` 和 `uia_window_action(element_ref, action)`，动作集合以 wrapper 实际支持的方法为准，返回操作后的状态。

## 范围

只包含 Window Pattern。窗口发现、激活、截图和输入仍由现有 Desktop Driver 负责。

## 验收

- 能查询 Window Pattern 是否支持。
- 能读取 WindowVisualState、WindowInteractionState、IsModal 等可用信息。
- 支持的窗口状态操作返回真实结果。
- Pattern 不支持时返回明确错误。
- Notepad 和 File Explorer 的窗口元素按实际 Provider 支持情况检查；不将现有 `activate(id)` 成功当作 Window Pattern 验收。
