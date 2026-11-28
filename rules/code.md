# 代码规则

## 主动 Panic

需求或功能文档未明确规定如何处理的异常，必须在检测到时主动 panic。包括未定义的错误、违反 Invariant 的状态和按约定不可能到达的分支。不得擅自降级、返回默认值、当作 No-op、仅记日志后继续，或将异常作为可恢复错误向上传递。

使用 `panic!`、`unreachable!` 或始终启用的断言，并提供足以定位问题的上下文。不得依赖只在 Debug 构建中生效的断言。

## 错误处理与传播

### 错误语义

需求或功能文档已规定的失败按约定处理；应由上层按约定处理的错误，向上返回 `Result`，不吞掉也不以日志代替处理。

符合约定的 `Option` 缺失、Asset 等待、Observer 目标过滤、交互 Guard 和合法 No-op 属于正常行为，不触发 panic。

## Lint Suppression

只有当 Lint 在当前语义下确实不适用，或存在无法合理消除的必要例外时，才允许使用 `#[allow(...)]` 等局部 Suppression。

- 优先修正产生 Lint 的代码，而不是 Suppress Lint。
- Suppression 必须限制在必要的最小范围。
- 不得通过大范围 Suppression 掩盖实际问题。
- 根目录 `Cargo.toml` 中明确配置的 Workspace 级 Lint 例外不受局部 Suppression 限制。

不强制为每处 Suppression 添加解释性注释；仅当例外原因不明显且有必要说明时，按照 `rules/documentation.md` 记录。

## Magic Number

代码中具有明确业务或算法含义的数字字面量，应提取为具有语义的命名常量或变量。

没有独立语义的数字，例如循环索引、初始值 `0`、简单数学运算中的系数等，可以直接使用，无需提取。

命名应准确表达数字的实际含义，不要求统一使用 `const`，局部变量同样可以。

## 命名约定

- Bevy Component 类型不追加 `Component` 后缀。
- Bevy Resource 类型不追加 `Resource` 后缀。
- Bevy Plugin 类型使用 `Plugin` 后缀。
- UI Style 数据或解析后 Style 值的类型使用 `Style` 后缀。

这些约定不强加给与 Bevy、UI 无关的后台 Crate。其他命名遵循 Rust 惯例，以语义清晰为准。
