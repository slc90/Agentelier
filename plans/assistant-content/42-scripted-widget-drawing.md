# 让 Rhai 自定义控件生成和更新图形

真正的运行时新控件需要表达预设 Scene 无法覆盖的外观。将已经建立的 Bevy/Widgetry 原生绘图能力接入 ScriptedWidget，脚本可描述点、线、路径、圆/椭圆、矩形、文字以及必要的颜色、坐标、裁剪和图形标记。Rust 使用公开绘图 API 负责实际资源和渲染；可复用此前数据图表、几何图形的路径和坐标基础，不引入新的 WebView 或通用 UI DSL。

脚本描述属于当前控件的语义绘图数据，可以更新对应图形并保持其逻辑身份；批量绘制通过必要的数据更新交付，避免每个采样点执行一次同步 ECS 请求。实际图形资源仍属于 Bevy World 和场景拥有关系。程序的正常绘制不改 Widgetry 私有状态，既有通用 ECS 操作及通信边界保持不变。

举例：Agent 在未注册抛体运动模拟器完整模板时，自行定义初速度、角度、重力和进度，用 Rhai 计算轨迹、当前位置及射程，再绘制轨迹和质点；Slider 触发自定义回调而非固定物理 Rust 类型。此步先支持用户操作驱动的变化，连续时间模拟随后接入。脚本及数据定义保存，局部绘制失败不销毁其他内容，保留最近有效图形。

## 验收

由真实 Root 编写自定义抛体轨迹或其他未预设图形程序；核对数值计算与实际 Bevy 图形，BRP 拖动参数/进度，图形原位更新。检查不支持图形数据的局部失败、缩放裁剪、资源归属和切换清理。

## 与相邻方案的关系

依赖自定义回调及此前实际绘图能力；下一项增加由时间驱动的变化。

设计依据：[Rhai 自定义控件](../../docs/design/assistant-content/06-scripted-widgets.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)、[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
