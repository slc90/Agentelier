# 登录数据环境

`agentelier_login` 是不依赖 Bevy 的功能库。当前公开 `DataEnvironment`，负责用户确认目录的检查和进程级占用；账号认证尚未实现。

- `DataEnvironment::acquire(&Path)` 要求目录已存在且为绝对路径，检查目录读取、已有 `auth.json` 读写和临时文件创建、替换、读回；返回真实 I/O 失败，不创建缺失目录或账号文件。
- `root()` 与 `credentials_path()` 分别返回规范化的固定根路径和该目录下的 `auth.json` 路径。
- `validate_data_directory(&Path)` 复用目录和凭据访问检查，返回规范绝对路径，不创建或持有候选目录锁，不初始化账号。
- `open_in_explorer()` 检查当前目录仍可访问，以真实路径调用 Windows Shell 打开目录；失败返回 I/O 错误，不改变当前环境。
- 实例持有 `.agentelier.lock` 上的操作系统独占文件锁；克隆共享占用，最后一个持有者释放后锁才解除。锁文件留在原目录，其存在本身不代表占用。

调用者应先让用户明确确认目录，采用后保持数据环境到后台任务和日志收尾结束。应用配置、日志和界面由宿主管理。完整启动关系见[数据目录与启动](../design/mechanisms/application-data.md)。

## 文件账号恢复

`AuthFile::new(PathBuf)` 绑定具体凭据文件，`restore(&mut self)` 同步读取并校验记录，成功后持有该记录，仅返回 `RestoredAccount` 的安全摘要及 `CredentialState`。应用通过 Agent/Tokio 调用，不在 ECS 主线程读取。该组件当前没有写入接口。

缺失文件返回 `NoRecord`；只有主机标识返回 `HostOnly`；登记存在、会话为空返回 `SignedOut`；已有会话返回 `StoredSession { expired }` 及账号摘要。过期仅按当前时间与记录有效期比较，不代表已完成网络认证或续期。

JSON 记录格式及字段关系见[认证设计](../design/modules/authentication.md)。未知字段、字段类型错误、无效主机标识、不一致登记/会话及时间字段均失败；文件最多读取 1 MiB，超限返回错误。读取、解析、结构与时钟错误由 `RestoreFailure` 区分，解析错误只提供行列，不携带正文。失败保留原文件及上次成功缓存；不搜索其他应用数据，不生成标识，不打开浏览器。

私有记录及 Token 类型不实现 `Debug` 或反射。返回摘要只含 subject、可选展示名和邮箱；任何文件操作继续使用原始路径。
