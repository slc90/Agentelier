# 建立左右头像布局的 ConversationView

## 目标

交付 D 区上方的聊天消息展示控件，按用户右侧、助手左侧、头像独占一行的方式呈现当前 Session 历史和流式内容。

## 消息布局

每条消息的头像单独占一行，正文从下一行开始：

- 用户头像在右侧，用户正文靠右。
- 助手头像在左侧，助手正文靠左。
- 正文与头像保持同侧对齐。
- 正文不强制使用狭窄固定宽度气泡；长篇 Markdown、代码和工具结果利用聊天区域宽度。

Avatar 统一使用 `AgentelierAvatar`。用户消息显示用户头像或默认头像；助手消息显示助手形象头像。

## 消息内容与协议展示

支持普通文本、Markdown、代码块、图片、工具调用与工具结果、历史消息和流式输出。复用 `agentelier_markdown`，不另建 Markdown 解析器。

消息身份、公开 Assistant phase、工具调用和结果顺序遵循 Conversation 展示投影。隐藏 reasoning、内部 compaction 和协议控制项不作为普通消息展示。历史恢复、失败部分回复和已提交工具结果沿用 Conversation 的权威展示记录。

## 流式更新与阅读位置

流式输出按消息身份增量更新受影响正文，不重建整个消息列表。用户向上阅读时，新内容不强制滚到底部；用户原本处于底部跟随状态时才跟随新消息。更新不能抢输入焦点、清空 ChatComposer 草稿或破坏文本选择。

ConversationView 是 Agentelier 组合控件，Agent 继续拥有消息历史，UI 只消费 Bridge 投影。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
## 完成条件与验收

1. 用户头像在右侧，助手头像在左侧。
2. 头像独占一行，正文从下一行开始；正文与头像保持同侧对齐。
3. 用户消息靠右，助手消息靠左，连续消息的归属清楚。
4. 普通文本、长篇 Markdown、标题、列表、引用和多段正文正确显示。
5. 代码块保留等宽字体、复制能力和合理的换行 / 横向滚动，不被窄气泡压缩。
6. 图片附件和工具结果图片使用当前 Session 的安全资源描述符；缺失资源显示占位，不读取任意用户路径。
7. 工具调用和工具结果顺序正确；失败、取消和部分完成状态不被伪装为成功。
8. Assistant 公开 phase 保持区分；隐藏 reasoning 和 compaction 不显示为普通消息。
9. 流式输出逐步更新，长回复不会每个片段都重建整个列表。
10. 用户向上滚动阅读时，新内容不会强制拉到底部。
11. 用户处于底部跟随状态时，新内容自然跟随；用户离开底部后停止强制跟随。
12. 切换 Session 后，旧 Session 的迟到流式事件不能写入当前消息列表。
13. 重开 Session 后，消息顺序、工具结果、图片和公开阶段与存档投影一致。
14. 主题变化、UI Scale 变化和窗口宽度变化不清空消息、焦点、选区或滚动位置。
15. 使用 BRP 验收滚动、选择、焦点和流式时序；截图确认头像与正文布局。
