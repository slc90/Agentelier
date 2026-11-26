# 提供原生散点图

折线图表达已知顺序的连续观测，散点图表达独立二维数据。增加散点 Scene，使用同一数值坐标变换和轴绘制，输入显式 X/Y 点、系列标签、单位及必要颜色/尺寸参数；不同系列以图例区分。不能因数据列表有顺序就连接各点，也不自动添加没有依据的回归曲线。

缺失或非有限点按明确规则处理；范围和坐标比例来自有效数据或语义参数。数据变化只更新受影响图形，命名参数可驱动筛选/系列选择，Scene 定义和当前有效参数沿既有 Session 产物持久化。模型提供数据而非 SVG 截图，Widgetry / Bevy 原生绘图遵守公开 API。

## 验收

创建包含无序样本、重叠点、多个系列及缺失值的散点图；BRP 核对点坐标、图例、无错误连线。通过已有参数机制筛选一个系列并重开历史，正确状态与位置恢复。

## 与相邻方案的关系

复用折线图建立的数值坐标层，但散点是独立的图形语义；下一项复用连续路径生成面积图。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
