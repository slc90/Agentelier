# 补充 ObjectModel Pattern 适配

## 目标与产出

补齐 `uiautomation` 0.25.3 未提供专用 Rust wrapper 的 ObjectModel Pattern，使完整 Pattern 能力面不因上游封装缺失而遗漏。

## 设计

当前 `UIPatternType` 枚举包含 ObjectModel Pattern ID，但 `patterns.rs` 没有 `UIObjectModelPattern` wrapper。Windows SDK metadata 中存在 `IUIAutomationObjectModelPattern` 接口，其核心操作为取得 Provider 暴露的 underlying object model。

使用 Agentelier 已有 `windows` 0.62 绑定补一个窄适配层：从 UIElement 获取 ObjectModel Pattern ID 对应接口，再 cast 到 `IUIAutomationObjectModelPattern`。适配层只封装该 Pattern 必要的 COM 操作和结果转换，不复制 `uiautomation` 的其他 Pattern 体系。

若当前 `windows` crate 版本缺少该接口或其依赖类型，先核对生成 API 与 SDK metadata；只有确实缺失时才记录为上游绑定缺口，不自行创建另一套 Windows 绑定依赖。

## Rhai API

提供 `uia_object_model(element_ref)`，返回 underlying object model 的可用状态和 Agentelier 能安全表示的普通结果。不得把任意 IUnknown/COM 对象暴露给 Rhai。

## 范围

只补充 ObjectModel Pattern，不重写 `uiautomation`，不引入其他依赖。

## 验收

- 能查询 ObjectModel Pattern ID 和接口是否可用。
- Provider 支持时可取得 underlying object model。
- Provider 不支持时返回 `uia_pattern_not_supported`。
- 结果不包含 COM 指针或 COM wrapper。
- 若目标应用不提供 ObjectModel，Notepad/File Explorer 记录为应用不支持；接口适配本身通过类型和运行时调用检查验证。
