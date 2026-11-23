# 建立 AgentelierAvatar 统一头像控件

## 目标

统一用户头像与助手头像的显示，为 B 区登录按钮和 D 区消息列表提供相同的尺寸、圆形 Mask 和资源回退行为。

## 设计

`AgentelierAvatar` 负责头像尺寸、圆形裁剪、图片比例适配、加载占位和默认回退。

- 用户头像有配置时显示用户图片；没有配置或读取失败时显示内嵌默认用户头像。
- 助手头像使用助手形象素材。
- 图片尚未加载时保留头像占位尺寸，不推动消息列表或按钮布局。
- Avatar 接收调用方提供的裁剪变换，不负责保存用户图片。

用户上传原图与裁剪设置由后续设置窗口增量负责；Avatar 只消费图片引用和裁剪变换。

## Widgetry 与资源

图片由 `crates/ui/assets` 统一注册和提供语义资源引用。Avatar 使用 Bevy Image / Widgetry Core 公开呈现能力，不重写 Asset 加载器，也不在各个页面散布图片物理路径。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
- 未经用户明确授权，不执行登出、重新授权、切换账号或清理现有凭据的验收操作。
## 完成条件与验收

1. 无用户头像时显示默认用户头像。
2. 用户头像有效时显示原图经裁剪变换后的结果，保持圆形 Mask。
3. 图片不存在、格式无法读取或 Asset 加载失败时回退默认头像。
4. 助手头像显示助手形象，人物比例不被拉伸。
5. 图片加载前后控件尺寸不变，不推动消息列表或登录按钮发生布局跳动。
6. 不同尺寸下保持圆形边界，图片不越过 Mask。
7. 运行时更新头像引用后，已有 Avatar 实例更新图像，不销毁其所在消息或按钮。
8. UI Scale 改变后头像仍保持预期尺寸。
9. 特殊路径显示不出现 `\\?\` 前缀；图片读取使用原始路径或 Asset 引用。
10. B 区登录按钮和 D 区消息共用同一个 Avatar 控件，不各自实现裁剪和回退。
11. 使用真实 GUI 和截图检查圆形 Mask、缺失图片回退与不同 UI Scale 下的布局。
