# Agentelier 工程规范与架构重构方案

## 一、重构目标

本次重构统一完成以下工作：

1. 取消强制独立 Code Review 及配套流程。
2. 限制开发 Agent 只完成已确定的功能及必要验证，不主动添加无关优化和防御性设计。
3. 禁止开发 Agent 未经授权退出登录、重新登录、切换账号或破坏现有凭据。
4. 将 `crates/` 重组为 `ui/`、`agent/`、`bridge/` 三个架构领域。
5. 根据实际业务职责拆分现有 Agent 和 Bridge Crate，避免继续使用职责过于宽泛的聚合库。

全部工作使用一份任务进度记录，最终只创建**一个 Git Commit**。

## 二、工程规范调整

### 取消独立 Code Review

删除 `rules/code-review.md`，清理 `AGENTS.md`、`rules/development.md`、`rules/git.md` 等文件中的强制 Review 要求。

移除 Reviewer Subagent、Findings 修复循环、Baseline 管理及相关进度记录要求。

保留用户明确要求时执行专项 Code Review 的能力。

### 限定开发职责

开发 Agent 应按照已确认的设计完成功能，并执行必要验证。

不得主动扩展需求、为假设中的极端边界条件增加防御逻辑、开展无关重构或额外优化。

完成当前任务所必需的错误处理、关联修改和缺陷修复仍然允许。任务范围外的问题只报告，不擅自修复。

### 登录状态保护

未经用户明确授权，不得主动：

- 退出登录、重新登录或切换账号。
- 发起新的交互式 OAuth 授权。
- 清除、覆盖或重置登录凭据。
- 为调试而破坏现有登录状态。
- 通过更换数据目录等操作间接造成登录失效。

该限制适用于开发、调试与 BRP 自动化验证。

不影响应用正常的账号恢复、按需 Token 续期及用户主动执行的账号操作。

### 保留开发流程

任务进度记录、Type-Driven Development、小步迭代、必要 Cargo 检查、BRP 验证、代码规范、测试禁令及 Git 提交规范全部保留。

取消 Review 不等于取消功能验证。

## 三、目标架构

### 3.1 目录结构

```text
Agentelier/
├── app/
└── crates/
    ├── ui/
    │   ├── application/
    │   ├── assets/
    │   └── login_ui/
    │
    ├── agent/
    │   ├── login/
    │   ├── account/
    │   ├── llm_model/
    │   └── runtime/
    │
    └── bridge/
        ├── account/
        ├── llm_model/
        └── connection/
```

`app/` 继续作为唯一程序入口。

三个领域目录均为架构边界，不是单独的 Rust Crate。领域内的 Crate 数量随实际功能和职责发展，不预先创建空库。

### 3.2 UI 领域

保留现有三个 Crate：

| Crate         | 职责                                             |
| ------------- | ------------------------------------------------ |
| `application` | 通用应用宿主、配置、日志、窗口装配及整体生命周期 |
| `assets`      | 应用 Asset 的统一管理                            |
| `login_ui`    | 数据目录、账号、LLM 连接验证等当前界面与交互     |

本次不拆分 UI 内部已有功能，仅迁移目录并更新必要引用。

### 3.3 Agent 领域

将原 `agentelier_agent` 中混合的职责重新划分。

| Crate       | Package 名称               | 职责                                                       |
| ----------- | -------------------------- | ---------------------------------------------------------- |
| `login`     | `agentelier_login`         | OAuth 协议、Token 处理、认证文件读写及数据环境             |
| `account`   | `agentelier_account`       | 账号操作、会话状态、共享凭据请求和按需续期协调             |
| `llm_model` | `agentelier_llm_model`     | LLM 服务通信、模型目录、Responses 请求、SSE 解析及错误语义 |
| `runtime`   | `agentelier_agent_runtime` | Tokio 执行环境、跨业务任务调度、取消、回收及运行生命周期   |

**Login** 负责具体认证机制与持久化，不承担 Agent 的统一运行调度。

**Account** 管理账号操作和凭据请求的运行状态，协调登录、恢复、续期和退出，具体协议与持久化由 Login 实现。

**LLM Model** 负责与大语言模型服务通信，通过 Account 提供的凭据能力取得认证信息。不直接维护第二份账号状态，也不读写认证文件。

**Runtime** 负责启动和管理 Agent 运行环境，协调 Account 与 LLM Model 的执行及生命周期，不包含具体 OAuth 或 LLM 网络协议实现。

主要依赖关系：

```text
Agent Runtime
  ├── Account ─────→ Login
  └── LLM Model ───→ Account
```

各 Crate 不得形成循环依赖。

### 3.4 Bridge 领域

将当前 `agentelier_bridge` 按业务转换与连接协调职责拆分。

| Crate        | Package 名称                   | 职责                                                             |
| ------------ | ------------------------------ | ---------------------------------------------------------------- |
| `account`    | `agentelier_account_bridge`    | 账号信息、安全展示投影、登录与退出状态及认证业务事件转换         |
| `llm_model`  | `agentelier_llm_model_bridge`  | LLM 请求结果、错误分类、取消及完成状态的业务转换                 |
| `connection` | `agentelier_bridge_connection` | 跨运行边界连接、命令与事件投递、操作关联、界面唤醒及通信生命周期 |

Account Bridge 与 LLM Model Bridge 只承担相应业务转换，不直接执行后台业务。

Connection 负责共用的跨边界传递和协调，不承接具体业务转换逻辑。

未来增加其他桥接能力时，按照实际职责建立或复用 Crate，不要求全部集中到 Connection。

### 3.5 跨领域依赖规则

允许：

- `ui → agent`
- `ui → bridge`
- `bridge → agent`

禁止：

- `agent → ui`
- `agent → bridge`
- `bridge → ui`

UI 可以直接使用 Agent 提供的业务能力，不强制所有访问经过 Bridge。

需要业务数据转换、跨运行边界消息传递或相关协调时，使用对应的 Bridge 能力。

Bridge 不依赖 Bevy ECS 的具体界面实现，不拥有 Agent 执行状态或 UI 展示状态。

### 3.6 Crate 划分原则

新增功能应先判断所属架构领域，再根据实际职责决定使用已有 Crate 或新建 Crate。

已有 Crate 不应仅因为名称属于某个领域，就承担该领域内所有新功能。

同时避免为每个类型、模块或小功能机械增加 Crate。

Crate 应具有明确职责，其独立性应能够通过功能边界和依赖关系解释，而不仅仅是为了目录整齐。

## 四、重构范围

本次允许：

- 移动现有 Crate。
- 根据上述职责拆分 `agentelier_agent` 与 `agentelier_bridge`。
- 为拆分后的 Crate 分配语义明确的新名称。
- 调整内部 API、可见性和必要依赖，使职责划分实际生效。
- 调整必要的任务调度和消息衔接代码，以保持原有功能。
- 更新 Workspace、Path Dependency、Cargo Package 引用和相关文档。

不允许：

- 借职责拆分重新设计 OAuth 或 LLM 通信协议。
- 改变登录、退出、凭据续期的业务语义。
- 改变现有 UI 可观察行为。
- 增加新的账号或 LLM 业务功能。
- 引入与此次拆分无关的重构或优化。
- 创建仅服务于假设中未来需求的占位 Crate。

原有对外可观察行为保持不变，但内部 Crate API 和 Package 名称允许随职责拆分调整。

## 五、实施流程

本次作为一个完整任务实施。

1. **更新工程规范**：取消强制 Review，增加开发范围限制及登录状态保护。
2. **梳理实际职责**：以现有源码和架构文档为依据，确认原 Agent 和 Bridge 中每项职责的新所有者。
3. **重组 Agent**：迁移 Login，提取 Account、LLM Model 和 Runtime，整理依赖关系。
4. **重组 Bridge**：区分账号转换、LLM 业务转换和共享通信协调。
5. **迁移 UI**：调整 Application、Assets 和 Login UI 的目录与依赖引用。
6. **同步文档**：更新 `rules/architecture.md`、`docs/design/architecture.md`、相关模块及机制设计文档。
7. **执行验证并提交**：完成适用检查后，将最终所有变更统一提交一次。

这些步骤是同一个任务内部的工作顺序，不代表拆分为多个 Git Commit。

## 六、验证与完成标准

重构完成后，应确认：

- Cargo Workspace 正确识别所有新 Crate。
- 内部依赖方向符合三领域架构，无循环依赖。
- 原 Agent 与 Bridge 的职责确实完成拆分，而不是仅移动文件或重新导出原有聚合 API。
- 原有程序入口正常启动。
- 原有数据目录与登录凭据不受破坏。
- 应用能够正常恢复已有账号。
- 原有账号状态展示、LLM 连接验证及必要的后台通信行为保持正常。
- 任务取消、后台结束及应用关闭仍遵循原有生命周期。
- 相关 Cargo 检查与适用的 BRP 验证完成。
- 规范中不再存在强制独立 Code Review 的残留要求。
- 文档路径、Crate 名称及模块关系与最终实现一致。

验证期间不得主动退出登录、重新授权或清除凭据。

无法在当前环境下完成的验证应明确记录，不得宣称通过。

最终删除临时任务进度记录，按照现有 Git 规范检查最终 Diff，创建**一个包含全部重构结果的 Git Commit**。

## 七、最终目标

重构后，Agentelier 保持原有运行行为，但代码组织具备明确的领域和职责边界。

UI 管理交互与应用宿主，Agent 的账号、LLM 服务通信和执行能力分别演进，Bridge 按业务类型承担转换与跨运行边界协调。

未来新增功能时，根据真实职责增加或复用 Crate，而不是继续扩大一个职责模糊的总库。
