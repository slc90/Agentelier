# 提供原生组成比例图

饼图表达各非负部分占总体的比例。输入显式类别、数值和单位，按实际合计计算角度、显示图例及可读标签；零值、总量为零、负值、缺失或非有限值应有明确语义，不能将不成立的比例伪装成完整圆形。图形使用原生路径/扇区绘制，复用主题和通用显示资源，不依赖坐标轴或 WebView。

系列的数据、名称、颜色和需要持久保留的选择状态进入现有 Scene 定义和 Session 状态流程。若支持选择或筛选，依据当前显式数据合计重算，不触发模型请求。

## 验收

输入可核算的 1:2:3 类别份额，检查真实角度和百分比；检查零值、总量零和非法输入；BRP 核对图例与有效数据一致并能重开恢复。

## 与相邻方案的关系

与柱状图仅顺序相邻，组成图不需要分类轴；下一项使用独立矩阵和颜色映射。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
