# 原生 Markdown 展示

`agentelier_markdown` 使用官方 pulldown-cmark 0.13.6 事件与原文区间解析当前消息全文。`MarkdownView` 是持久 SceneComponent，Props 仅初始化 `MarkdownSource`；运行更新只修改该组件。聊天状态与协议历史中的原始正文不受展示影响。

解析结果包含块、行内样式和链接目标；标题、段落、软硬换行、粗体、斜体、删除线、行内代码、引用和分隔线通过 BSN 与原生 TextSpan 组合。字体匹配使用 Bevy 官方 system_font_discovery，行内代码选择系统等宽字体。HTML 字面显示，图片展示替代文字和目标。尚未适配的列表、代码和表格保留对应原文。

一帧合并业务快照后，Changed MarkdownSource 触发全文解析，使后补引用定义也能校准前文。按块位置与内容复用未变化实体，只替换变化块并保持顺序；历史完成消息不因其它消息增量重建。界面输入控件不在 Markdown 子树中。

链接只提供用户主动按钮，解析和执行边界均检查 HTTP/HTTPS 和有效主机。实际系统打开由已有 Widgetry JobManager 管理，结束时消费结果并移除任务；错误显示在仍存在的对应链接下，不把正文或地址写入业务日志。Markdown 不加载图片或执行 HTML、自定义 URI、脚本。

聊天阅读由 chat_ui 管理：公开 ScrollPosition 与布局尺寸确定是否在底部，用户向上滚动后停止自动跟随；更新后布局只在跟随时移到底部。回到底部按钮恢复跟随，必要时请求重绘。页面重建恢复内存中的阅读状态。

解析器依据：[官方指南](https://pulldown-cmark.github.io/pulldown-cmark/)、[0.13.6 API](https://docs.rs/pulldown-cmark/0.13.6/pulldown_cmark/)。具体支持范围见[聊天功能](../../features/chat.md)。
