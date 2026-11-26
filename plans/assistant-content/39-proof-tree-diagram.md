# 原生展示逻辑证明树与推导结构

证明树由前提、结论、推导边及推理规则标签构成，不能将其当作普通任意节点网络或单一数学字符。支持由模型明确给出的推导层级和数学记号，复用原生路径、布局、箭头/推导线及公式排版，表达多前提推导、嵌套证明和逻辑标签。`bussproofs` 等是结构与记号参照，不要求运行完整 TeX 文档或内置证明校验器；没有给出推导依据时不宣称证明已验证。

图形定义记录稳定节点和真实关系，宽度不足时保持可阅读/滚动。定义错误在原内容位置反馈，普通源文仍可查看。作为正式动态 Scene 保存定义、必要布局和用户状态，重开按同一语义结构重建。

## 验收

用包含两前提合并及多层推导的真实公式创建证明树，核对父子关系、规则文字、公式与布局；BRP 观察窄宽度、非法关系回退和历史恢复。

## 与相邻方案的关系

与交换图共享数学标签和结构绘制，但证明树有独立的推导语义。下一项从预设场景进入脚本自定义控件。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[数学排版](../../docs/design/assistant-content/03-math-typesetting.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
