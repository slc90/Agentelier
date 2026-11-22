# 读取元素属性并使用 UIA Cache Request

## 目标与产出

提供常用和扩展 UIA 属性读取，并用 Cache Request 批量预取属性与 Pattern，减少重复跨进程调用。

## 设计

常用属性集至少包含 Name、AutomationId、ControlType、ClassName、BoundingRectangle、IsEnabled、IsOffscreen、HasKeyboardFocus、IsKeyboardFocusable、FrameworkId 和 RuntimeId。

扩展属性按需读取 ProcessId、NativeWindowHandle、LocalizedControlType、HelpText、AccessKey、AcceleratorKey、IsPassword、IsControlElement、IsContentElement、ItemStatus、ItemType、LabeledBy、ProviderDescription、HeadingLevel 和 IsDialog 等。

`UICacheRequest` 支持添加 Property 和 Pattern，配置 TreeScope、TreeFilter 和 AutomationElementMode。`UIElement` 提供 Current/Cached 属性读取和 `build_updated_cache()`。缓存来源必须明确区分 live 与 cached；Provider 未提供的属性保留缺失状态。

## Rhai API

- `uia_properties(element_ref, property_names)`：读取指定属性。
- `uia_cached_properties(element_ref, property_names)`：读取当前元素已有缓存属性。
- `uia_prefetch(element_ref, property_names, pattern_names, view, scope)`：建立 Cache Request 并获取带缓存元素。
- `uia_patterns(element_ref)`：作为后续 Pattern 方案的公共查询入口，在本方案先建立返回数据契约。

## 范围

包含属性枚举映射、Variant 转换、Cache Request 构建、属性和 Pattern 预取及缓存/实时来源标记。不包含 Agentelier 控件树快照缓存。

## 验收

- 常用属性逐项可读。
- 扩展属性按需读取。
- 不存在的属性明确表示缺失，不伪造默认值。
- Cache Request 能预取指定属性和 Pattern。
- cached 与 live 读取可区分。
- `build_updated_cache()` 能刷新单个元素缓存。
- Accessibility Insights Live Inspect 对照 Name、AutomationId、ControlType、ClassName、BoundingRectangle 和 Pattern 列表。
