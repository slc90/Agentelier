# 提供原生分类柱状图

分类图不是把类别名强行转换为数值坐标。注册柱状图 Scene，输入显式类别及顺序、各系列量值、单位、基线、分组关系和图例。类别轴承担离散位置，量值轴承担数值映射；不同系列的分组及量值方向必须可读。保留零值、负值和缺失值的区别，默认不擅自将未知值写成零；基线的含义明确。

利用已经完成的轴、文字、矩形绘制和裁剪，不另建 UI 架构。类别过滤和系列显隐可以使用当前参数绑定，数据/定义变更沿原位更新；实际数据与状态按所属产物保存并恢复。

## 验收

用真实类别顺序、正负量值、多组及缺失项生成柱图；BRP 核对基线、标签、分组、图例、缩放与状态更新，柱高按输入数据验证。

## 与相邻方案的关系

与面积图仅共享绘图基础，不依赖面积语义；下一项独立处理组成比例。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
