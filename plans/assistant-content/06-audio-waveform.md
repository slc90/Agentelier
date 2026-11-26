# 展示真实音频时域波形与播放进度

紧凑播放器由播放按钮、时域波形、总时长组成。采用 Symphonia 解码能力从已确认的 MP3、FLAC、WAV 提取整段音频的幅度包络，以显示区间聚合峰值或极值。多声道预览保留各声道的真实幅度，静音、强弱变化和脉冲在摘要中可辨。

后台计算有界包络，缓存按同一内容摘要及显示需求复用。UI 只消费摘要，使用原生 BSN/绘图和现有主题。Widgetry Waveform 的缩减及绘制接口可作为复用依据；其完整控件面向多通道、测量与电压单位，既不发声也不适合直接整块嵌入聊天。优先复用合适的公开能力，否则实现紧凑的 Agentelier 原生波形控件。不引入音频编辑工作台，不修改上游私有状态。

波形只展示音频幅度与进度，播放和暂停通过同一个按钮完成。已播放波形使用强调色，未播放部分较淡。进度来自实际 `AudioSinkPlayback::position()` 和总时长，暂停时保持当前位置，结束时显示实际终点。首次资源计算显示适当占位；失败时保留有效播放器并明确波形不可用。波形摘要可从已保存的音频重建。UI Scale、宽度变化不重复加载全部历史资源。示意见 [音频播放器图](../../docs/design/assistant-content/images/audio-player.svg)。

## 验收

选取真实 MP3、FLAC、WAV，覆盖静音、音量变化和短脉冲，用实际解码结果对照波形。BRP 核对幅度形态、时长、播放进度、暂停和恢复、换播归零、结束与重开。波形不可用时仍可操作有效的播放器。

## 与相邻方案的关系

依赖实际播放及时间状态。该项完成后可通过按钮控制播放，对照波形查看时长和进度。

设计依据：[媒体产物](../../docs/design/assistant-content/02-media-artifacts.md)、[回复呈现](../../docs/design/assistant-content/01-reply-presentation.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
