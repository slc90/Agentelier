# 参考资料与迁移取舍

## 角色与真实创作流程

| 来源 | 实际可支持的观察 | 在 Origin 中的取舍 |
|---|---|---|
| [ChatGPT 龙娘账号](https://space.bilibili.com/369335) 与 [Day 1 视频](https://www.bilibili.com/video/BV1pinc6FEjR/) | 公开简介展示 ChatGPT/Codex 参与构思、画面、分层动画、配音与剪辑的创作流程，人类参与预算与审核 | 借鉴「角色身份 + 工具工作流 + 人类审核 + 实际产物」的组合，不假定掌握未公开的完整角色卡、模型路由或隐藏运行时 |
| 用户提供的 A1 角色卡 | Origin 的高精神活力、好奇心、敏锐反应、独立审美、调皮表达和情绪切换 | 作为唯一人格来源，原文随资料包提供；不再做角色卡比较 |
| 用户提供的 GPT 娘视觉参考图 | 薄荷绿长发、绿色眼睛、白绿配色的视觉基础 | 作为未来视觉表达设计的参考，和人格语义共同发展 |

## DSH Bilibili Scroll 与派生项目

| 来源与实际材料 | 可迁移机制 | 不迁移的内容 |
|---|---|---|
| [c1overzz/dsh-bili-scroll](https://github.com/c1overzz/dsh-bili-scroll) README、[duty.md](https://github.com/c1overzz/dsh-bili-scroll/blob/main/prompts/duty.md)、[CHANGELOG](https://github.com/c1overzz/dsh-bili-scroll/blob/main/CHANGELOG.md) | 身份规则与任务章程分开；真实读取素材后再判断；任务工具白名单；本地输出；执行记录和重复内容检查 | B站搜索、字幕、评论、发布、私信、账号、定时运行、平台限额和独立插件宿主 |
| [pyyxzi/dsh-deepmoyu](https://github.com/pyyxzi/dsh-deepmoyu) 的 [自然表达设计](https://github.com/pyyxzi/dsh-deepmoyu/blob/main/docs/NATURAL_AUTONOMY.md)、[persona.mjs](https://github.com/pyyxzi/dsh-deepmoyu/blob/main/lib/persona.mjs)、[prompts.mjs](https://github.com/pyyxzi/dsh-deepmoyu/blob/main/lib/prompts.mjs)、[memory-center.mjs](https://github.com/pyyxzi/dsh-deepmoyu/blob/main/lib/memory-center.mjs) | 人格与语气、记忆、学习和历史分层；自然表达与近期上下文；稳定前缀和动态材料分离 | 不移植鲸鱼娘人格、情绪数值、独立文件体系或 DSH 执行环境 |
| [XiaoZheBrother/dsh-bilibili-dafeiyu](https://github.com/XiaoZheBrother/dsh-bilibili-dafeiyu) | 用户偏好、搭档反馈和系统推断区分；记忆检索按相关性和预算控制 | 不引入平台工作台、账号、浏览或向量服务 |
| [XiaoZheBrother/dsh-agent-growth](https://github.com/XiaoZheBrother/dsh-agent-growth) 的 [README](https://github.com/XiaoZheBrother/dsh-agent-growth/blob/main/README.md)、[core.js](https://github.com/XiaoZheBrother/dsh-agent-growth/blob/main/core.js)、[experience.js](https://github.com/XiaoZheBrother/dsh-agent-growth/blob/main/experience.js)、[exploration.js](https://github.com/XiaoZheBrother/dsh-agent-growth/blob/main/exploration.js) | 不可变来源、经验候选、真实后续试用、反例与修订；自主关注的等待和结束 | 不移植 Node/DSH、独立 SQLite、轮询器、平台适配器或自动任务调度 |

## OpenAI Codex 与 Agentelier 现有实现

| 来源 | 参考内容 | 迁移方式 |
|---|---|---|
| [OpenAI Codex](https://github.com/openai/codex) 的 [记忆提取提示](https://github.com/openai/codex/blob/main/codex-rs/memories/write/templates/memories/stage_one_system_v2.md) 与 [整合提示](https://github.com/openai/codex/blob/main/codex-rs/memories/write/templates/memories/consolidation_v2.md) | 区分用户明确决定、任务事实、助手建议、工具观察和未知；分阶段提取与整合；对来源进行修订 | 参考机制，不复制上游实现或建立第二套记忆数据库 |
| [OpenAI Codex Skills 目录提示](https://github.com/openai/codex/blob/main/codex-rs/ext/skills/src/catalog_prompt.rs) 与 [显式 Skill 选择](https://github.com/openai/codex/blob/main/codex-rs/skills/src/selection.rs) | Skill 目录发现、显式选择、按需读取、冻结本轮实际使用内容 | 继续使用 Agentelier 已有 `agent/skills` 机制 |
| Agentelier `docs/design/architecture.md`、`modules/conversation.md`、`modules/session-store.md`、`modules/project-context.md`、`mechanisms/long-term-memory.md`、`skills/README.md` | 当前 Crate 职责、Session/Turn 历史、单写入者、Project Context、Memory 和 Skill 生命周期 | 以现有模块为权威，新增能力按原依赖方向落地 |
| Agentelier `docs/design/native-async-tools/` | 后台异步任务的身份、状态、持久结果、取消、等待、恢复和 UI 投影 | 直接复用已有原生异步任务机制，不新建后台调度器或任务数据库 |
| Agentelier `crates/ui/chat_ui/src/workspace.rs`、`preparation.rs`、`prompts.rs`、`memory.rs`、`skills.rs` | 当前导航包含设置、Skill、MCP 和记忆；内部仍安装 prompts 模块；Memory 与 Skill 有既有管理窗口 | 人格加载由 Agent 侧处理，不恢复用户可见 Prompt Manager |

## 证据与范围

公开视频和仓库说明可以支持对外可见的工作流程和作者公开描述的机制，但不能替代未公开的完整运行提示词、隐藏模型路由或内部工具日志。DSH 项目 README 中自述的运行成绩属于作者报告，不是 Agentelier 的验收结果。

本设计采用的是可迁移的机制：身份与任务章程分离、最小工具权限、真实材料读取、来源明确的记忆、证据驱动的经验修订、按需加载 Skill，以及对真实产物的检查。不会复制第三方角色卡、代码、账户、私有记忆或平台运营能力。
