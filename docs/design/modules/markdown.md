# 原生 Markdown 展示

`agentelier_markdown` 使用官方 pulldown-cmark 0.13.6 事件与原文区间解析当前消息全文。`MarkdownView` 是持久 SceneComponent，Props 初始化 `MarkdownSource` 并选择是否构造原文工具栏。运行更新只修改 `MarkdownSource`。聊天状态与协议历史中的原始正文不受展示影响。

解析结果包含块、行内样式和链接目标；标题、段落、软硬换行、粗体、斜体、删除线、行内代码、引用和分隔线通过 BSN 与原生 TextSpan 组合。字体匹配使用 Bevy 官方 system_font_discovery，行内代码选择系统等宽字体。HTML 字面显示，图片展示替代文字和目标。列表按条目展开为带层级和序号的块，后续段落与嵌套条目保留缩进。围栏与缩进代码从解析事件保留正文及语言标签，以等宽、不折行文本和独立横向 ScrollArea 展示；未闭合围栏仍按当前事件树呈现。代码文本增长时原位更新文本和高度，保留横向位置；纵向滚轮在横向控件消费前转交消息区。Mermaid 仅作为代码。GFM 表格保留列对齐、表头、行和单元格，使用 BSN Grid 与独立横向 ScrollArea，每列保留最小可读宽度。实际布局高度回写滚动容器，纵向滚轮继续交给消息区。流式更新按行列身份复用未变化单元格和整个滚动容器。Widgetry Table 的数据源、选择与编辑模型不用于这些内联只读、随流式增量变化的文档片段。任务列表仅显示勾选状态，不修改原文。

独立数学公式进入原生 Markdown 块。解析器消费 `$$…$$` 和经源文区间适配的 `\[…\]`，按消息原始区间复制 TeX。Markdown 视图在后台将闭合公式排版为 Bevy 图像，用 Widgetry 横向滚动承载超宽公式，排版失败时局部保留原文。实现边界见[数学排版](../assistant-content/03-math-typesetting.md)。

行内公式进入与正文共享的文本树，适用于段落、标题、列表与表格。Bevy 的 `InlineBox` 负责断行，图像根据布局返回的坐标和 TeX 基线绘制。公式等待排版或失败时，原有 TextSpan 显示对应源文。正文复制和原文视图仍从 `MarkdownSource` 读取。

一帧合并业务快照后，Changed MarkdownSource 触发全文解析，使后补引用定义也能校准前文。按块位置与内容复用未变化实体，只替换变化块并保持顺序；历史完成消息不因其它消息增量重建。界面输入控件不在 Markdown 子树中。

链接只提供用户主动按钮，解析和执行边界均检查 HTTP/HTTPS 和有效主机。实际系统打开由已有 Widgetry JobManager 管理，结束时消费结果并移除任务；错误显示在仍存在的对应链接下，不把正文或地址写入业务日志。Markdown 不加载图片或执行 HTML、自定义 URI、脚本。

聊天阅读由 chat_ui 管理：公开 ScrollPosition 与布局尺寸确定是否在底部，用户向上滚动后停止自动跟随；更新后布局只在跟随时移到底部。用户滚回底部时恢复跟随，必要时请求重绘。页面重建恢复内存中的阅读状态。

ConversationView 将用户头像与正文放在右侧，将助手头像和最终回复的 Markdown 正文放在左侧。头像独占一行，正文可使用聊天区的可用宽度。消息行使用会话、输入或输出身份复用，收到变化时更新对应正文。

解析器依据：[官方指南](https://pulldown-cmark.github.io/pulldown-cmark/)、[0.13.6 API](https://docs.rs/pulldown-cmark/0.13.6/pulldown_cmark/)。

记忆聊天的原文与排版共用一个 MarkdownSource。切换视图时，工具栏、渲染正文和只读 TextField 保持身份。原文更新通过 WidgetryTextFieldHeadless 公开 API 同步。复制正文直接取当前 MarkdownSource，代码复制取当前解析代码文本。操作结果显示在对应按钮上。渲染子树尚在 Scene 构造时，完成后再同步。
