# 通过 Widgetry Scene 实现运行时控件创建

## 目标与产出

让 Agent 能在运行中的界面中创建已有控件的实例、选择父 Entity、设置语义参数，并在完成后清理。控件实例是真实的 Bevy Entity 和 Widgetry Scene 结构，不是截图、图片或模拟鼠标结果。

本增量使用前面建立的 ECS 请求通道、Entity 引用、反射查询、修改和 Event 派发能力，增加有限的 Scene Factory Registry。

## 为什么复合控件需要 Scene Factory

普通 ECS Entity 可以通过 `ecs_spawn` 创建并插入反射 Component。Widgetry 复合控件则可能由多个 Entity、内部行为 Component、绑定关系、Observer 和视觉子部件共同组成。只在一个 Entity 上插入控件根 Component，不等价于完整构造控件。

Agentelier 的 UI 构造继续遵循 BSN、Scene 和 SceneList。Agent 不能在 Rhai 中直接构造 Rust 的 `impl Scene`，也不需要一套新的通用 UI 树描述语言。因此，应用向 Agent 暴露少量已注册的 Scene Function：

- Rust 侧的模板函数负责构造 Scene，内部使用 `bsn!` 表达组件、层级、Children 和 Widgetry 组合。
- Scene Factory Registry 提供稳定名称、用途说明和参数描述。
- Rhai 只选择模板、传递语义参数和父 Entity。
- 模板通过现有 Widgetry / Agentelier 控件公开 API 建立受管理状态和部件关系。

这不是替代 BSN 的 spawn / build Helper 体系。`scene_spawn` 是 Agent 运行时选择已注册 Scene Function 的单一入口；控件构造本身仍由 Rust Scene Function 和 BSN 决定。

## Scene API

| API | 用途 |
| --- | --- |
| `scene_templates()` | 列出可用模板、说明和参数 |
| `scene_spawn(name, parent, props)` | 创建指定模板并挂载到父 Entity |
| `scene_despawn(root)` | 清理该模板实例拥有的 Scene 结构 |

`scene_spawn` 返回新 Scene 的根 EcsEntity。模板参数需要严格校验；父 Entity 必须有效，并且符合模板的挂载要求。模板构造失败时清理本次创建的 Scene 结构，并向 Rhai 返回结构化错误。

初始模板使用现有 AgentelierButton。它已经是 SceneComponent，具有 Reflect 注册和 Widgetry Button 行为。模板参数至少包括 `label` 和 `style`；Rust Scene Function 将文字内容和样式转换为 BSN Scene。后续需要时可以注册其他 Agentelier 或 Widgetry Scene Function，不要求为每种控件增加一组 Rhai 专用函数。

动态创建的 Scene 根实体通过 `scene_despawn` 清理。模板必须明确自己拥有的子实体关系，使实例清理时能够移除完整控件结构；不能误删调用方提供的业务 Entity。

## 在真实界面中组合

Agent 先通过 `ecs_query` 找到真实的容器 Entity，再查询 `scene_templates` 了解可用模板，最后调用 `scene_spawn`。创建后的控件仍然是普通 ECS 对象，可以继续通过 `ecs_components`、`ecs_get`、`ecs_reparent` 和 `ecs_trigger` 检查或操作。

当前 Workspace 中存在 Name 为 `WorkspaceF` 的空 Pane，可作为首个动态控件验收的候选容器。Agent 应在运行时通过 Name 和 Node 查询找到它，而不是预先获得 Entity ID。若当前界面状态或布局发生变化，以查询到的实际容器为准。

示意脚本：

```rhai
let hosts = ecs_query(#{
    with: ["Node"],
    limit: 128
});

let parent = find_named_entity(hosts, "WorkspaceF");
let templates = scene_templates();

let button = scene_spawn(
    "agentelier.button",
    parent,
    #{
        label: "Agent 动态控件",
        style: "secondary"
    }
);

print(ecs_components(button));
```

`find_named_entity` 表示 Agent 可以用前面已提供的 `ecs_components` / `ecs_get` 组合完成的 Rhai 辅助函数，不是必须新增的 Native API。

## 最终验收保持短小

最终验收使用当前 Function Calling，不要求提示词管理、持续任务推进、长时间任务或多轮自主规划。让 Agent 完成一个明确的小任务即可：

1. 查询当前 Workspace，发现 Name 为 `WorkspaceF` 的容器及其组件。
2. 查询可用 Scene 模板，选择 `agentelier.button`。
3. 在该容器下创建一个带指定文字和样式的按钮。
4. 读取新 Entity 的组件与父子关系，确认它是实际 Widgetry 控件。
5. 通过 BRP Screenshot 确认按钮真实显示。
6. 使用 `scene_despawn` 删除按钮，再通过 ECS Query 和 Screenshot 确认它已经清理。

该场景验证 Agent 自主发现父实体、选择模板、创建控件和清理实例的完整能力。它不通过鼠标或键盘模拟用户输入，也不把直接 ECS 操作描述成物理点击测试。

## 完成条件与运行验证

- `scene_templates` 返回已注册 Scene Function 的名称、说明和参数。
- `scene_spawn` 能调用 Rust 侧 Scene Function，并通过 BSN 创建完整 Widgetry 控件。
- 初始 `agentelier.button` 模板支持文字和样式参数，创建后返回真实根 Entity。
- 新控件正确挂载到指定父 Entity，Widgetry 内部部件和受管理状态符合公开契约。
- Agent 能通过 ECS 查询发现 WorkspaceF 或当前运行时等价的真实容器，不预先获得 Entity ID。
- Agent 能在一次短小的 `ecs_execute` 调用中创建按钮、检查结构并删除；BRP Screenshot 与 ECS State 能确认显示和清理结果。
- Scene 模板参数错误、父 Entity 失效和 Scene 构造失败返回结构化错误，并清理本次创建的结构。
- 在 Windows x64 实际运行 Agentelier 验证；不编写测试代码、Benchmark 或 Example。

## 保持的边界

- Agent 只能实例化已注册的 Scene Function，不能运行时定义新的 Rust Component、Observer、System 或 Scene 类型。
- 不建立通用 UI Tree DSL，不以 Rhai 描述替代 BSN 的 Scene 构造。
- AgentelierButton 和其他 Widgetry 控件遵循公开 API 与托管状态契约。
- 新控件通过 ECS / Scene 直接创建，不使用 Windows UIA、BRP、HTTP、MCP 或 OS 鼠标 / 键盘输入作为执行路径。
- BRP Screenshot 只用于开发验证；它不是 Agent 执行控件创建的依赖。
