# 项目资料与约束

仓库中的资料按以下职责区分：

- `rules/`：工程约束，包括开发流程、代码规范、架构边界、验证与提交要求。
- `docs/`：设计文档、功能文档和已知问题。
- `plans/`：方案规划、探索结论与历史设计背景。

所有仓库修改均须遵守 `rules/` 中的适用规则。

`plans/` 不作为当前实现事实，也不需要默认读取。只有任务涉及历史设计依据、已有方案或相关探索结论时，才按需查阅。

根据任务目标、涉及范围和实际变更选择相关资料，不机械读取全部规则与文档。

如果任务要求与现有工程规则发生实质冲突，应先明确冲突并解决设计问题，不得自行放宽规则。

# 规则导航

## 基础规则

- `rules/project-context.md`：项目背景、目标环境、平台边界与 UI 构造原则。代码开发须读取。
- `rules/task-scope.md`：限定当前任务的修改范围，避免无关改动。
- `rules/development.md`：开发流程、任务进度记录、工程化开发方法及专项验证衔接。

实际修改仓库前，必须按照 `rules/development.md` 创建并维护任务进度记录。纯只读分析不需要创建记录。

## 专项规则

根据任务实际涉及的内容读取：

- `rules/code.md`：Rust 代码规范、错误语义、命名及 Lint 约束。
- `rules/architecture.md`：Workspace、Crate 职责、依赖管理、Assets 与源码组织。
- `rules/ui-composition.md`：Bevy BSN、Scene、Props、Runtime ECS 与 UI 组合边界。
- `rules/documentation.md`：代码注释、Rustdoc 及文档规范。
- `rules/gui-debugging.md`：GUI 运行时行为及 BRP 自动验证。
- `rules/logging.md`：日志职责、记录条件、日志内容与持久化。
- `rules/git.md`：Git 提交范围、变更分组及 Commit Message。
- `rules/common-commands.md`：常用开发与验证命令，命令列表仅供按需选择，不代表每次修改都必须执行全部命令。

相关规则可以同时适用。规则选择应依据真实任务内容和变更影响，而不是仅凭文件所属目录判断。

# 文档同步

设计、功能或已知问题变化时，按 `rules/documentation.md` 同步受影响的文档。
