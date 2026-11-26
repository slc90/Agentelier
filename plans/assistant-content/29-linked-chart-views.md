# 让多个图表视图共享选定参数

当一个分析需要多个图表或数据表同时展示时，使用既有组合 Scene 放入不同视图。相较组合图，多视图各自有布局与坐标语义，仅显式绑定共同的数据选择、系列或坐标范围。Vega-Lite 的命名选择、legend binding 和 scale binding 是参考；此处仍由原生控件事件及本地参数机制驱动。

一个视图的选择变化通过共享参数更新依赖的其他图形/表格，不覆盖各自不关联的参数，也不反复触发循环。缩放共享的前提是轴意义相容；布局使用 BSN，保持回复归属和稳定子项身份。保存共用数据、每视图定义及必要的选择/缩放状态，恢复后保持相同关系。

## 验收

组合折线和散点或表格，实际点击选择/调整共享参数，BRP 检查依赖视图同步、独立视图不误变、无模型请求或反馈循环。重开 Session 后共享选择正确恢复。

## 与相邻方案的关系

依赖组合、绑定和至少两个已完成的数据视图；与同轴组合的展示机制不同。下一项开始数学表达式求值。

设计依据：[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)、[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
