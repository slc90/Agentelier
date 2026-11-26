# Agentelier 主窗口与交互体系

主窗口标题栏集中放置导航和管理入口。左侧组织 Project 和单独聊天，右侧展示当前 Session。切换 Session 时，聊天区恢复该 Session 的记录、草稿和阅读位置。跨会话的后台任务仍由执行层持有，侧栏按 Session 标出需要关注的状态。

## 整个应用仍由原有三层协作

UI 负责呈现和收集用户意图，Agent 负责业务事实和执行，Bridge 负责跨运行边界的消息转换与唤醒。主窗口改造不会把任务、会话或记忆的所有权搬进 Bevy ECS。

```mermaid
flowchart TD
    subgraph UI["crates/ui · 展示与交互"]
        App["application · 窗口、配置、生命周期"]
        Workspace["chat_ui · 主工作区与辅助窗口"]
        Host["agent_host · 命令、事件与 Winit 唤醒"]
        Assets["assets · 字体、图标与图片资源"]
        App --> Workspace
        App --> Host
        Workspace --> Assets
        Workspace --> Host
    end

    subgraph Bridge["crates/bridge · 跨运行边界"]
        Conn["connection · 唯一连接与操作关联"]
        AccountBridge["account"]
        ConversationBridge["conversation"]
        ModelBridge["llm_model"]
        Host --> Conn
        Conn --> AccountBridge
        Conn --> ConversationBridge
        Conn --> ModelBridge
    end

    subgraph Agent["crates/agent · 业务与执行"]
        Account["account / login"]
        Conversation["conversation / session_store"]
        Runtime["runtime / turn_runner"]
        Model["llm_model"]
        Skills["skills"]
        MCP["mcp"]
        Memory["memory"]
        ProjectContext["project_context"]
        AccountBridge --> Account
        ConversationBridge --> Conversation
        ModelBridge --> Model
        Runtime --> Conversation
        Runtime --> Account
        Runtime --> Model
        Runtime --> Skills
        Runtime --> MCP
        Runtime --> Memory
        Conversation --> ProjectContext
    end
```

现有职责继续成立：

- `crates/ui/application` 管理通用应用配置、窗口和插件装配、诊断及整体生命周期。
- `crates/ui/chat_ui` 管理聊天工作区、模型准备、输入和展示投影；它不持有会话历史或执行线程。
- `crates/ui/login_ui` 管理账号界面及其 Bevy / Bridge 衔接。
- `crates/ui/agent_host` 持有共享执行连接、命令投递、业务消息分发和 Winit 唤醒。
- `crates/ui/assets` 统一注册字体、图标和图片资源。
- `crates/agent/conversation` 管理 Session 设置快照、Turn、协议历史和任务登记。
- `crates/agent/session_store` 管理 Session 存档、恢复、提交确认和存储收尾；Project 元数据与 Session 归属也由 Agent 侧持久化所有者管理。
- `crates/agent/runtime` 管理 Tokio 运行环境、跨 Session 后台工作、取消、回收和执行收尾。
- `crates/agent/turn_runner` 推进模型请求与工具调用。
- `crates/agent/account` 管理当前账号状态、凭据请求和执行身份；`crates/agent/login` 管理 OAuth 与凭据持久化。
- `crates/agent/llm_model` 管理模型目录、请求、上下文限制和响应 usage。
- `crates/agent/skills`、`crates/agent/mcp`、`crates/agent/memory` 分别拥有 Skill、MCP 和长期记忆业务事实。
- `crates/agent/project_context` 仍然按 Session 的 `working_directory` 发现 `AGENTS.md` 等项目规则。

跨领域依赖方向保持 `ui → agent`、`ui → bridge`、`bridge → agent`。Agent 不依赖 Bevy、UI 或 Bridge；Bridge 不持有 Agent 业务状态，也不依赖具体界面实现。

## 主窗口

主窗口固定为 **1920 × 1080**。UI Scale 继续由应用配置控制，默认值为 **1.25**。标题栏空白区域用于拖动窗口，右侧保留最小化和关闭按钮。窗口边缘和标题栏双击均不改变窗口尺寸。

标题栏从左侧直接显示登录、设置、Skill、MCP 和记忆管理入口。登录入口使用用户头像。左侧 Session 侧栏与右侧聊天区按 **3:7** 分配可用宽度，中间保留一条细分隔线。Widgetry 窗框绘制四角。主窗口、管理窗口、文件选择器和消息框共用标题栏控制配色，悬停显示深绿色底色，按下时进一步加深。文件选择器和消息框的内容底色透明。窗框在 UI 内以线性颜色绘制，提交给 Windows 桌面前将半透明像素转换为 sRGB 预乘颜色，使四角边缘正常与桌面背景混合。

## Project 让相关 Session 有组织地放在一起

侧栏使用一棵可滚动的项目树。拥有 Project 归属的 Session 展示在对应项目下。内置「单独聊天」固定排在全部用户项目之后，展示没有 Project 归属的 Session。该节点没有持久项目记录、绑定目录和项目管理菜单。深鼠尾草绿色标出聊天区当前打开的会话。新聊天仍是草稿时，高亮所属项目或「单独聊天」。

Project 组织 Session。每个 Session 独立保存聊天历史、模型、推理强度和工作目录。

悬停在分区标题时显示创建项目入口。项目行以文件夹图标标识，悬停时显示新聊天入口。用户项目另有重命名和删除菜单。Session 菜单提供重命名、归档和删除。归档将会话移出常规导航与启动默认选择，保留本地历史及项目关系。

### 创建 Project 时必须选择文件夹

确认一个已经存在的本地文件夹后，直接以文件夹名称创建 Project。Project 保存稳定 ID、名称、目录路径和所属 Session 关系。

用户项目和内置「单独聊天」都先在主聊天区打开临时草稿。草稿绑定入口所属的分组，同一进程内切换聊天时会保留。首次提交后，Runtime 在该分组中创建会话并发送消息。首条输入保存时生成本地标题，后台模型生成简短标题后更新列表，手动名称优先。

Project 目录是项目内新 Session 的默认工作目录。它不等于 Session 自己的工作目录快照，也不等于 `project_context`：

- 在 Project 中新建 Session 时，默认将 Project 目录写入该 Session 的 `working_directory`。
- Session 的工作目录随会话设置保存在存档中。
- 删除 Project 时，所属 Session 自动转换为单独聊天；Session 历史和外部文件夹均保留。

Project 元数据保存在 Agentelier 应用数据目录，不写入用户选择的项目文件夹。删除 Project 只删除组织关系，不删除项目目录内容。

`project_context` 继续以 Session 的 `working_directory` 为根发现 `AGENTS.md` 等规则。Project ID 不会触发额外项目上下文扫描，也不会恢复已经移除的 Project Skill。

路径显示使用统一路径展示能力，Windows 路径不出现 `\\?\` Verbatim 前缀；文件操作仍使用原始 `PathBuf`。

### Session 切换只改变当前聊天

选择 Session 后，D 加载该 Session 的聊天历史、草稿、模型设置和阅读位置。不同 Session 的这些状态彼此独立。

UI 在 Agent 确认打开目标 Session 后替换 D 区内容。其他 Session 继续生成和保存回复，侧栏分别显示其忙碌状态。用户可以在当前 Session 发送或停止，该操作只作用于这个 Session。

## D 让用户与当前 Session 交流

项目侧栏和聊天区共用一块铺满工作区的半透明底面。项目树的容器透明，交互状态在条目上显示。消息展示由 Agentelier 的 `ConversationView` 负责，`ChatComposer` 位于聊天区底部。消息行在共享底面上排版，头像、文字和交互控件保持独立层级。

### 消息头像与正文分成两行

每条消息的头像独占一行，正文从下一行开始：

- 用户头像在右侧，用户正文靠右。
- 助手头像在左侧，助手正文靠左。
- 正文与头像保持同侧对齐。
- 长篇 Markdown 和代码使用聊天区域的可用宽度。

```mermaid
flowchart TB
    subgraph User["用户消息 · 右侧对齐"]
        direction TB
        UAvatar["用户头像"]
        UBody["消息正文"]
        UAvatar --> UBody
    end
    subgraph Assistant["助手消息 · 左侧对齐"]
        direction TB
        AAvatar["助手头像"]
        ABody["消息正文"]
        AAvatar --> ABody
    end
```

`ConversationView` 展示用户消息、附件和助手最终回复，使用 `agentelier_markdown` 渲染正文。当前轮次接纳后立即显示助手头像和等待圆环，收到首段最终回复正文时在同一消息行切换为正文。明确标记为 `final_answer` 的消息支持流式更新。工具过程和请求状态保存在原有会话记录中。

流式输出只更新受影响的消息内容，不重建整个消息列表。用户向上阅读时，新内容不强制滚到底部；只有用户原本处于底部跟随状态时才跟随新消息。消息更新不能抢输入焦点、清空草稿或破坏文本选择。

### 聊天输入区

主聊天和记忆聊天在输入框聚焦时使用 Enter 提交和 Shift+Enter 换行。主聊天输入框下方显示当前会话的只读状态，包括实际模型、推理强度、项目路径和会话名称。新聊天草稿显示首次发送会使用的默认模型、推理强度和目录，创建会话后补上名称。单独聊天显示实际工作目录。停止按钮在回复生成时显示。

默认模型为 `gpt-6.2-sol`，默认推理强度为 `high`。这两个默认值只在创建新 Session 时注入 `SessionSettings`；已有 Session 保留自己的模型和推理强度。用户在 D 区切换模型只改变当前 Session。

新建普通聊天时由 Runtime 加载应用数据目录的 Origin 人格。文件读取或校验失败时，原聊天保留并显示失败原因，首次发送的草稿保留。指令来源和摘要沿用会话的请求核对入口。

请求核对中的「本次请求的情境指令」显示该请求实际使用的动态 Developer 材料及其位置和 SHA-256。「实际组装顺序」同时显示该材料与固定指令、项目正文、记忆和 Skill 的相对位置。

发送、Steer、Queue、Stop、Retry 和结果续接继续走现有 Conversation / Runtime 语义。Composer 不拥有 Turn、任务或协议历史。

## Session 条目显示执行状态

Chat UI 按 Session ID 合并当前聊天状态和 Runtime 提供的全局任务投影。模型回复、待执行输入或未完成的工具任务显示转圈。侧栏和消息区的可见圆环在事件驱动的窗口中主动请求重绘。失败、触及限额或需要确认的状态优先显示红色圆圈内的红色感叹号，空闲条目不显示状态符号。工具结果保留在会话记录中。

独立后台 Session 随发起聊天组织。主聊天区的只读浮窗按当前 Session 显示正在执行、待执行或需要查看的异常任务，切换聊天更新列表。浮窗可在聊天区内拖动并折叠，状态来自 Runtime 和本地存档。成功后的结果仍可从原聊天查询。任务操作通过原聊天提交，删除确认同时显示关联任务数量。

### 任务通知只打断真正需要关注的时刻

任务状态发生以下变化时，显示小型非阻塞通知：

- 任务成功。
- 任务失败。
- 任务需要用户进行操作。

普通运行、排队、等待资源和取消过程不弹通知。任务成功通知在 Agent 确认成功终态后产生；任务失败通知在失败终态确认后产生；需要操作通知只在进入明确的用户操作等待状态时产生，不把普通资源等待当成用户操作请求。

通知复用 Widgetry 自定义 Window，不另建底层原生窗口系统。窗口非模态，显示所属 Session、任务名称和准确状态，最长显示 5 秒，用户可以手动关闭。多个通知同时到达时依次排列，避免覆盖；同一 Task ID 的同一终态只通知一次。关闭通知只关闭窗口，不改变任务。

## 辅助窗口提供低频管理能力

标题栏入口打开设置、Skill、MCP 和记忆管理窗口。Skill、MCP 和记忆管理窗口共用只带关闭按钮的固定尺寸窗口行为。标题栏支持拖动，双击不会最大化。

自定义窗口标题文字与左侧窗框之间留有间距。文件和目录选择窗口的标题栏只显示关闭按钮，双击标题栏不会最大化。

### 登录窗口

登录按钮在未登录时显示默认用户头像；登录后若设置了用户头像则显示该头像，否则继续显示默认头像。

未登录时左键点击，弹出类似 Widgetry MessageBox 的登录窗口，显示浏览器授权链接，供用户复制后自行粘贴到浏览器。应用不自动打开系统浏览器。已登录时左键点击不执行任何操作。已登录时右键点击显示菜单，当前只有“登出”；未登录时右键点击无操作。

登录与登出继续由 `agent/login`、`agent/account` 和现有 Bridge 流程完成。UI 不读取或写入 OAuth 凭据。

### 设置窗口与全局配置

设置窗口使用固定尺寸，标题栏只显示关闭按钮。设置项在窗口内直接排列。

| 设置项 | 默认 / 生效方式 |
|---|---|
| 应用数据目录 | 显示当前目录，更改下次启动生效 |
| UI Scale | 默认 1.25，保存成功后立即生效 |
| 默认模型 | 初始值为 `gpt-6.2-sol`，选择后自动保存，用于新 Session |
| 默认推理强度 | 初始值为 `high`，选择后自动保存，用于新 Session |
| 用户头像 | 选择 PNG 或 JPEG，保存圆形裁剪 |

用户项目的新会话继承项目目录。内置「单独聊天」的新会话使用应用启动目录。

设置文件继续使用程序目录下的 `config/config.ron`。当前 `AppConfig` 已有 `scale_factor`、`data_directory` 和 BRP 配置，并通过同目录临时文件、同步后替换目标文件。新增设置沿用同一个 ConfigStore：每次更新基于最新完整配置，校验后原子保存；只有落盘成功才更新内存配置。保存失败保留旧配置并向用户显示错误。应用数据目录变更只影响下次启动，不迁移当前运行中的数据环境。

#### 用户头像保留原图，裁剪只保存显示变换

点击头像时打开 Widgetry 文件选择窗口。选择 PNG 或 JPEG 后在独立窗口显示原图、可移动和缩放的圆形裁剪选区以及实时预览。保存后应用裁剪，取消或关闭编辑窗口时丢弃草稿。头像右上方提供浅粉色的圆形移除按钮，与头像留有间距。

存入应用数据目录的是用户上传的原始图片，不生成破坏性的裁剪副本。配置只保存图片引用和裁剪变换参数，例如归一化偏移与缩放值。标题栏登录按钮和聊天区用户消息头像共用同一份图片及变换。没有头像或图片读取失败时使用默认用户头像。

### Skill 管理窗口

Skill 窗口展示当前可用 Skill，并提供添加和删除按钮。用户 Skill 目录固定为应用数据目录下的 `skills/`。

Skill 来源只保留两种：

- **用户 Skill：**位于用户 Skill 目录，可添加和删除；内容可以通过外部编辑器或聊天让助手维护。管理窗口不提供正文编辑器。
- **内置 Skill：**只保留 `skill-creator`，由应用维护，不可删除。

不再发现或使用项目 `.agents/skills/`。项目中已有的文件保留在原位置，不由 Agentelier 删除。

Skill 数量不会让所有正文自动进入上下文。发现阶段只提供名称、描述等必要元数据；完整 `SKILL.md` 和 references 在实际使用时按需读取。

#### 内置 skill-creator 使用 Rust 与编译期资源

`skill-creator` 不再依赖 Python、PyYAML 或运行时脚本。资源在编译期嵌入程序，不导出到 `.system/<hash>` 或其他磁盘缓存。

模型继续负责理解需求和创作 Skill 内容；Rust 负责确定性操作：名称规范化、目录初始化、生成 frontmatter 与 `agents/openai.yaml` 模板、验证 YAML、检查目录名称和资源引用，以及防止意外覆盖。内置 `SKILL.md` 和 references 通过 Agent Skills 的受控资源读取通道提供，不能把一个不存在的磁盘路径交给 `exec_command`。

保留内置 Skill 的来源说明与 Apache-2.0 许可材料。

### MCP 管理窗口

MCP 窗口展示所有已配置 Server，并提供主动添加、编辑、启用/禁用、删除、连接状态和错误查看，以及 Tools / Resources / Prompts 能力查看。

支持两种传输：

- **STDIO：**命令、参数、工作目录和允许传入的环境变量。
- **Streamable HTTP：**Endpoint URL 和当前支持的认证配置。

添加 MCP 是登记连接配置，不等于安装或下载第三方 Server。本阶段不提供 MCP 商店或自动安装器。

配置状态、启用状态和连接状态分别表达。配置保存成功不代表连接成功；连接失败时保留配置并显示真实错误。MCP Server 启动本身不直接占用模型上下文；进入 Session ToolCatalog 的 Tools 声明会随模型请求发送并产生上下文开销。Resources 和 Prompts 不自动注入，由用户选择、预览并确认后才成为会话材料。

配置变化影响之后创建的 Session；已有 Session 保持创建时冻结的 ToolCatalog。MCP 配置和连接生命周期仍由 `crates/agent/mcp` 管理，UI 不自行启动 Server 或创建第二个 MCP Client。

### 记忆管理窗口使用专用 Session

记忆窗口借鉴 ChatGPT 网页版的记忆摘要体验：

- 顶部显示“记忆摘要”和最近更新时间。
- 中间显示已发布的只读摘要、详细记录和来源。
- 底部显示与助手交流的多行输入框，聚焦后按 Enter 提交。

记忆窗口聊天使用专用 Session，不使用 C 区当前选中的 Session。专用 Session 在当前程序运行期间唯一；第一次打开记忆窗口时按需创建，之后重复使用。下次启动程序时创建新的专用 Session。它不出现在 Project / 单独聊天列表中，也不改变用户当前选择的 Session。

摘要随打开窗口、聊天和后台发布自动更新。记忆摘要、详细记录和来源只读，可选择和复制，不提供手动编辑器、单条删除或“重置长期记忆”。用户通过聊天明确表达添加、纠正、忘记或查询意图，由 Agent 的记忆查询和显式修订能力完成。普通提问只查询，不自动视为记忆修改请求。

专用 Session 为 Memory 修订提供真实的用户原话、Session ID、Turn 和记忆纪元，继续走 Conversation / Runtime / Memory 工具链。修订保存与摘要发布分开显示：修订已保存但等待整合时，界面显示等待状态；只有新摘要发布成功后才更新摘要和更新时间。

专用 Session 的历史作为内部会话保留，不显示在 C 区；下一次程序启动创建新的 Session，不恢复为上一轮运行的活动记忆会话。

## Agentelier 项目级控件建立在 Widgetry 之上

页面不应该到处重新组合 Widgetry 的 Button、颜色、尺寸和状态反馈。高频复用的基础控件先封装成 Agentelier 自己的控件，页面和业务组合控件再使用它们。

### 基础控件

- **`AgentelierButton`：**统一按钮外观和交互状态，提供圆形图标、普通、次要和危险样式。标题栏和辅助窗口复用此按钮。
- **`AgentelierAvatar`：**统一头像尺寸、圆形 Mask、裁剪变换、默认头像回退和资源加载表现。
- **头像裁剪：**头像编辑窗口中的圆形选区与实时预览，复用图片预览与变换数据。

### 组合控件

- **`ConversationView`：**聊天消息、头像、Markdown、流式更新和阅读位置。
- **`ChatComposer`：**多行输入、当前 Session 状态、发送/停止。
- **`SessionNavigator`：**Project / Session 树形导航与选择。
- **`TaskToast`：**使用 Widgetry Window 展示的终态通知。
- **`LoginContextMenu`：**已登录时登录按钮的右键登出菜单。

这些控件属于 Agentelier 自己的 UI 层，不是新增 Widgetry Crate，也不要求修改 `bevy_widgetry` 上游。

### Widgetry 公开能力复用

| Widgetry 能力 | Agentelier 用途 |
|---|---|
| Button / ButtonHeadless / Loading | `AgentelierButton` 的交互与自定义外观 |
| Theme / ColorOverrides | 深绿黑与翡翠绿主题 |
| Core Text / Icon / Font | 字体、图标与 Emoji fallback |
| TextField / ReadOnlyTextField | 聊天输入、授权链接和只读记忆 |
| ComboBox | 模型、推理强度和 MCP 传输选择 |
| Tree / ListView | Project / Session、Skill 和 MCP 列表 |
| Collapsible | 新会话目录、记忆详情 |
| ScrollArea | 聊天、列表和记忆内容滚动 |
| Window | 设置、Skill、MCP、记忆和 TaskToast |
| MessageBox | 登录说明、确认与错误 |
| FileDialog | Project 目录和 Skill 目录选择 |
| Ui Scale | 配置驱动的应用缩放 |

Widgetry Theme 管理颜色，不负责字体、尺寸、布局或动效。需要项目特化外观时，优先使用公开 Headless、Renderer、Slots、Scene 组合和自定义交互策略。TaskToast 复用 Widgetry 自定义 Window，Agentelier 只管理通知内容、显示时机、队列和自动关闭。

所有 UI 构造遵循项目 BSN 规则：

- UI 结构通过 `bsn!`、Scene 和 SceneList 声明、构造和组合。
- 只有需要独立、持久 ECS 身份时才使用 `SceneComponent`；普通复用结构使用返回 `impl Scene` / `impl SceneList` 的函数。
- Props 只负责 Scene 创建时的初始化；运行期状态保存在持久 Component / Resource 中。
- 调用方提供的 Children 通过 Scene Composition 传入，不用 `spawn_*` / `build_*` Helper 建立替代构造 API。
- 使用 Widgetry 时遵循公开 API 对受管状态的契约，不直接修改其内部 Component。

## 本阶段不包含的内容

- System Tray。
- Splash。
- Agent 自主 UI 操控或自主修改 UI。
- Project Skill。
- 提示词管理页面。
- Skill 商店和 MCP 商店。
- 记忆重置、手动编辑和手动删除。
- 聊天附件 `+` 按钮与语音输入。

## 相关设计与规范

- [总体架构](../architecture.md)
- [账号界面](../modules/account-ui.md)与[认证运行](../modules/authentication.md)
- [Conversation 与 Session 设置](../modules/conversation.md)
- [Session Store](../modules/session-store.md)
- [Project Context](../modules/project-context.md)
- [Skills 设计](../skills/README.md)
- [MCP 设计](../mcp-extension-tools.md)
- [长期记忆设计](../mechanisms/long-term-memory.md)
- [异步任务交互与集成](../native-async-tools/05-interaction-and-integration.md)及[验证清单](../native-async-tools/06-validation.md)
- [架构规则](../../../rules/architecture.md)、[UI 构造规则](../../../rules/ui-composition.md)、[GUI 验证规则](../../../rules/gui-debugging.md)、[项目约束](../../../rules/project-context.md)

`bevy_widgetry` 的公开能力参考：[Theme](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/theme.md)、[Button](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/button.md)、[Window](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/window.md)、[Splitter](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/splitter.md)、[Tree](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/tree.md)、[ListView](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/list_view.md)、[Ui Scale](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/ui_scale.md)。
