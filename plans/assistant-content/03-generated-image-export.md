# 从回复图片保存原图副本

上一增量让生成图片可以在聊天中查看，但预览的 RGBA 资源不是用户应当保存的原始文件。现有 `ImageAttachmentAction::Export` 已通过 Runtime 读取原图，要求外部绝对路径、应用数据目录之外的目标，以同目录临时文件同步并不覆盖提交。

在生成图内容项接入 Widgetry FileDialog 和用户主动的另存为操作。来源仍是该 Session 的原调用和原图描述符，不从缩略图编码，也不把外部导出路径替换为会话内附件路径。导出时保留正确的 PNG/JPEG 格式；已有目标、不可写路径或用户取消按实际结果反馈。导出回执只更新对应内容项，晚到时不会作用于另一个 Session。会话原件、模型派生图及历史位置保持不变。重开 Session 后可直接导出已确认的生成图。普通工具读图材料不增加公开导出入口。

## 验收

使用 BRP 点击图片另存为、实际选择有效目标，确认导出编码文件摘要与原件相同且可解码；取消、已有目标、写入失败均不改动原件，重新打开历史仍能预览并导出。

## 与相邻方案的关系

依赖图片产物与预览行；后面的音频交付是独立媒体能力，不依赖导出。

设计依据：[媒体产物](../../docs/design/assistant-content/02-media-artifacts.md)、[回复呈现](../../docs/design/assistant-content/01-reply-presentation.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
