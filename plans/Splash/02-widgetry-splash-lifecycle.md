# 接入 Widgetry Splash 启动生命周期

## 目标

让 Agentelier 每次启动时都先显示独立 Splash 窗口，再把显示权交给主窗口。数据目录检查与首次选择放在 Splash 交接完成之后执行；首次启动的数据目录选择使用模态原生窗口，不再嵌在主窗口内容区域中。

该增量使用 `bevy_widgetry` 已有的 Splash 会话完成首帧显示和主窗口交接，不自行实现第二套原生 Splash 窗口。动画由 Bevy 实时生成，不引入 GStreamer、视频容器、视频解码器或外部视频播放 Pipeline。

## Widgetry 与 application 的职责

`bevy_widgetry::splash` 已经提供独立窗口、窗口定位、首帧呈现握手和主窗口交接。Agentelier 不再实现第二套原生 Splash 窗口。

- Widgetry 创建并定位 Splash 原生窗口，管理其固定物理尺寸、首次显示、主窗口显示和帧确认。
- application 创建 Splash Camera、场景实体、资源和播放状态，并决定何时提交 `ready`、`finished`、`main_ready`。
- application 在 `SplashCompleted` 后清理自己创建的 Camera 和场景实体。
- application 负责在 Splash 完成后通知 login_ui 开始数据目录恢复或选择。
- login_ui 仍然拥有数据目录检查、目录选择、数据环境创建和 `DataDirectoryReady`。
- Splash 不进入 Agent、Bridge 或 Session 的业务状态，也不改变 System Tray 的隐藏与恢复逻辑。

## 每次启动都先运行 Splash

Splash 不以数据目录配置作为启动条件。即使 `config/config.ron` 已经保存有效数据目录，也必须先完成 Splash 演出和主窗口交接，然后才开始检查该目录。

当前 `LoginUiPlugin` 在 `LoginUiHost` 添加时就读取保存路径并立即检查；没有保存路径时则直接在主窗口内容区显示目录选择器。该行为需要拆成两个阶段：

1. `LoginUiHost` 挂载时只记录主窗口内容宿主，不立即检查或选择数据目录。
2. application 在收到 `SplashCompleted` 后，向 login_ui 发出一个应用级启动事件，例如 `BeginDataDirectorySetup`。login_ui 收到后才开始现有的数据目录检查或目录选择流程。

这样数据目录工作不会提前发生在 Splash 后方，也不会让首次启动因为等待用户选择目录而延长 Splash 的显示时间。

## 插件装配与主窗口隐藏

在 `crates/ui/application/Cargo.toml` 当前 `bevy_widgetry` 依赖 Feature 列表中加入 `splash`，并在 application 插件装配中注册 `SplashPlugin`。

`SplashPlugin` 的系统顺序依赖已经安装的 `WidgetryUiScalePlugin`。当前 application 已经在 `DefaultPlugins` 前安装 UI Scale，因此 Splash 插件在 DefaultPlugins 和 Widgetry UI 基础插件可用后注册即可。

PrimaryWindow 必须在原生创建时就设置为 `visible: false`。不能先创建可见主窗口，再在 `Splash::start` 后隐藏；Widgetry 的 Splash API 不会替应用隐藏主窗口。

主窗口初始尺寸采用既定的 1920 × 1080。创建 Splash Window 时，从 PrimaryWindow 的实际物理尺寸构造相同大小的窗口参数，而不是把逻辑尺寸或 UI Scale 值直接当成物理像素。Splash 窗口由 Widgetry 固定为物理像素尺寸，并排除普通 UI Scale。

## 创建 Splash 窗口和内容 Camera

在 PrimaryWindow Entity 已存在后调用：

```rust
let splash = Splash::start(world, main_window, splash_window)?;
```

返回的 Entity 是独立 Splash Window。随后由 application 创建专属 `Camera2d`，并将渲染目标指定为：

```rust
RenderTarget::Window(WindowRef::Entity(splash))
```

Splash 的场景根和字形渲染实体都由 application 创建，并通过专属 Camera 渲染。正式画面不放幕次标题或字幕。不要把 Splash 内容挂到主窗口的 Widgetry Root 下，也不要复用主窗口 Camera。

本增量先将共享的 `main-window-background.png` 作为静态内容绘制到 Splash Camera，验证窗口创建、显示和交接。后续字形渲染和四幕时间线会替换这一静态内容。

## 首帧显示握手

场景根、Camera 和背景资源准备完成后调用：

```rust
Splash::ready(world, splash)?;
```

`ready` 表示 application 的内容已经准备好，不表示窗口可以立即显示。Widgetry 仍会等待真实渲染帧完成呈现，然后发出 `SplashShown { splash, main }`。

`SplashShown` 是正式播放计时的起点。当前静态生命周期阶段从该事件开始计时，达到演出总时长后调用 `Splash::finished`。后续四幕时间线会接管该完成信号。

## 主窗口就绪不再依赖数据目录

主窗口需要先拥有可以显示的基础界面，然后才能向 Splash 提交 `main_ready`。这一条件只代表主窗口的基础 UI 已经挂载，不代表 Agentelier 的业务工作区已经初始化。

当前 `setup` 会创建 PrimaryWindow 对应的 Widgetry Window Root，并在内容中挂载 `LoginUiHost`。application 可以在 `LoginUiHost` 添加后提交 `Splash::main_ready`。Widgetry 随后仍会等待主窗口隐藏状态下的有效渲染帧，因此 `main_ready` 不需要等待数据目录、ChatUiHost 或 Session。

`Splash::finished` 和 `Splash::main_ready` 是两个独立条件，可以任意先后到达。主窗口先就绪时，Splash 继续播放；Splash 先结束时，保持最后一帧，等待主窗口就绪。

主窗口的基础窗口尺寸、背景和标题栏在 Splash 期间已经准备好。`DataDirectoryReady` 不再负责把窗口从 1100 × 760 调整到 1920 × 1080；尺寸与位置应在 Splash 启动前确定，避免交接完成后窗口突然变化。

## 主窗口显示后再处理数据目录

当两个 Splash 条件都满足时，Widgetry 会：

1. 等待隐藏主窗口的有效渲染帧。
2. 显示主窗口。
3. 等待主窗口显示后的新帧完成呈现。
4. 移除 Splash Window，并发送 `SplashCompleted { splash, main }`。

application 收到 `SplashCompleted` 后通知 login_ui 开始数据目录初始化。此时主窗口已经显示，数据目录选择不再影响 Splash 的交接。

login_ui 根据现有配置决定后续行为：

- **已保存数据目录：**启动现有 `DataEnvironment::open` 检查流程。检查期间可以在主窗口基础内容区域显示状态；检查成功后发送 `DataDirectoryReady` 并挂载 `HomeContent`。
- **没有已保存目录：**创建独立的 Widgetry Owned Window 作为目录选择窗口，并将 `WidgetryModalWindow { parent: primary_window }` 绑定到主窗口的原生 Window Entity。目录选择器作为这个模态窗口的内容，不再作为 `LoginUiHost` 的普通子节点。
- **已保存目录不可用：**保留现有失败信息与重试能力；用户选择“重新选择”时打开同一个模态目录选择窗口。

模态关系由 Widgetry Window 管理。主窗口继续显示在后面，但被模态窗口阻挡输入；用户确认目录并完成初始化后，目录选择窗口关闭，主窗口挂载正常工作区。

目录选择窗口使用现有 `WidgetryFileDialog`，不另造文件系统浏览器。其关闭和销毁由 login_ui 所有，父窗口关闭时清理该模态窗口。用户取消选择时沿用现有启动取消语义，向应用提交正常 `AppExit::Success`，由既有 shutdown 流程收尾。

## Widgetry 交接与资源清理

收到 `SplashCompleted` 后，application 清理 Splash Camera、场景根和专用资源，移除会话状态。不要在 `SplashFinished` 时提前销毁场景；主窗口帧确认完成前，Splash 的最后一帧必须保持可渲染。

主窗口进入正常运行后，Splash 不再参与窗口管理。System Tray 继续只负责隐藏和恢复现有窗口；从托盘恢复主窗口不会重新创建 Splash，也不会重新检查或选择数据目录。

## 更新模式与失败回退

Splash 动画窗口可能没有获得键盘焦点。播放期间 application 应将 `WinitSettings` 切换为持续更新/渲染模式，避免桌面应用的低功耗更新策略暂停时间线。完成或取消后恢复 `WinitSettings::desktop_app()`。

如果 `Splash::start`、场景资源加载或首帧呈现初始化失败，application 应取消已创建的 Splash 会话，并显式将主窗口设为可见。随后仍然向 login_ui 发出 `BeginDataDirectorySetup`，继续已保存目录检查或模态目录选择，不能让启动流程停在空窗口。

收到 `AppExit` 时清理仍然存在的 Splash 会话和场景资源。正常启动完成后，托盘隐藏和恢复只操作现有主窗口与辅助窗口，不重新播放 Splash。

## 完成条件

- application 使用 Widgetry 的 `splash` Feature 和 `SplashPlugin`，没有自行实现第二套原生窗口管理。
- Agentelier 每次进程启动都会先运行 Splash，不因已保存数据目录而跳过。
- PrimaryWindow 从原生创建时就是隐藏状态，且初始尺寸已经是 1920 × 1080。
- Splash Window 与主窗口物理尺寸匹配，并由独立 Camera 渲染。
- `Splash::ready` 后等待 `SplashShown`，而不是直接设置 Splash Window 可见。
- `Splash::main_ready` 只依赖主窗口基础 UI 已挂载，不等待 `DataDirectoryReady`、Session 或工作区初始化。
- `SplashFinished` 与 `MainWindowReady` 任意先后到达都能正确交接。
- Widgetry 完成主窗口帧确认并显示主窗口后，application 才启动数据目录恢复/选择。
- 首次启动的数据目录选择以独立 Widgetry Owned Window 显示，并通过 `WidgetryModalWindow` 阻挡主窗口输入。
- 已保存有效目录时不重复弹出选择窗口，仍然执行现有自动恢复检查。
- 已保存目录不可用时可以重试或通过模态窗口重新选择。
- 用户取消首次目录选择时沿用正常退出语义。
- `SplashCompleted` 后清理 application 所有的 Camera 和场景实体。
- Splash 创建或初始化失败时主窗口仍然可以显示，并继续数据目录初始化。
- AppExit 能清理未完成的 Splash 会话。
- 托盘恢复不会重新播放 Splash，也不会重新启动数据目录选择流程。

验证使用真实 Windows 启动、目录选择状态、保存目录状态、BRP 和日志观察；遵循项目现有约束，不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。
