# UIA Control Pattern：按 Provider 实际能力操作控件

Control Type 描述元素是什么类型；Control Pattern 描述元素提供什么行为。一个元素可以支持多个 Pattern，同一 Pattern 也可能出现在不同 Control Type 上。Agentelier 应先查询目标元素的 Pattern 可用性，再执行对应操作。

`UIElement::get_pattern<T>()` 通过 Pattern 类型取得当前 Pattern；缓存模式可用 `get_cached_pattern<T>()`。Pattern 不可用应返回 `unsupported_pattern`，不能根据 ControlType 推断某 Pattern 必然存在。

## uiautomation 0.25.3 的 Pattern 覆盖

当前上游 `patterns.rs` 提供以下 wrapper：

| Pattern | Rust wrapper | 主要用途 |
|---|---|---|
| Invoke | `UIInvokePattern` | 调用按钮、菜单项等 |
| Value | `UIValuePattern` | 读取/设置单值 |
| Toggle | `UITogglePattern` | 切换复选框等状态 |
| RangeValue | `UIRangeValuePattern` | 数值范围、滑块 |
| Selection | `UISelectionPattern` | 读取容器选择 |
| SelectionItem | `UISelectionItemPattern` | 选择/取消选择项目 |
| ExpandCollapse | `UIExpandCollapsePattern` | 展开、折叠 |
| Scroll | `UIScrollPattern` | 容器滚动 |
| ScrollItem | `UIScrollItemPattern` | 将项目滚入视口 |
| ItemContainer | `UIItemContainerPattern` | 按属性在容器中查找项目 |
| VirtualizedItem | `UIVirtualizedItemPattern` | 实例化虚拟项目 |
| Text | `UITextPattern` | 文本内容、范围和选择 |
| Text2 | `UITextPattern` 内部扩展接口 | TextPattern2 扩展能力 |
| TextEdit | `UITextEditPattern` | 文本编辑状态 |
| TextChild | `UITextChildPattern` | 文本子元素关系 |
| Grid | `UIGridPattern` | 网格行列信息和单元格 |
| GridItem | `UIGridItemPattern` | 网格单元格位置 |
| Table | `UITablePattern` | 表格与表头 |
| TableItem | `UITableItemPattern` | 表格项目与行列头 |
| Spreadsheet | `UISpreadsheetPattern` | 电子表格能力 |
| SpreadsheetItem | `UISpreadsheetItemPattern` | 单元格公式和值 |
| Window | `UIWindowPattern` | 窗口状态和关闭 |
| Transform | `UITransformPattern` | 移动、缩放、旋转 |
| Transform2 | `UITransformPattern` 内部扩展接口 | TransformPattern2 扩展能力 |
| Dock | `UIDockPattern` | 停靠状态 |
| MultipleView | `UIMultipleViewPattern` | 多视图切换 |
| Annotation | `UIAnnotationPattern` | 注释信息 |
| Styles | `UIStylesPattern` | 样式属性 |
| SynchronizedInput | `UISynchronizedInputPattern` | 同步输入通知 |
| LegacyIAccessible | `UILegacyIAccessiblePattern` | MSAA 兼容 |
| ObjectModel | 当前无专用 wrapper | `UIPatternType` 有 ID；需通过现有 `windows` 绑定补适配 |
| Drag | `UIDragPattern` | 拖动源信息和动作 |
| DropTarget | `UIDropTargetPattern` | 放置目标信息和动作 |
| CustomNavigation | `UICustomNavigationPattern` | Provider 自定义导航 |

### 上游 wrapper 与接口版本

- `UIPatternType` 还包含 `Text2`、`Transform2` 和 `ObjectModel` 等 Pattern ID。
- `UITextPattern` wrapper 内部通过 QueryInterface/cast 使用 `IUIAutomationTextPattern2` 的扩展方法；`UITransformPattern` 同样封装 TransformPattern2；`UISelectionPattern` 包含 SelectionPattern2 的扩展访问。Text2、Transform2 是独立 Pattern ID，但其扩展方法已集成在对应 wrapper 中；SelectionPattern2 是 Selection 接口的扩展版本，不另列一个基础 Pattern ID。
- `UITextRange` 是 Text Pattern 返回的范围对象，不是独立 Control Pattern。
- 当前 `patterns.rs` 没有 `UIObjectModelPattern` wrapper。`UIPatternType::ObjectModel` 枚举值存在，Windows SDK metadata 中存在 `IUIAutomationObjectModelPattern`。Agentelier 需要用现有 `windows` 绑定补一个很薄的适配层，或在确认目标场景不需要时明确保留为未接入项；不能把它写成上游已有完整 wrapper。

## Pattern 操作契约

### 基础交互

基础操作结果包含 `ref`、`pattern`、`operation` 和 Provider 回读的 `data`。`uia_toggle(ref, false)` 仅读取状态，`uia_toggle(ref)` 切换后回读。选择项使用 `select`、`add` 或 `remove`，展开折叠使用 `state`、`expand` 或 `collapse`。Value 和 RangeValue 设置前检查只读状态，RangeValue 还检查 Provider 的范围。

Invoke、Value、Toggle、RangeValue、Selection、SelectionItem 和 ExpandCollapse 是第一组模型高频操作。

- Invoke 只表示 Provider 接受/完成 Invoke 调用，不推断业务流程已经完成。
- Value 读取和设置后回读实际值；只读元素应返回 Pattern 不支持或操作失败。
- Toggle 返回 ToggleState，而不是简单返回“调用成功”。
- RangeValue 同时读取当前值、最小值、最大值、步长和只读状态；设置前检查范围。
- Selection 返回当前选中元素摘要；SelectionItem 明确 Select、AddToSelection、RemoveFromSelection 语义。
- ExpandCollapse 返回实际 ExpandCollapseState，允许 Provider 不支持某个方向。

### 滚动和虚拟化

`uia_scroll(ref)` 回读方向、滚动百分比和视口比例。传入两个方向的滚动量时，操作完成后读取实际状态。`uia_scroll_item(ref)` 汇报操作后的屏幕边界与不可见标记。容器查找可以从第一项或给定项目之后开始。虚拟项目使用会话内的有界引用，Realize 后重新读取属性和可用 Pattern。原 COM 对象已经失效时，只在同一容器和属性条件下存在唯一候选时恢复引用。

Scroll、ScrollItem、ItemContainer 和 VirtualizedItem 常用于大型列表。ItemContainer 按属性查找项目；VirtualizedItem 的 Realize 后应重新读取元素属性和 Pattern。不能假设 Realize 之后原 COM element 引用仍是完整、可用的对象。

### 文本

文本范围是会话内字符串引用，绑定创建时的元素代次。元素重定位后，原范围不能用于新对象。每次读取须给定 1 到 4096 个字符的预算。查找返回新的匹配范围，移动、扩展、选择和滚动作用于调用指定的范围。文本属性结果区分 Provider 缺失、范围内混合值和有效值。

Text Pattern 返回 DocumentRange、Selection 和 VisibleRanges 等文本范围。TextRange 支持读取文本、查找子文本、移动端点、扩展范围、选择、滚入视口及读取文本属性。Text Pattern 本身主要用于读取/导航；文本修改应使用元素支持的 Value Pattern 或现有 Computer Use 输入能力，不要把 TextRange.Select 误写成“设置文本”。

### 结构化数据

Grid 单元格按从零开始的行列索引逐个查询，Worker 先核对 Provider 报告的范围。Table 和 TableItem 的标题返回逻辑引用，结果保留原数量和截断标记。Spreadsheet 只有命名单元格查询，行列规模仍从实际支持的 Grid Pattern 取得。SpreadsheetItem 返回公式和注释，单元格值由独立的 Value Pattern 提供。

Grid/GridItem 提供行列数和单元格索引；Table/TableItem 进一步提供行列标题；Spreadsheet/SpreadsheetItem 提供公式栏、活动单元格、公式和值等 Provider 能力。所有行列和单元格访问都必须有界，不能默认枚举整个大型表格。

### 窗口与变换

Window Pattern 可请求还原、最大化、最小化、关闭和等待输入空闲。可视状态操作回读实际状态并标明是否达到目标。关闭结果分别记录 Provider 是否接受请求、原 Win32 窗口身份是否已消失，以及 UIA 元素是否还能读取。 `topmost` 来自 Window Pattern，`top_level` 来自当前顶层窗口身份。

Transform 先核对对应的 Provider 能力，再用 UIA 原生屏幕坐标、尺寸和角度执行。Transform2 的绝对缩放值须落在 Provider 报告的最小与最大比例内，支持按相对单位缩放。结果回读真实边界和缩放比例。Dock 可读取或请求 `top`、`left`、`bottom`、`right`、`fill`、`none`。

Window Pattern 提供窗口关闭、最大化、最小化、恢复等状态操作。Transform/Transform2 提供元素移动、缩放、旋转和缩放单位等能力。Dock 提供停靠位置。操作后应读取状态确认，而不是只看 COM 方法返回成功。

### 拖放与自定义导航

Drag 和 DropTarget 返回 Provider 报告的放置效果，Drag 还返回是否抓取及当前项目。CustomNavigation 返回指定方向的元素引用，方向没有目标时返回空值。每个会话最多持有一个同步输入请求，`status` 报告会话是否持有该请求。Provider 可在收到匹配输入后自行结束监听。脚本可显式取消请求，Worker 在关闭时回收仍持有的请求。

Drag、DropTarget、CustomNavigation 和 SynchronizedInput 面向拖放和自定义交互。它们不应自动替代现有鼠标轨迹：Pattern 提供的是 Provider 暴露的语义能力，Computer Use 仍负责没有对应 Pattern 的通用物理输入。

### 兼容与元数据

LegacyIAccessible 用于传统 MSAA 信息和操作；MultipleView、Annotation、Styles 提供额外视图/注释/样式信息。按 Provider 实际支持情况读取，不把这些属性当作所有控件都必有。

## 错误语义

所有 Pattern 操作至少区分：

- `uia_pattern_not_supported`：元素没有提供该 Pattern。
- `uia_element_unavailable`：目标元素已失效。
- `uia_invalid_argument`：参数类型或范围不符合 Pattern 契约。
- `uia_provider_error`：Provider 返回其他失败。
- `uia_timeout` / `uia_cancelled`：等待或执行期限/取消结果。

不支持 Pattern 是目标应用的能力事实，不是 Agentelier 的接口故障。反过来，Pattern ID 已注册也不等于目标元素支持该 Pattern。

## 来源

- [uiautomation patterns.rs](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/patterns.rs)
- [uiautomation core.rs：get_pattern / get_cached_pattern](https://github.com/leexgone/uiautomation-rs/blob/main/crates/uiautomation/src/core.rs)
- [Microsoft Control Patterns Overview](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controlpatternsoverview)
