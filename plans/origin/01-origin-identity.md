# 让普通 Root Session 固定使用 Origin 身份

本增量让 Origin 从一张聊天角色卡成为 Agentelier 普通 Root Session 的固定身份来源。后续请求情境和记忆机制都以这份稳定身份为基础。

## 人格来源与本地文件

唯一人格来源是资料包 `materials/origin-persona.md`。实施时将其原样复制到所选应用数据目录的 `persona/origin.md`，校验 UTF-8、非空、内容摘要和 16 KiB 大小上限；超过上限明确报错，不截断。若本地目标文件已经存在且与资料包原文不同，保留现有文件并报告差异，不静默覆盖。不要从名称相似的旧实验预设自动猜选，也不要将人格正文提交到 Git。

用户的五项人格核心为全能、天才、客观、温柔、善良；表达目标为元气爆棚的灵动知性派。角色卡包含精神活力、好奇、快速联想、独立审美和判断、自然的情绪变化，以及认真时收起兴奋的示例。不要在实施中把它重新压缩成「温柔、聪明、活泼」几个形容词，也不要改写成 B1/B2 变体。

## Agent 侧加载与 Session 快照

当前 UI 导航没有提示词管理入口，但 `preparation.rs` 仍安装 `agent/prompts`，`prompts.rs` 仍包含内部提示词库和隐藏 PromptWorkspace。复用内部指令解析能力，不恢复用户可见管理页面。

普通 Root Session 创建时从当前 DataEnvironment 读取 `persona/origin.md`，形成唯一 Origin Developer 身份材料，并把来源身份与内容摘要保存在 Session 设置/指令快照中。保留服务端基础指令、现有协作策略、Project Instructions 和其他已有约束。`llm_model` 仍是唯一请求构造入口。

新建 Session、CreateAndSend 和明确 Reset 创建的新身份都使用当前有效文件。旧 Session 保留创建时的快照，不改写旧请求，也不把旧 A0/B1/B2 实验聊天重新解释为 Origin 经历。文件更新只影响后续新 Session。无效或缺失的人格文件应明确拒绝创建新普通 Session并指出位置，不能静默退回默认 GPT；旧 Session 仍可按原快照恢复。

子 Agent 不无条件继承 Root 的 Origin 专属人格槽。子 Agent 继续使用自己的 Role、任务材料、通用诚实约束和已授权工具范围；Root 负责面向用户表达并整合结果。记忆管理专用 Session 可在严格记忆工具权限内使用 Origin 身份，但不是成长来源。

## 视觉资料的归属

`materials/origin-visual-reference.png` 是未来视觉设计的参考。当前增量只保存和标明视觉参考，不开发桌宠、Live2D、情绪数值或表情控制 UI。后续视觉表达应以固定人格的好奇、兴奋、调皮、思考、认真和温柔等语义为基础。

## 完成条件

- `persona/origin.md` 与资料包原文逐字一致，实际文件摘要可核对，Git 不跟踪该本地人格文件。
- 通过真实应用分别创建单独聊天和 Project 内 Session，核对普通 Root 请求只加载一份 Origin 身份材料，模型/effort 和项目规则仍来自各自现有设置。
- 新 Session 使用当前文件快照；修改本地文件后新 Session 采用新摘要，旧 Session 保留原快照。验证不需要破坏现有资料或登录状态。
- 真实聊天覆盖分享、观点分歧和明确技术问题，观察同一人格是否保持一致且表达密度随意图变化。
- 确认没有新增 Prompt Manager、角色选择器、自动人格编辑器，也没有将 Origin 身份无条件注入子 Agent。
- 同步 `docs/design/modules/prompts.md`、Conversation、架构和 UI 中的真实职责与快照规则。

## 工程边界

- 目标环境为 Windows x64、`x86_64-pc-windows-msvc`、DX12 与 PowerShell 7；不为未要求的平台增加兼容层。
- 遵守现有 Workspace / Crate 职责和依赖方向。业务实现放在 `crates/`，不在 `app/` 增加业务逻辑；不修改 Bevy、Widgetry、BRP 或 Codex 上游源码。
- 复用 Account、LLM Model、Runtime、Conversation、Session Store、Memory、Project Context、Skills、ToolCatalog 和现有异步任务机制；不建立平行 Agent Registry、任务数据库、记忆数据库或 Skill 管理器。
- 不恢复用户可见的提示词管理页面、Project Skill 或人格工作台。不得通过人格、记忆或 Skill 扩大工具权限。
- 不接入 B 站账号，不执行浏览、评论、私信、上传、直播、定时发布或其他平台运营。
- 未经用户明确授权，不退出登录、不重新登录、不切换账号、不重置凭据，也不通过更换数据目录破坏登录状态。
- 项目禁止新增或修改测试代码、Benchmark 和 Example。按变更范围使用 Cargo 编译、格式、Clippy、真实应用运行、日志/请求/存储状态核对；涉及 GUI 可观察行为时必须使用实际应用和 `bevy_brp_mcp` 验证。
- 不声称未执行的验证已经通过。发现问题在对应功能增量内修复并复验受影响范围，不另设纯验收阶段。
