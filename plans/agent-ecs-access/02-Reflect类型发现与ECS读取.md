# 建立 Reflect 类型发现与 ECS 读取 API

## 目标与产出

在已有 `ecs_execute` 和 EcsAccess 主线程请求通道上，开放通用的运行时类型探索与 ECS 只读查询。完成后，Agent 可以自行发现 Entity、Component、Resource 和父子关系，不需要预先获得 Entity ID，也不需要由程序把 UI 整理成一棵树交给它。

本增量只开放读取和类型信息，不修改 ECS 数据。Entity、Component、Resource 的写入由下一增量负责。

## 从运行时类型注册信息开始

Bevy Reflect 提供运行时类型结构。Agent 通过以下 API 发现它能够理解的类型：

| API | 用途 |
| --- | --- |
| `reflect_types()` | 列出当前 TypeRegistry 中已注册的 Reflect 类型 |
| `reflect_type(name)` | 获取指定类型的 kind、字段、字段类型和注册能力 |
| `ecs_world_info()` | 读取 World 基础统计和可用类型概况 |

应用启用了 `reflect_auto_register`，也有控件和模块通过 `register_type` 显式注册类型。API 只返回运行时真正存在的注册信息，不把 Rust 源码中的所有类型假定为可反射类型。

Component 的存在性和字段读取是不同能力。`ecs_components(entity)` 可以列出实体当前拥有的 Component 类型；`ecs_get(entity, type)` 读取字段时则要求对应 `ReflectComponent` 已注册。Resource 使用相同原则，读取需要 `ReflectResource` 能力。

## 通过 Component 条件查询 Entity

读取 API 使用完整 Entity 身份封装成 `EcsEntity`，包含 generation。每次读取都会重新检查 Entity 是否仍然存在，销毁后复用的 index 不会指向新实体。

| API | 用途 |
| --- | --- |
| `ecs_query(filter)` | 根据 Component 条件查找 Entity |
| `ecs_exists(entity)` | 检查 Entity 是否有效 |
| `ecs_components(entity)` | 获取 Entity 当前 Component 类型列表 |
| `ecs_get(entity, type)` | 读取指定 Component 的反射数据 |
| `ecs_parent(entity)` | 获取父实体 |
| `ecs_children(entity)` | 获取子实体 |

`ecs_query` 的过滤条件至少支持 `with` 和 `without`，并提供 `limit` 与 `cursor`。结果有明确上限，分页游标用于继续读取，不一次性将整个 World 返回模型。

例如，Agent 可以先查询具有 Node 的实体，再读取它们的 Name 和 Children，逐步确定某个页面区域：

```rhai
let entities = ecs_query(#{
    with: ["Node"],
    limit: 128
});

for entity in entities {
    let components = ecs_components(entity);
    if components.contains("Name") {
        print(ecs_get(entity, "Name"));
    }
}
```

这只是 ECS 查询，不代表系统存在一棵完整 UI Tree。Agent 根据 Component 和关系逐步探索；`ChildOf` / `Children` 只是其中一种可查询关系。

## Resource 读取

| API | 用途 |
| --- | --- |
| `ecs_resources()` | 列出可查询的 Resource 类型 |
| `ecs_resource_get(type)` | 读取支持 `ReflectResource` 的 Resource |

Resource 数据使用与 Component 相同的受限值表示。Resource 没有反射注册时，返回明确的 `reflect_not_registered` 错误，不尝试通过字符串或 Debug 输出伪造结构化数据。

## Rhai 值与跨线程结果

EcsAccess 请求不传递 Rhai Dynamic 或 Bevy 反射对象。主线程将结果转换成受限的 Rhai 可消费数据结构，再通过响应通道返回。支持的值包括基本类型、字符串、数组和字段映射；不能转换的反射类型返回 `unsupported_type`，并保留类型名称和失败字段。

Component / Resource 可能包含很大的集合或嵌套结构。读取 API 必须遵守脚本输出和集合限制；对查询结果使用 `limit` / `cursor`，对字段数据提供明确的深度或大小边界。读取失败不能导致整个 ScriptSession 丢失。

## 完成条件与运行验证

- `reflect_types()` 返回真实 TypeRegistry 中的类型；`reflect_type(name)` 能返回已注册类型的结构信息。
- `ecs_query` 能按 `with` / `without` 条件查询真实 Entity，并支持有界结果和分页。
- EcsEntity 保存完整 Entity 身份；Entity 销毁后旧引用返回 `entity_not_found`。
- `ecs_components` 能列出实体组件；`ecs_get` 只对具备 ReflectComponent 的类型返回字段数据。
- `ecs_resources` / `ecs_resource_get` 能读取已注册 ReflectResource 的 Resource。
- Agent 通过一次普通 Function Calling 自主查找当前 Workspace 的一个真实 UI 实体，读取它的组件和父子关系；不预先提供 Entity ID。
- 对未注册反射能力的类型返回明确错误，不崩溃、不返回误导性的空数据。
- 使用实际 Agentelier 运行和 Function Calling 验证；GUI 可观察变化使用 BRP Screenshot / ECS State 检查。不得编写测试代码、Benchmark 或 Example。

## 保持的边界

- 本 API 不把 ECS 关系包装成通用 UI Tree；查询以 Entity、Component、Resource 和真实 Relationship 为准。
- 不要求所有 Rust 类型实现 Reflect；只有注册了对应能力的类型才能读取字段。
- 不开放任意 Rust 方法调用，也不将反射类型信息解释为任意行为权限。
- 请求仍通过进程内 EcsAccess 通道在 Bevy 主线程执行；不引入 BRP、HTTP、MCP 或 UIA。
- 本增量只读，不提前加入 Component、Resource、Entity 或 Hierarchy 修改。
