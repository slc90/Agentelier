# 接入窗口与桌面 UIA 根元素

## 目标与产出

让 UIA Runtime 能够获取桌面根元素和现有窗口的 UIA 根元素，并将窗口身份与现有 `WindowId` 统一。

## 设计

桌面根元素通过 `UIAutomation::get_root_element()` 获取。指定窗口通过 `UIAutomation::element_from_handle()` 获取。

窗口发现继续使用现有 `desktop::windows()`，不新建顶层窗口枚举。Rhai 传入 `windows()` 返回的 `id` token；Worker 使用 `WindowId::from_token()` 解析，并在 UIA 调用前重新核对 HWND、ProcessId 和 ThreadId。句柄被复用时旧 token 必须失效。

根元素由 Worker 注册为逻辑元素引用并返回普通摘要。此阶段先建立根引用所需的最小注册能力；完整的元素引用失效恢复由方案 06 扩展。

## Rhai API

- `uia_desktop_root()`：获取桌面根元素引用。
- `uia_window_root(window_id)`：获取指定 WindowId 对应的窗口根元素引用。
- `uia_root_info(element_ref)`：读取根元素摘要和 Provider 可访问状态。

## 范围

包含桌面根元素、窗口根元素、WindowId 复用和基础引用注册。不重复实现窗口枚举、激活、截图或坐标操作。

## 验收

- 桌面根元素可获取。
- 现有 `windows()` 返回的窗口 ID 可用于获取 UIA 根元素。
- 窗口 token 与实际 HWND/进程/线程身份不一致时返回 `uia_window_unavailable`。
- 窗口关闭后旧根引用不能继续操作。
- Provider 不可访问时返回明确错误，不把空树伪装为成功。
- Notepad 和 File Explorer 的窗口根元素均可通过 UIA 获取，并能在 Accessibility Insights Live Inspect 中定位同一窗口。
