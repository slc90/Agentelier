# 提供动态离散选择与开关控件

有些交互不是连续数值，而是选择类别、是否显示某条曲线等离散状态。复用 Widgetry 已有 ComboBox、Radio Group、Segmented Control、CheckBox、Toggle Button，按实际需要启用对应 Feature，并为枚举选择、布尔值这两类离散参数提供语义一致的 Scene 模板。Button 已存在，可作为用户主动动作入口，不从选择控件直接修改其内部状态。

选择模板接受选项的稳定值、可见标签、默认选中；开关表达布尔初始值。选择结果通过真实 Widgetry 事件和公开 API 返回；无效选项、重复身份及不一致初值应可读反馈。Props 初始化，运行时 Component 持有交互状态，模板发现返回真实支持的参数。后续绑定用值身份而不是标签字符串或临时 Entity ID。这里不引入自动模型请求或专用权限限制。

## 验收

BRP 验证真实下拉、Radio/Segmented 选择、键盘导航与开关/勾选交互，检查选项值和变化回执。非法定义不残留空控件，多个实例不互相改变状态。

## 与相邻方案的关系

依赖原生 Scene 注册；与数值控件仅共享语义输入和宿主。下一项提供文本输入。

设计依据：[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
