# 架构规则

## Workspace / Crate 职责与依赖管理

当前整体设计与模块职责关系见 `docs/design/architecture.md`，模块核心设计见 `docs/design/modules/`。本节约束 crate 的职责边界、依赖方向及依赖管理方式。

### Crate 职责与依赖方向

- `app/`：仅保留可执行包所需的 `Cargo.toml` 和程序入口 `src/main.rs`。`main.rs` 只承担入口级平台声明并调用 `crates/` 提供的启动接口，不实现业务、配置、窗口/UI 装配、诊断或生命周期协调，也不声明其他功能 Module。下层库 crate 不得依赖 `app`。
- 所有功能实现必须放在 `crates/`，按真实职责归入 `ui/`、`agent/`、`bridge/` 三个领域。领域目录只是架构边界，不是 Rust Crate，不预先创建空库，也不建立承担整个领域功能的总包。
- `crates/ui/application/`：通用应用宿主，负责配置、日志、诊断、窗口/插件装配与整体启动退出协调，不接管具体账号界面、认证协议或模型执行。
- `crates/ui/login_ui/`：目录、账号界面，以及相应的 Bevy/Bridge 交互衔接和浏览器交互。
- `crates/ui/agent_host/`（`agentelier_agent_host`）：共享执行连接、Winit 唤醒、命令转交、业务消息分发和执行收尾。
- `crates/ui/chat_ui/`（`agentelier_chat_ui`）：模型准备、聊天输入和展示投影，不接管认证、HTTP、会话权威或运行线程。
- `crates/ui/assets/`：统一管理应用 Asset。
- `crates/agent/login/`（`agentelier_login`）：OAuth 协议、Token、认证文件持久化及数据环境，不承担统一运行调度。
- `crates/agent/account/`（`agentelier_account`）：账号操作与会话运行状态、共享凭据请求、登录/恢复/续期/退出协调，具体协议和文件读写由 Login 实现。
- `crates/agent/llm_model/`（`agentelier_llm_model`）：模型服务通信、模型目录、Responses 和 SSE，通过 Account 取得凭据，不维护第二份账号状态或读写认证文件。
- `crates/agent/conversation/`（`agentelier_conversation`）：聊天会话、设置快照、Turn、展示与历史业务，不接管线程或网络。
- `crates/bridge/conversation/`（`agentelier_conversation_bridge`）：聊天请求与安全展示投影，不持有会话权威状态。
- `crates/agent/runtime/`（`agentelier_agent_runtime`）：Tokio 执行环境、跨业务任务调度、取消、回收与运行生命周期，不包含具体 OAuth 或模型网络协议。
- `crates/bridge/account/`（`agentelier_account_bridge`）：账号安全投影及认证业务事件转换。
- `crates/bridge/llm_model/`（`agentelier_llm_model_bridge`）：模型结果、错误分类、取消和完成状态转换。
- `crates/bridge/connection/`（`agentelier_bridge_connection`）：跨运行边界连接、命令/事件投递、操作关联、界面唤醒及通信生命周期，不承接具体业务投影和错误分类。

跨领域允许 `ui → agent`、`ui → bridge`、`bridge → agent`；禁止 `agent → ui`、`agent → bridge`、`bridge → ui`。UI 可以直接使用 Agent 业务能力，需要转换或跨运行边界协调时使用对应 Bridge，不强制所有访问经过 Bridge。

领域内部不得循环依赖。Runtime 依赖 Account 和 LLM Model；LLM Model 依赖 Account，Account 依赖 Login。Bridge 不依赖 Bevy 的具体界面实现，不拥有 Agent 执行状态或 UI 展示状态。

新增能力先判断领域，再按真实职责复用或建立 Crate。既有 Crate 不因领域名称就承接所有新功能，也不为每个类型或小功能机械建库；独立边界须能以功能和依赖解释。

### 外部依赖管理

能够统一管理的外部依赖，其版本在根目录 `Cargo.toml` 的 `[workspace.dependencies]` 中定义。

Workspace 成员通过 `workspace = true` 引用，不得在多个 Crate 中分别维护同一外部依赖的版本。

各 Crate 可以根据实际需要配置自身使用的 Dependency Feature，不要求启用完全相同的 Feature 集合。

新增依赖必须服务于当前明确需求，不得仅为了实现方便或假设中的未来用途引入依赖。

### 上游问题处理

当问题根源位于 Bevy 或其他上游依赖时，优先通过公开 API、配置调整或其他不修改上游源码的方式解决。

禁止为了修复上游问题而引入或维护修改版上游源码，包括：

- 将上游源码全部或部分复制、Vendor 到当前仓库。
- Fork 上游仓库并自行修改，通过 Cargo Patch、Git Dependency 或其他方式使用修改后的 Fork。

允许正常升级至上游官方发布的修复版本。

如果无法在上述约束内合理解决问题，则保留该问题，不得通过引入修改版上游源码强行修复。

未解决的问题记录在 `docs/known_issues.md`，包含问题现象、涉及的上游依赖及版本、实际影响、已确认的原因，以及暂不解决的理由。未经证实的原因应明确标为推测，不得作为既定事实。

问题不影响的其他任务内容可以继续完成，但必须如实报告相关限制。

### 内部依赖与命名

Workspace 内部 Crate 默认使用 Path Dependency 并遵守必要的职责与依赖方向约束。目录用简洁、能反映职责的名称；Package 名以实际语义为准。

## Assets

应用 Asset 统一由 `crates/ui/assets/` 管理，资源文件位于 `crates/ui/assets/src/assets/`，`crates/ui/assets/src/lib.rs` 公开 `AppAssetPlugin` 负责应用需要的注册。

其他代码通过语义名称引用应用 Asset，不到处散布物理/Embedded 路径；Asset 的注册和路径选择集中在实际的 Asset Owner 中。

新增独立库确实需要自有 Asset 时，根据其职责决定所在位置，不预设统一的跨 Crate Asset Owner 规则。

## 源码与 Module 组织

### Crate 入口

Library 使用 `lib.rs`，Application 使用 `main.rs`。`lib.rs` 主要负责 Module 声明、Re-export 和必要的 Crate 级装配，不堆放大量具体实现。

### Module 组织

Module 根据实际职责和语义划分，不机械采用一种 Type 对应一个文件的结构。

不得仅因为文件较长、希望目录整齐或希望各 crate 结构一致，就拆分或重组 Module。

源码采用现代 Rust Module 文件组织方式，不使用 `mod.rs`：

```text
src/
├── lib.rs
├── foo.rs
└── foo/
    ├── bar.rs
    └── baz.rs
```

### Visibility

声明使用满足当前需求的最小 Visibility：

- `private`：仅当前 Module 使用。
- `pub(crate)`：仅 Crate 内共享。
- `pub`：明确需要跨 Crate 访问或属于对外 API。

不得仅为了实现方便而扩大 Visibility，也不得在普通开发任务中顺手增加公共 API。
