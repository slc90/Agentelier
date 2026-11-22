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
| Invoke (10000) | `UIInvokePattern` | `uia_invoke` | 未观察 | 文件条目支持 | 通过，Accessibility Insights 的录制按钮触发并回读 |
| Selection (10001) | `UISelectionPattern` | `uia_selection` | 未观察 | 项目视图支持 | 通过，Explorer 回读被选中的 A-Child |
| Value (10002) | `UIValuePattern` | `uia_value_get / uia_value_set` | Edit 支持 | 项目视图支持 | 通过，记事本设置、回读并恢复原文 |
| RangeValue (10003) | `UIRangeValuePattern` | `uia_range_value_get / uia_range_value_set` | 未观察 | 未观察 | 未验证实际操作 |
| Scroll (10004) | `UIScrollPattern` | `uia_scroll` | Edit 支持 | 项目视图支持 | 通过，Explorer 垂直滚动到约 73.56% |
| ExpandCollapse (10005) | `UIExpandCollapsePattern` | `uia_expand_collapse` | 未观察 | 导航树节点支持 | 通过，临时窗口导航树节点折叠→展开→折叠 |
| Grid (10006) | `UIGridPattern` | `uia_grid_info / uia_grid_cell` | 未观察 | 详细信息列表支持 | 通过，两次回读 46×4、44×4；46 行时取得单元格 (45,0) |
| GridItem (10007) | `UIGridItemPattern` | `uia_grid_item_info` | 未观察 | 详细信息单元格支持 | 通过，回读单元格行列及跨度 |
| MultipleView (10008) | `UIMultipleViewPattern` | `uia_multiple_view` | 未观察 | 项目视图支持 | 通过，Explorer 枚举 8 种视图，5→4→5 |
| Window (10009) | `UIWindowPattern` | `uia_window_state / uia_window_action` | 窗口支持 | 窗口支持 | 记事本最小化、恢复及临时窗口关闭通过；输入空闲等待收到 Provider 错误 |
| SelectionItem (10010) | `UISelectionItemPattern` | `uia_select` | 未观察 | 目录和文件条目支持 | 通过，Explorer 选中临时条目并回读 |
| Dock (10011) | `UIDockPattern` | `uia_dock_position` | 窗口不支持 | 窗口不支持 | 受测窗口不支持 |
| Table (10012) | `UITablePattern` | `uia_table_info / uia_table_headers` | 未观察 | 详细信息列表支持 | 通过，回读 4 个列标题 |
| TableItem (10013) | `UITableItemPattern` | `uia_table_item_info` | 未观察 | 详细信息单元格支持 | 通过，回读单元格列标题 |
| Text (10014) | `UITextPattern` | `uia_text_document_range / uia_text_get / uia_text_find / uia_text_range_select` | Edit 不支持 | 未观察 | 通过，Accessibility Insights 的 WPF 编辑框全文、截断、查找 Beta、选区及范围移动 |
| Toggle (10015) | `UITogglePattern` | `uia_toggle` | Edit 不支持 | 未观察 | 记事本 Edit 返回不支持 |
| Transform (10016) | `UITransformPattern` | `uia_transform_capabilities / uia_transform` | 窗口支持 | 窗口支持，最大化时不可移动/缩放 | 通过，记事本实际移动和缩放，回读并恢复边界 |
| ScrollItem (10017) | `UIScrollItemPattern` | `uia_scroll_item` | 未观察 | 文件条目支持 | 通过，Explorer 离屏项回读 IsOffscreen=false |
| LegacyIAccessible (10018) | `UILegacyIAccessiblePattern` | `uia_legacy_accessible` | Edit 支持 | 项目视图支持 | 通过，记事本读取 MSAA 数据并写入、回读文本 |
| ItemContainer (10019) | `UIItemContainerPattern` | `uia_find_item` | 未观察 | 项目视图支持 | 通过，Explorer 定位尚未实例化的 item-100.txt |
| VirtualizedItem (10020) | `UIVirtualizedItemPattern` | `uia_realize` | 未观察 | 离屏占位文件支持 | 通过，条目同一引用由占位变为有效，代次 1→2 |
| SynchronizedInput (10021) | `UISynchronizedInputPattern` | `uia_synchronized_input` | 未观察 | 未观察 | 通过，Accessibility Insights WPF Edit 启动、回读、取消 key_down 监听 |
| ObjectModel (10022) | 无专用 wrapper；使用 `windows` 绑定适配 | `uia_object_model` | Edit 不支持 | 项目视图不支持 | 受测目标返回不支持；未观察到可取回底层对象的 Provider |
| Annotation (10023) | `UIAnnotationPattern` | `uia_annotation` | Edit 不支持 | 未观察 | 记事本 Edit 返回不支持 |
| Text2 (10024) | 集成于 `UITextPattern` wrapper | `uia_text_caret / uia_text_annotation_range` | 未观察 | 未观察 | 未验证实际操作 |
| Styles (10025) | `UIStylesPattern` | `uia_styles` | Edit 不支持 | 未观察 | 记事本 Edit 返回不支持 |
| Spreadsheet (10026) | `UISpreadsheetPattern` | `uia_spreadsheet_info` | 未观察 | 项目视图不支持 | Explorer 列表返回不支持 |
| SpreadsheetItem (10027) | `UISpreadsheetItemPattern` | `uia_spreadsheet_item_info` | 未观察 | 未观察 | 未验证实际操作 |
| Transform2 (10028) | 集成于 `UITransformPattern` wrapper | `uia_transform (zoom / zoom_by_unit)` | 窗口不支持 | 窗口不支持 | 受测窗口不支持 |
| TextChild (10029) | `UITextChildPattern` | `uia_text_child_range` | 未观察 | 未观察 | 未验证实际操作 |
| Drag (10030) | `UIDragPattern` | `uia_drag_info` | 窗口根不支持 | 窗口根不支持 | 受测窗口根返回不支持 |
| DropTarget (10031) | `UIDropTargetPattern` | `uia_drop_target_info` | 窗口根不支持 | 窗口根不支持 | 受测窗口根返回不支持 |
| TextEdit (10032) | `UITextEditPattern` | `uia_text_edit_state` | 未观察 | 未观察 | 未验证实际操作 |
| CustomNavigation (10033) | `UICustomNavigationPattern` | `uia_custom_navigation` | 窗口根不支持 | 窗口根不支持 | 受测窗口根返回不支持 |

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

### 实际运行记录

在 Windows x64 调试构建中，Agentelier 通过真实 `computer_execute` 控制独立的记事本临时文档和 Explorer 临时目录。临时目录含 A-Child、B-Child 以及用于滚动和实例化的 100 个编号文件。

| 场景 | 通过条件 | 实际观察 |
|---|---|---|
| 结果与窗口树 | 三种来源使用同一外层字段，三种树视图可分页读取 | `live`、`cache`、`snapshot` 三种来源均回读成功。记事本 Raw/Control/Content 共 27/27/15 节点，Explorer 临时目录共 133/127/115 节点。 |
| 记事本属性和文本 | 找到同一编辑控件，写入后回读并恢复原文，明确报告 Text 能力 | AutomationId 15 的 Edit 回读 Name、ClassName、边界、框架和焦点等属性。Value 和 LegacyIAccessible 两条路径验证了读写、恢复；本机 Edit 的 Text 返回 `uia_pattern_not_supported`。Accessibility Insights 的 WPF 编辑框验证了 TextRange 和选区。 |
| Explorer 列表和引用 | 三种视图可查找文件和目录，测试条目可选中、滚动及实例化，目录切换后验证旧引用 | 按属性组合找到临时条目，选择后回读；详细信息列表读取 4 个列标题。item-100.txt 从占位变为有效，随后滚动入视野。目录切换后实测旧引用不可用，返回原目录后实测唯一重定位。 |
| Accessibility Insights 对照 | 实时树可找到同一窗口和控件，Events 能记录实际操作 | 记事本的编辑区两端名称和边界一致，Live Inspect 命中的是 Document 投影，Agentelier 也验证了树中的 Edit 投影。Explorer 项目视图对照出 Selection、Grid、Table、MultipleView 与 ItemContainer。记事本改写期间记录到属性变化 20004；Explorer 目录切换期间记录到 StructureChanged，录制区间也观测到 SelectionItem_ElementSelected。 |
| 会话监听和等待 | 四类通知能跨脚本读取；取消订阅生效；超时和取消可区分 | 实测焦点 20005、结构 20002 与 RuntimeId、选择 20012、属性 20004 与 30045。Accessibility Insights 录制按钮的属性事件提供字符串值。取消后未交付旧订阅通知。Explorer 导航与实际焦点切换唤醒等待并重读状态；返回了 `uia_timeout` 和 `uia_cancelled`。工具始终以 Exclusive 接纳。 |
| 快照和事件丢失 | 更新后的窗口状态有新代次；遗漏事件使旧快照要求重建 | 记事本 ValueValue 变化触发增量更新，编辑引用保持一致；显式刷新编辑子树也保留节点关系。Explorer 导航触发重建并回读新标题。实际队列溢出后 `rebuild_required` 为 true，`auto` 完整重建并回读恢复后的文本。 |
| 关闭后的引用 | 临时窗口关闭后，根和子元素操作返回稳定错误 | 两个窗口的 Window Pattern 都接受关闭请求。记事本在保存提示中选择不保存后退出，Explorer 原 HWND 已消失。两个根引用和两个子元素引用的后续读取均返回 `ok=false` 和 `uia_window_unavailable`。 |

上表的通过记录限于实际观察到的 Provider 行为。缺少对应 Provider 的操作和未诱发的场景单独保留结果。未注入在途 COM 挂起、分页中途 Provider 故障或重复重定位候选。未单独验证无任何通知时条件从假转真，以及能独立定位到单个父节点的 ChildRemoved 刷新。
