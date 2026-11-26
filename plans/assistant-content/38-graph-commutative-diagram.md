# 展示图论结构与交换图

图论和范畴论中的图需要节点身份、边关系、方向、标签及必要布局信息。使用 Widgetry Canvas 的公开路径、连接线、箭头和文字能力或等价原生绘制，建立语义图形 Scene；数学公式/对象标签复用既有排版。对图论无向/有向边和交换图映射保持准确，不将图形看起来闭合当作交换性已经证明。模型给出真实关系，不要求自动推导范畴论结论。

允许节点位置或明确参数的变化更新相应连接线和标签，按需复用画布交互，不引入独立图编辑器或工作流引擎。复杂 `amscd` 类表达可经明确结构化描述显示，原 LaTeX 保持可读；保存关系定义、布局状态与资源，不保存临时 Entity 引用。

## 验收

创建一个有向图及带数学箭头/标签的交换图；核对边方向、关系、数学标签和布局。BRP 操作可移动节点并检查连接跟随及重开恢复，无效节点引用局部反馈。

## 与相邻方案的关系

共享几何绘图能力，不需要几何约束求解；下一项为推导层级提供证明树语义。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[数学排版](../../docs/design/assistant-content/03-math-typesetting.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
