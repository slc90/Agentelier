# 提供动态数值参数控件

Widgetry 上游已有 Slider 与 Number Input，当前 Agentelier 未启用相应 Feature。本增量形成统一的数值参数输入能力：注册支持整数/浮点语义、初始值、最小/最大值及步进的原生数值 Scene。需要精确输入的数值可用 Number Input，需要直观调节时可用 Slider；两者通过同一类数值语义提供真实变化事件，不在此步硬编码具体数学计算。

复用现有 Scene Factory 的发现和语义参数，BSN 构造相应 Widgetry 控件，Props 只负责初始化，持续状态通过公开值接口和 `ValueChange` 或相应公开事件管理。无效范围、步进和值返回原有结构化错误。保持键盘操作、拖动、聚焦和主题一致。创建或更新仍以已发布的产物身份及定义修订为依据，不把数值写进另一个长期 Props 副本。独立场景定义可恢复，用户值的通用绑定和持久化由后续增量接入。

## 验收

真实 Agent 分别创建整数和浮点数值输入实例；BRP 实际拖动 Slider、键盘操作及 Number Input 输入/步进，检查边界与公开事件，非法初值有明确反馈。更新定义后实例状态遵守实际范围，原聊天上下文不变。

## 与相邻方案的关系

依赖已发布和可更新的模板；下一项扩展离散参数，不要求数学图形已经完成。

设计依据：[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
