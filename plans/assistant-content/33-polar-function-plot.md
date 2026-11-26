# 绘制极坐标函数

极坐标图以半径 r 和角度 θ 的关系描述几何，不将 θ 直接当成直角 X 坐标显示。提供 `r(θ)`、角域、单位/周期和参数定义，按明确的极坐标语义转换为显示坐标，绘制合适的极轴/刻度与实际曲线。负半径、跨周期、奇点和无效样本遵循所选表达式语义。数学计算使用已有求值器，不在排版引擎里实现绘图。

用户可通过参数改变玫瑰线等实际图形，复用本地绑定、原生绘图和 Session 状态，保留真正的公式与定义域。

## 验收

绘制圆形、玫瑰线、带负半径或不连续点的极坐标表达式；按输入角域核对实际几何和标签，BRP 调参、缩放及恢复。

## 与相邻方案的关系

依赖数学求值和坐标绘制，不依赖不等式区域；下一项为一般参数方程提供独立入口。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
