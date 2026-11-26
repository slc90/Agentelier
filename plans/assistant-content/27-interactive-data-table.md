# 提供可供动态场景使用的原生数据表

当前 Markdown 已有只读表格，并以原生 BSN Grid 和横向 ScrollArea 展示，不需要替换。新增的是动态场景需要的结构化数据表：按实际字段/列类型、行身份、数据集和必要格式显示，可作为与其他控件联动的数据来源或结果区。优先使用 Widgetry 公开 Table 或适当的 ListView 能力，核实与当前锁定 Feature 的兼容，不把 Markdown 表格改造成编辑器。

用户选择行后，通过已有本地参数绑定输出真实的行身份和字段值，其他组件可消费同一数据。若提供排序，按稳定行身份保持选择，并区分数据与显示顺序。按已发布 Scene 的定义、资源及需要恢复的选择状态保存。数据量超过适合显示的范围时使用控件实际支持的可读滚动或虚拟化方式。

## 验收

生成有名称、数值及多行数据的表。BRP 检查实际行选择、滚动、窄宽度和与另一显示项联动。更新和重开后核对行身份与数据，再用普通 Markdown 表格核对已有展示。

## 与相邻方案的关系

与各图表共享结构化数据和参数状态，没有依赖箱线图；下一项组合已有绘图系列。

设计依据：[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)、[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
