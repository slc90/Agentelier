# 开放 Entity、Component、Hierarchy 与 Resource 修改

## 目标与产出

在已有 Entity 引用、Reflect 类型信息和 ECS 读取 API 上，开放通用的 World 修改能力。Agent 可以创建和销毁 Entity，添加、修改和移除 Component，调整实体关系，并修改具备反射能力的 Resource。

本增量让 Agent 能真正改变运行中的 ECS 数据，但不把整段 Rhai 脚本变成一个事务。

## Entity 与 Component 操作

| API | 用途 |
| --- | --- |
| `ecs_spawn(components)` | 创建 Entity，并按类型名插入 Component |
| `ecs_insert(entity, type, value)` | 向已有 Entity 插入 Component |
| `ecs_patch(entity, type, fields)` | 修改已有 Component 的指定字段 |
| `ecs_remove(entity, type)` | 移除 Component |
| `ecs_despawn(entity)` | 销毁 Entity |

这些函数复用前一增量建立的 EcsEntity 和 ReflectComponent 适配器。类型名称必须对应当前 World 已注册的 Component 类型；值转换、字段名称和字段类型都在执行前检查。

`ecs_spawn` 将创建实体和初始 Component 插入作为一个完整操作。如果其中任一 Component 无法构造或插入，应清理这次新建的实体，避免留下半成品。`ecs_patch` 在修改前验证目标字段和数据类型；成功后返回实际写入的字段信息或重新读取的值。

`ecs_insert`、`ecs_patch` 和 `ecs_remove` 都是单次操作。一个 Rhai 脚本可以连续调用它们，但多个调用之间不具备事务回滚语义。前面的成功修改会保留；后续操作失败时，Agent 应重新读取状态再决定如何恢复。

## Hierarchy 使用 Bevy Relationship API

| API | 用途 |
| --- | --- |
| `ecs_parent(entity)` | 读取父实体 |
| `ecs_children(entity)` | 读取子实体 |
| `ecs_reparent(entity, parent)` | 将实体挂到新的父实体 |
| `ecs_unparent(entity)` | 解除父子关系 |

修改关系必须通过 Bevy 正常的 Relationship API。不能分别直接写入 ChildOf 和 Children，否则可能产生不一致的关系数据。目标 Entity 和新父实体都需要在操作时重新验证。

Entity 销毁遵循 Bevy 关系生命周期。对于动态 UI，Scene Factory 必须把自己拥有的子实体放在可随实例正常清理的关系范围内。

## Resource 修改

| API | 用途 |
| --- | --- |
| `ecs_resource_insert(type, value)` | 插入已注册 ReflectResource 的 Resource |
| `ecs_resource_set(type, value)` | 替换已有 Resource |
| `ecs_resource_patch(type, fields)` | 修改 Resource 的指定字段 |
| `ecs_resource_remove(type)` | 移除 Resource |

Resource 修改使用 ReflectResource 适配器。未注册反射能力、目标 Resource 不存在、字段缺失或值类型不匹配时，返回结构化 EcsError。不会因为 Resource 是全局数据就绕过类型验证。

Agentelier 的配置文件和 ConfigStore 等具有独立业务生命周期的对象不自动等价于普通 Reflect Resource。只有明确注册了反射适配器的 Resource 才能通过通用 API 修改；具有专门保存或提交语义的业务操作仍由其现有 Native API 负责。

## Widgetry 托管状态

通用 ECS 修改面向已注册的 Component 和 Resource，但不改变 Widgetry 对托管运行时状态的公开契约。某个控件的状态如果由 Widgetry Headless API、Observer、System 或专用状态提交入口维护，应通过相应公开能力修改，而不是绕过控件内部状态机直接改写私有 Component。

普通 Bevy Component、应用自己的 Reflect Component 和布局数据仍可通过通用 API 操作。对于组合控件，完整构造使用后续的 Scene API。

## 完成条件与运行验证

- Agent 能创建一个包含已注册 Reflect Component 的临时 Entity，并读取其初始值。
- Agent 能通过 `ecs_patch` 修改字段，再次 `ecs_get` 确认实际值变化。
- Agent 能添加和移除 Component，并通过 `ecs_components` 确认。
- Agent 能调整父子关系，并通过 `ecs_parent` / `ecs_children` 确认关系一致。
- Agent 能销毁临时 Entity，旧 EcsEntity 引用随后返回 `entity_not_found`。
- 对一个安全、可恢复的 Reflect Resource 完成读取、修改、确认和恢复。具体 Resource 从实际注册类型中选定，避免直接修改账号、会话或布局等有业务副作用的状态。
- 无效 Entity、未知 Component、字段类型不匹配和未注册 Resource 都返回结构化错误。
- 每个操作在 Bevy 主线程执行；Rhai 脚本等待结果期间 UI 仍然响应。
- 在 Windows x64 实际应用中通过 Agent Function Calling 验证；涉及 GUI 变化时按项目规则使用 BRP Screenshot / ECS State。不得编写测试代码、Benchmark 或 Example。

## 保持的边界

- 所有修改继续通过进程内 EcsAccess 请求通道进入 Bevy 主线程。
- Entity 引用始终包含 generation；每次修改前重新验证实体有效性。
- 不建立脚本级事务，不承诺跨多个 Native API 调用自动回滚。
- 不把通用 Reflect 写入当成任意 Rust 方法调用；Widgetry 托管状态遵循其公开 API。
- 不引入 UIA、BRP、HTTP、MCP 或 OS 鼠标 / 键盘输入作为运行时执行路径。
