# 将 Agent 取得的音频文件发布为回复产物

当前用户音频通过 `AudioDraft` 保存并在 `Accepted` 时关联用户输入。`session_store::audio::import` 已使用 Symphonia 0.6.1 检测并完整解码 MP3/PCM WAV，`AudioAttachment` 保存 UUID、文件名、格式、摘要、编码长度和时长；附件在 `sessions/<uuid>/attachments/audio-<sha256>.<ext>` 不可变保存。助手生成文件的语义不同，不能把它挂成用户草稿或把一段工具输出路径当成产物。

提供 Root Agent 明确交付已完成音频的能力。Agent 可先用现有工具或 Skill 生成文件，再发布该文件；工具名称和 schema 根据冻结 ToolCatalog 与现有工具/Runtime 分工确定，发布能力在创建支持该功能的 Session 时登记。没有确定可用的音频生成服务，本增量不开发 TTS、音乐模型或转写替代服务。

```mermaid
flowchart LR
    F["已完成 MP3、FLAC 或 WAV"] --> I["后台导入校验"]
    I --> S["现有 Session Store 不可变原件"]
    S --> R["持久助手音频产物记录"]
    R --> V["Turn 内容项"]
```

按冻结工作目录解析目标文件。扩展助手产物导入的实际格式识别和完整解码，接受 MP3、FLAC、WAV，验证文件长度、SHA-256、时长及完整性。沿用单文件 20 MiB、会话 128 MiB 音频预算及按摘要去重。记录明确所属 Session、Root Turn、来源请求/调用、音频描述符与产物 ID。实际文件和记录提交确认后才返回发布成功并投影到原回复。用户附件保留原有输入、转写和文稿选择契约，助手产物单独显示在回复内。

损坏、不完整、超限、原文件读取变化或保存失败如实返回，不删除外部生成文件。恢复从已确认快照读取，支持异步晚到、跨 Session 隔离；删除沿用既有受控目录和清理规则。此增量的可见产出为有真实元数据和稳定身份的助手音频项；播放在下一方案接入。

## 验收

分别发布实际可解码的短 MP3、FLAC、WAV，检查存档身份、文件摘要、时长、当前回复的音频项和实际工具回执。移动或替换源文件后，重开仍读取已保存原件。检查损坏、超限及其他格式的失败反馈，以及用户草稿和转写的原有行为。BRP 核对产物位于原助手控件。

## 与相邻方案的关系

上一项图片导出无技术依赖；复用第一项有序回复容器。下一项从已确认音频产物加载播放。

设计依据：[媒体产物](../../docs/design/assistant-content/02-media-artifacts.md)、[回复呈现](../../docs/design/assistant-content/01-reply-presentation.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
