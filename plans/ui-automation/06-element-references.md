# 实现跨调用逻辑元素引用与失效恢复

## 目标与产出

使模型能够在同一会话的多次 `computer_execute` 调用之间引用已发现的元素，同时确保 Rhai 不持有 UIA COM 对象。

## 设计

Rhai 只保存不透明 ElementRef 字符串。UIA Worker 的元素表保存实际 `UIElement` wrapper、WindowId、RuntimeId、引用代次、发现时的搜索范围和可重定位条件。

RuntimeId 只作为当前元素的辅助身份，不作为永久 ID。AutomationId 和 Name 也不能单独保证唯一。重定位使用父级范围与多个属性条件组合，必要时结合元素树路径。

每次 Pattern 操作前，Worker 先核对 WindowId，再检查元素当前可访问性。元素不可用时按保存条件重新定位：
- 唯一匹配：更新底层元素并递增引用代次。
- 零匹配：返回 `uia_element_unavailable`。
- 多匹配：返回 `uia_ambiguous_target`，要求模型重新读取树。
- 窗口关闭/重建：使窗口下引用整体失效。

引用表按 ScriptSession 生命周期管理，并设置数量上限。会话结束时全部释放。

## Rhai API

- `uia_ref_info(element_ref)`：读取引用状态。
- `uia_refresh_element(element_ref)`：重新验证并尝试重定位。
- 所有后续树、属性和 Pattern API 均接受 ElementRef。

## 范围

包含引用 ID、会话和窗口绑定、RuntimeId 辅助识别、定位描述、有效性检查、唯一重定位和资源回收。不包含 UIA 树快照的完整增量更新。

## 验收

- ElementRef 可跨多次 `computer_execute` 使用。
- Rhai 变量中只有普通引用字符串，不包含 UIElement/Pattern wrapper。
- 引用不能跨 ScriptSession 使用。
- 元素仍有效时可继续操作。
- 目标元素失效后唯一重定位成功并更新代次。
- 零匹配、多匹配和窗口失效返回不同结果。
- Notepad 文本编辑区域和 Explorer 文件/文件夹元素可验证引用在文本变化、目录切换后的行为。
