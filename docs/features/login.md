# 登录数据环境

`agentelier_login` 是不依赖 Bevy 的功能库。当前公开 `DataEnvironment`，负责用户确认目录的检查和进程级占用；账号认证尚未实现。

- `DataEnvironment::acquire(&Path)` 要求目录已存在且为绝对路径，检查目录读取、已有 `auth.json` 读写和临时文件创建、替换、读回；返回真实 I/O 失败，不创建缺失目录或账号文件。
- `root()` 与 `credentials_path()` 分别返回规范化的固定根路径和该目录下的 `auth.json` 路径。
- `validate_data_directory(&Path)` 复用目录和凭据访问检查，返回规范绝对路径，不创建或持有候选目录锁，不初始化账号。
- `open_in_explorer()` 检查当前目录仍可访问，以真实路径调用 Windows Shell 打开目录；失败返回 I/O 错误，不改变当前环境。
- `display_path(&Path)` 仅生成可读路径文本：扩展盘符路径显示为普通盘符路径，扩展 UNC 路径显示为 `\\server\share`。不改变存储路径或文件操作语义。
- 实例持有 `.agentelier.lock` 上的操作系统独占文件锁；克隆共享占用，最后一个持有者释放后锁才解除。锁文件留在原目录，其存在本身不代表占用。

调用者应先让用户明确确认目录，采用后保持数据环境到后台任务和日志收尾结束。应用配置、日志和界面由宿主管理。完整启动关系见[数据目录与启动](../design/mechanisms/application-data.md)。
