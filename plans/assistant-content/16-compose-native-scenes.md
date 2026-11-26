# 在一个回复产物中组合多个原生 Scene

标准控件能够单独创建，但一个函数探索器需要公式区、绘图区、参数区和结果区放在同一个内容块中。让现有 Scene Factory 支持组合场景的语义位置及可用容器，Rust 内部仍以 BSN、Scene Function/SceneList 组合实际 Children，不建立一个替代 BSN 的通用 UI 描述语言，也不使用手写 ECS Entity Tree 拼装 Widgetry 私有结构。

一个组合产物拥有根和若干具有稳定逻辑身份的内容位置，可包含多个已注册的输入/显示 Scene。模型提供组成所需语义数据；其 Scene 构造和父级拥有关系必须完整。现有 Scene Factory 的 owned entities 清理原则适用，销毁或更新组合只清理它实际拥有的资源。布局根据聊天宽度、滚动及 UI Scale 正常工作；内容更新不影响兄弟实例。组合定义保存实际组件与参数，恢复时重建同一结构；本阶段可使用文本、按钮和数值参数构成简单控制区，不要求函数绘图先完成。

## 验收

真实 Agent 创建包含文字、Slider、Number Input 与按钮/选择器的单一内容场景；BRP 检查层级、宽度、位置、焦点、个别控件操作及切换/重开后结构。创建失败只清理该组合的拥有实体。

## 与相邻方案的关系

依赖必要基础模板和发布机制；下一项建立多个控件之间真实的数据依赖。

设计依据：[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)、[持久化与 Skill](../../docs/design/assistant-content/07-persistence-and-skills.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
