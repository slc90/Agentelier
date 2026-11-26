# 提供有明确基线的面积图

面积图在连续观测曲线之外还表达一个范围或基线以下的面积。提供原生面积 Scene，使用真实系列和连续坐标轴，明确填充边界、基线、系列颜色和图例；数据跨越基线或出现缺失区间时按实际片段填充，不跨断点画出不存在的区域。若需要多个系列，保持各自语义和图例，未给出堆叠含义时不擅自相加。

复用折线图的连续坐标、裁剪和路径能力，原生填充使用实际有效的绘图 API。数据、基线及可变参数经现有产物定义和状态保存，支持原位更新和历史恢复。

## 验收

使用正负值、非零基线及有缺口的数据，检查实际填充区域及轴单位；BRP 确认缩放和更新后不跨越缺口或裁剪错误，重开保持定义。

## 与相邻方案的关系

依赖连续坐标/路径基础，不依赖散点图；下一项扩展离散类别的柱形展示。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
