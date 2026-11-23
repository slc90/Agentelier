# 建立 AgentelierButton 项目级基础按钮

## 目标

建立 Agentelier 自己的统一按钮控件，让主窗口和所有辅助窗口复用同一套尺寸、形状、颜色和交互反馈。页面不再分别组合 Widgetry Button 与项目样式。

## 设计

`AgentelierButton` 通过样式变体支持：

- 圆形图标按钮：B 区五个导航入口。
- 普通按钮：弹窗中的主要操作。
- 次要按钮：取消、返回和辅助操作。
- 危险按钮：删除、登出等有明确后果的操作。

所有变体共享 Normal、Hovered、Pressed、Focus、Disabled 状态反馈和尺寸规则。需要等待状态的场景使用统一 Loading 表现。图标和文字由调用方通过 BSN Scene / SceneList 提供，按钮不硬编码业务内容。

底层优先使用 Widgetry Button 的公开配置；需要完全控制外观时使用 `WidgetryButtonHeadless`，继续复用 Widgetry 的输入、焦点、禁用和激活契约。右键菜单不属于通用按钮行为，登录按钮副键菜单由 `LoginContextMenu` 单独负责。

## Widgetry 与 BSN

使用 `WidgetryButton` / `WidgetryButtonHeadless`、`WidgetryButtonLoading`、`WidgetryTheme` 和 `ColorOverrides`。控件需要稳定 ECS 身份和运行期状态时使用 `SceneComponent`；内容通过 BSN Scene Composition 传入。Props 只负责初始化样式，运行期状态由持久 Component 和 Widgetry 受管状态提供。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
- 未经用户明确授权，不执行登出、重新授权、切换账号或清理现有凭据的验收操作。
## 完成条件与验收

1. 圆形图标、普通、次要和危险四种变体都通过 `AgentelierButton` 使用，不需要页面直接设置 Widgetry 原始按钮样式。
2. B 区按钮保持圆形，图标居中，点击热区覆盖视觉按钮。
3. 四种样式的高度、文字对齐、边框和状态反馈一致。
4. Hover、Pressed、Focus、Disabled 和 Loading 状态使用 Agentelier 绿色体系，不残留 Widgetry 默认蓝色主色。
5. 主键点击只触发一次激活；副键不会触发普通按钮动作。
6. 获得焦点后，Enter / Space 遵循 Widgetry 激活行为。
7. Disabled 状态阻止用户激活；Loading 结束后只恢复 Loading 自己造成的禁用状态。
8. 运行时切换样式和主题不会重建调用方内容，不丢失焦点或子节点状态。
9. 在 UI Scale 1.0、1.25、1.5、2.0 下检查图标、文字、焦点环和命中区域。
10. B 区五个按钮和辅助窗口按钮均使用 `AgentelierButton`，不出现平行的页面级按钮样式。
11. 使用 BRP 实际执行鼠标 Hover、点击和键盘激活；通过截图与状态检查确认行为。
