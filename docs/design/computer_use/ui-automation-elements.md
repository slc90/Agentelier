# UIA 元素模型：从窗口根节点到可重复定位的元素引用

UIA 把桌面和应用界面表示为元素树。元素包含可选属性、可选 Control Pattern 和子元素。Agentelier 需要把这棵树转换为模型能逐步探索的结构，同时控制读取成本和引用生命周期。

## 根元素与现有 WindowId

桌面根元素由 `UIAutomation::get_root_element()` 获取；指定窗口的根元素由 `element_from_handle()` 获取。窗口发现继续使用现有 `desktop::windows()`，不再实现另一套顶层窗口枚举。

现有 `WindowId` 包含 HWND、进程 ID 和线程 ID，并通过 token 字符串跨 Rhai 调用表示。UIA 窗口根元素请求接收该 token，在 Worker 内解析并重新核对窗口身份，再从 HWND 获取 UIA 元素。窗口关闭或 HWND 被其他窗口复用时，旧 token 不能继续代表新窗口。

## 三种树视图

`uiautomation` 提供 Raw、Control、Content 三种 TreeWalker：

- **Raw View**：Provider 暴露的底层 UIA 元素关系。
- **Control View**：面向可交互控件的元素关系。
- **Content View**：面向内容阅读的元素关系。

模型可以显式选择视图。常规控件定位默认使用 Control View；调查 Provider 暴露结构时使用 Raw View；读取文档或内容时使用 Content View。

## 有界遍历与继续读取

UIA 的 `UITreeWalker` 提供 parent、first child、last child、next sibling、previous sibling 和 normalize，也提供对应的 BuildCache 方法。它没有内建分页游标。

Agentelier 应在 UIA Worker 中实现有界 DFS 遍历。请求至少包含：

- 根元素引用。
- 视图类型。
- 最大深度。
- 最大节点数。
- 每节点属性集。
- 可选继续读取游标。

继续读取游标由 Worker 保存当前父元素、当前子元素/后续兄弟位置、视图和快照代次。控件树结构变化后，旧游标应返回 `uia_tree_cursor_stale`，而不是继续沿已改变的树猜测位置。

不要直接使用 `UITreeWalker::get_children()` 作为需要精确错误语义的底层遍历：该 helper 返回 `Option<Vec<UIElement>>`，并把读取第一个子节点或后续兄弟时的错误折叠成 `None`/结束。正式遍历应逐步调用 `get_first_child` 和 `get_next_sibling`，区分正常结束、元素失效和 Provider 错误。

## 元素属性

常用属性摘要包含：

| 属性 | 作用 |
|---|---|
| Name | 可访问名称 |
| AutomationId | Provider 提供的稳定性有限的控件标识 |
| ControlType | 控件类别 |
| ClassName | Provider/框架类名 |
| BoundingRectangle | UIA 元素边界 |
| IsEnabled | 是否启用 |
| IsOffscreen | 是否在屏幕外 |
| HasKeyboardFocus | 是否持有键盘焦点 |
| IsKeyboardFocusable | 是否可获得键盘焦点 |
| FrameworkId | Win32、WPF、WinUI 等框架标识 |
| RuntimeId | 当前 UIA 元素运行时标识 |
| ProcessId | 所属进程 |
| NativeWindowHandle | 关联 HWND（若有） |
| LocalizedControlType | 本地化控件类型 |
| HelpText | 帮助文本 |
| AccessKey | 访问键 |
| AcceleratorKey | 快捷键 |

按需属性可扩展到 IsPassword、IsControlElement、IsContentElement、ItemStatus、ItemType、LabeledBy、ProviderDescription、HeadingLevel 和 IsDialog 等。缺失属性应表达为缺失/不可用，不把它伪装成空字符串或默认值。

## 查找条件

`UIAutomation` 提供 `create_property_condition`、`create_and_condition`、`create_or_condition` 和 `create_not_condition`；`UIElement` 提供 `find_first`、`find_all` 及 BuildCache 变体。可用条件组合 Name、AutomationId、ControlType、ClassName、ProcessId 等属性。

`UIMatcher` 是便利筛选器，支持 Raw/Control/Content 模式、起始元素、深度、超时、间隔、Name、ClassName、ControlType 和自定义过滤函数；其默认深度为 7、默认超时为 3000ms、默认重试间隔为 100ms。当前实现没有专门的 AutomationId builder，因此 AutomationId 应使用 `UIProperty::AutomationId` 创建 property condition，或通过自定义过滤器读取属性，不能在设计中声称 `UIMatcher.automation_id()` 已存在。

对于小范围精确条件优先使用 UIA 原生条件与 FindFirst/FindAll。FindAll 会一次返回所有匹配元素，不能只在结果序列化时截断就宣称底层读取有界。大范围查找应使用 Agentelier 自己的有界 TreeWalker 搜索，设置最大节点数、深度和结果数。

## 逻辑元素引用

Rhai 只保存不透明的字符串引用，例如 `uia:element:<id>`。真正的 `UIElement` COM wrapper 保存在所属 MTA Worker 的元素表中。

每条引用记录：

- 会话内唯一 ID。
- 所属 `WindowId` token。
- 引用代次。
- 当前 UIA RuntimeId（若可读）。
- 初始发现时的父引用/搜索范围。
- 可用于重新查找的属性条件与定位上下文。
- 最近一次轻量元素摘要。
- 当前状态（有效、待验证、失效、歧义）。

RuntimeId 只作为当前元素的辅助身份信息，不能作为跨窗口重建或长期稳定身份。AutomationId 也不保证存在或唯一；Name 可能动态变化或本地化。

每次操作前由 Worker 解析逻辑引用。底层元素可用时继续使用；若 Provider 返回元素不可用，则使用保存的搜索范围和条件重新定位。只有唯一匹配才更新引用代次。零匹配返回失效，多匹配返回歧义，模型重新读取树并选择目标。

## 元素引用与快照的关系

元素引用代表“某个会话当前持有的 UIA 元素对象及其重新定位线索”；树快照代表“某个时刻读取的有限元素投影”。两者不能混为一谈：

- 快照更新不必让所有引用立即失效。
- 结构变化会使受影响区域的快照失效，并可能使该区域内引用需要重新验证。
- 属性变化只使相应属性缓存失效。
- 窗口关闭或重建使该窗口下的全部引用失效。
- UIA 元素对象仍在 Worker 表中，不代表 Provider 一定仍能访问它。

## 来源

- [uiautomation core.rs：UIElement、查找、RuntimeId、属性与 Pattern](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/core.rs)
- [uiautomation core.rs：TreeWalker、Cache Request、Matcher](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/core.rs)
- [Microsoft UI Automation Tree Overview](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-treeoverview)
