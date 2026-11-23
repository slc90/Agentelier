# 建立 Project / Session 导航并联动聊天区域

## 目标

交付 C 区的 Project 与单独聊天导航，并让 D 区只在 Agent 确认打开目标 Session 后切换内容。

## 导航结构

C 区包含两个分组：

- **项目：**可展开的 Project 节点，下面显示所属 Session。
- **单独聊天：**不属于任何 Project 的 Session 列表。

Project 可展开和收起。C 提供创建 Project、创建单独聊天、在 Project 中创建 Session、重命名、删除、移动 Session 和选择 Session 的入口。创建 Project 必须使用 FileDialog 选择已有目录；名称默认取文件夹名，可自定义。

列表项使用稳定 Project ID / Session ID，不使用显示名称、行 Entity 或列表索引作为业务身份。导航数据来自 Agent 的安全目录投影，不直接扫描 Session 文件或 Project 元数据。

## C 与 D 的切换契约

用户选择 Session 后，UI 向 Agent 提交打开请求。只有 Agent 返回目标 Session 已成功打开的回执后，D 才替换为目标 Session 的历史、草稿、模型设置和阅读位置。打开失败时保留当前 D 内容并显示错误。

切换 C 不停止其他 Session 的后台任务；E 继续展示全部 Session。当前 Session 有未提交存储操作或不可切换的执行状态时，由 Agent 业务规则裁决；UI 不隐式取消任务或先清空 D 区。

## Widgetry 复用

Tree 负责 Project / Session 层级、展开与选择；ListView 可用于单独聊天列表；FileDialog 选择 Project 目录；TextField / MessageBox 用于命名和确认；ScrollArea 负责列表滚动。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
- 未经用户明确授权，不执行登出、重新授权、切换账号或清理现有凭据的验收操作。
## 完成条件与验收

1. C 区显示 Project 分组和单独聊天分组。
2. Project 节点可展开、收起；展开状态变化不创建或删除 Session。
3. Project 下 Session 与 Agent 持久化归属一致，移动后旧分组不残留重复项。
4. 单独聊天只显示没有 Project 归属的 Session。
5. 创建 Project 必须选择已有目录；取消选择不创建空 Project。
6. 在 Project 中创建 Session 后，新 Session 出现在对应 Project 分组；创建单独聊天后出现在单独聊天分组。
7. 选择 Session A 后 D 显示 A 的历史；选择 Session B 后 D 显示 B 的历史，消息和工具结果不串接。
8. 切换后恢复目标 Session 自己的草稿、模型设置和阅读位置。
9. 打开 Session 失败或存档损坏时，D 保留原内容并显示错误。
10. 当前 Session 有未提交记录或不可切换的执行状态时，UI 按 Agent 回执显示限制，不自行取消任务。
11. 切换 Session 不停止其他 Session 后台任务；E 的全局任务范围不随 C 过滤。
12. Project 删除后所属 Session 自动转为单独聊天；历史、附件和工作目录不变。
13. Project / Session 重命名只更新对应名称，不改变稳定 ID、目录和历史。
14. 列表虚拟化与滚动复用不会把旧行点击事件绑定到新 Session。
15. 使用 BRP 真实点击、展开、折叠、选择和目录选择，结合 Agent / Bridge 状态检查确认联动。
16. 验收后释放 BRP Pointer 并确认状态为 inactive。
