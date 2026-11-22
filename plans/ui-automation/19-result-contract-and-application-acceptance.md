# 统一 UIA 结果契约并完成 Notepad / File Explorer 验收

## 目标与产出

统一所有 UIA Rhai API 的模型可见结果和错误语义，完成跨能力工作流验证，并使用 Windows Notepad、Windows File Explorer 和已安装的 Accessibility Insights for Windows 进行最终验收。

本方案包含可交付的结果契约收口，不是独立的纯验收方案。

## 统一结果契约

所有 UIA API 使用一致外层结构：

- `ok`：请求是否成功。
- `data`：成功结果。
- `error.code` / `error.message`：失败分类和说明。
- `source`：live、cache 或 snapshot。
- `generation`：涉及快照时的代次。

统一元素摘要、树页、查找候选、Pattern 操作结果、事件 DTO 和等待结果字段。结果受现有 ToolPayload 文本预算限制；树和属性必须有界。UIA 结果不携带 COM 对象，也不把图片转为 Base64。截图仍由现有 Computer API 返回。

## Notepad 端到端验收

使用独立临时文档，例如 `%TEMP%/Agentelier-UIA-Acceptance/notepad-probe.txt`，不覆盖用户现有文件。验收结束只清理本次创建且路径明确的临时文件。

1. 打开 Notepad 并创建临时文档，内容为 `Alpha Beta Gamma`。
2. 通过现有窗口枚举获取 WindowId，再获取 UIA 窗口根元素。
3. 分别读取 Raw、Control、Content 视图，定位文本编辑元素。
4. 读取 Name、AutomationId、ControlType、ClassName、FrameworkId、BoundingRectangle、IsEnabled、焦点等属性。
5. 查询 Text、Text2、TextEdit、Value 等 Pattern 的实际支持情况。
6. 若 Text Pattern 可用，读取 DocumentRange 全文，预期文本为 `Alpha Beta Gamma`。
7. 使用 TextRange.FindText 定位 `Beta`，选择该范围并读取 Selection，核对文本和范围。
8. 若 Value Pattern 可写，设置为 `Alpha Delta Gamma` 并回读；若不可写，明确记录 Provider 不支持，再使用现有 Computer Use 键盘输入验证两种交互通道的协作，不把键盘输入计为 UIA Pattern 成功。
9. 通过 Accessibility Insights Live Inspect 对照编辑元素的属性和 Pattern。
10. 通过 Accessibility Insights Events 视图记录焦点/文本相关事件，并对照 Agentelier 事件队列。
11. 修改文本后刷新快照，确认缓存代次和文本结果变化。
12. 关闭窗口后验证根元素和子元素引用失效。

## File Explorer 端到端验收

使用独立临时目录，例如 `%TEMP%/Agentelier-UIA-Acceptance/Explorer/`，创建 `A-Child`、`B-Child` 等子目录和一批编号文件（例如 `item-001.txt` 至 `item-100.txt`），以便观察滚动和虚拟化；不操作用户的重要文件。验收结束只清理本次创建且路径明确的临时目录。

准备包含多个子目录和足够多文件的目录结构，使导航树、文件列表和滚动/虚拟化路径都能被观察。

1. 获取 Explorer 窗口 UIA 根元素。
2. 读取 Raw、Control、Content 三种视图。
3. 识别导航树、文件列表和详细信息视图。
4. 按 Name、AutomationId、ControlType 组合条件查找目标目录和文件。
5. 在 Provider 支持时使用 ExpandCollapse 展开/折叠目录。
6. 使用 Selection/SelectionItem 选择目标文件。
7. 查询 Scroll/ScrollItem、ItemContainer、VirtualizedItem 的实际支持情况；支持时执行滚动、定位和 Realize。
8. 查询 Grid、GridItem、Table、TableItem 的实际支持情况；支持时读取行列和单元格信息。
9. 切换目录并检查旧元素引用是否失效或重新定位。
10. 通过 Accessibility Insights Live Inspect 对照导航树和文件列表的属性/Pattern。
11. 使用 Accessibility Insights Events 视图记录展开、选择、目录切换相关事件，并对照 Agentelier 事件结果。
12. 刷新快照，确认结构变化和缓存代次更新。
13. 关闭窗口后验证引用失效。

## Accessibility Insights 操作

- Live Inspect：查看 UIA Tree、Raw/Control/Content 视图、属性和 Pattern。
- Pattern Actions：对目标元素执行对应 Pattern 动作，观察返回结果。
- Events：在元素菜单选择 Listen to Events，配置所需事件并开始记录；对目标应用执行操作，再与 Agentelier 事件结果对照。

Accessibility Insights 的事件功能在 Troubleshooting / Events 视图中，不把它误写成一个独立外部事件服务。

## Pattern 完整性矩阵

最终逐项填写全部 UIA Pattern ID。`uiautomation` wrapper 状态按 0.25.3 实际源码记录；Text2/Transform2 由对应 wrapper 内部扩展接口提供，ObjectModel 需要 Agentelier 适配层。

| Pattern ID | 上游 Rust 封装状态 | Agentelier API | Notepad Provider | Explorer Provider | 操作验证 |
|---|---|---|---|---|---|
| Invoke | `UIInvokePattern` | 记录 | 支持/不支持/未观察 | 支持/不支持/未观察 | 通过/失败/不适用 |
| Selection | `UISelectionPattern` | 记录 | 同上 | 同上 | 同上 |
| Value | `UIValuePattern` | 记录 | 同上 | 同上 | 同上 |
| RangeValue | `UIRangeValuePattern` | 记录 | 同上 | 同上 | 同上 |
| Scroll | `UIScrollPattern` | 记录 | 同上 | 同上 | 同上 |
| ExpandCollapse | `UIExpandCollapsePattern` | 记录 | 同上 | 同上 | 同上 |
| Grid | `UIGridPattern` | 记录 | 同上 | 同上 | 同上 |
| GridItem | `UIGridItemPattern` | 记录 | 同上 | 同上 | 同上 |
| MultipleView | `UIMultipleViewPattern` | 记录 | 同上 | 同上 | 同上 |
| Window | `UIWindowPattern` | 记录 | 同上 | 同上 | 同上 |
| SelectionItem | `UISelectionItemPattern` | 记录 | 同上 | 同上 | 同上 |
| Dock | `UIDockPattern` | 记录 | 同上 | 同上 | 同上 |
| Table | `UITablePattern` | 记录 | 同上 | 同上 | 同上 |
| TableItem | `UITableItemPattern` | 记录 | 同上 | 同上 | 同上 |
| Text | `UITextPattern` | 记录 | 同上 | 同上 | 同上 |
| Toggle | `UITogglePattern` | 记录 | 同上 | 同上 | 同上 |
| Transform | `UITransformPattern` | 记录 | 同上 | 同上 | 同上 |
| ScrollItem | `UIScrollItemPattern` | 记录 | 同上 | 同上 | 同上 |
| LegacyIAccessible | `UILegacyIAccessiblePattern` | 记录 | 同上 | 同上 | 同上 |
| ItemContainer | `UIItemContainerPattern` | 记录 | 同上 | 同上 | 同上 |
| VirtualizedItem | `UIVirtualizedItemPattern` | 记录 | 同上 | 同上 | 同上 |
| SynchronizedInput | `UISynchronizedInputPattern` | 记录 | 同上 | 同上 | 同上 |
| ObjectModel | 无专用 wrapper；使用 `windows` 绑定适配 | 记录 | 同上 | 同上 | 同上 |
| Annotation | `UIAnnotationPattern` | 记录 | 同上 | 同上 | 同上 |
| Text2 | 集成于 `UITextPattern` wrapper | 记录 | 同上 | 同上 | 同上 |
| Styles | `UIStylesPattern` | 记录 | 同上 | 同上 | 同上 |
| Spreadsheet | `UISpreadsheetPattern` | 记录 | 同上 | 同上 | 同上 |
| SpreadsheetItem | `UISpreadsheetItemPattern` | 记录 | 同上 | 同上 | 同上 |
| Transform2 | 集成于 `UITransformPattern` wrapper | 记录 | 同上 | 同上 | 同上 |
| TextChild | `UITextChildPattern` | 记录 | 同上 | 同上 | 同上 |
| Drag | `UIDragPattern` | 记录 | 同上 | 同上 | 同上 |
| DropTarget | `UIDropTargetPattern` | 记录 | 同上 | 同上 | 同上 |
| TextEdit | `UITextEditPattern` | 记录 | 同上 | 同上 | 同上 |
| CustomNavigation | `UICustomNavigationPattern` | 记录 | 同上 | 同上 | 同上 |

TextRange 是 Text Pattern 返回的范围对象，不是独立 Pattern ID。SelectionPattern2 是 Selection 的扩展接口，不另列一个 Pattern ID。

应用不支持某个 Pattern 不代表 Agentelier 接入失败；Agentelier 必须准确报告不支持。API 注册成功也不能替代真实操作验证。

## 通用生命周期验收

- MTA Worker 初始化和退出。
- COM 对象仅在 Worker 线程使用和释放。
- ScriptSession 跨工具调用复用 Worker。
- 会话关闭后事件处理器、元素引用和缓存全部释放。
- 事件订阅可取消，队列溢出会触发快照失效。
- 条件等待支持超时和取消。
- 元素失效、歧义和 Provider 错误分别返回。
- UIA 与鼠标键盘操作复用现有 Exclusive 许可。
- 等待期间许可保持与现有同步脚本行为一致。
- 大型树、查找结果、文本范围和事件队列均有界。
- 不增加独立权限审批、沙箱或安全审查机制。

## 结果记录

每项验收记录前置条件、目标应用、操作场景、预期行为、实际观察、通过条件和失败条件。状态区分通过、失败、应用不支持、环境不满足、未验证。不得将未执行的操作写成通过。

遵守 Agentelier 项目规则：不编写任何 Unit Test、Integration Test、Regression Test、Benchmark 或 Example。使用实际运行的 Agentelier、Notepad、File Explorer、Accessibility Insights、日志和必要调试信息完成验证。BRP 只用于 Agentelier 自身 Bevy GUI 行为变化；本次 UIA 目标应用不是 Bevy App，不使用 BRP 代替 UIA 验收。
