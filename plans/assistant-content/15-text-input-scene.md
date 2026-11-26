# 提供动态文本参数输入

当动态控件需要名称、表达式、搜索或自定义文本时，可使用现有 Widgetry TextField。提供可发现的原生文本输入 Scene，接受语义化初始文本、占位说明和必要的文本约束，通过 Widgetry 公开 Headless 接口和真实编辑/提交事件提供值。保持 IME、选择、键盘焦点和原聊天输入区独立；在动态控件中输入或 Enter 不应被 ChatComposer 当成发送聊天。不要以模型请求或重复创建文本 Entity 实现本地编辑。

定义与创建、错误及销毁沿用已登记场景产物机制。Props 仅初始化。文本编辑事件的消费和状态保存通过下一阶段的组合、绑定及 Session 状态机制完成，本增量提供真实可用的输入与值事件。

## 验收

BRP 获取动态 TextField 焦点，执行真实输入、编辑、选择及键盘操作，确认动态值而非主聊天草稿改变。检查切换、多个独立输入及错误反馈；IME 需要实际对应环境验证。

## 与相邻方案的关系

是动态原生控件模板的又一种输入，不依赖数值/离散模板实现细节。下一项将已完成的输入和显示组合成一个内容场景。

设计依据：[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
