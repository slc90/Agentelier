# 已知问题

## 多个 BRP 实例争用固定 Render 端口

当前锁定的 `bevy_brp_runtime` 0.4.2（Git revision `25e014bb`）只允许通过 `with_port` 或 `BRP_EXTRAS_PORT` 改变 Main World 端口；Render World HTTP listener 固定使用 Bevy 默认的 `15703`。即使两个实例使用不同 Main 端口，同时启用 BRP 时，后启动者仍会因 Render listener 绑定失败记录错误并退出。

已通过真实双实例运行确认，日志包含 `endpoint="Render"` 和 Windows `os error 10048`，并核对上游 `runtime/src/lib.rs` 的公开配置与 `runtime/src/http.rs` 的 Render listener 装配。该限制影响多实例调试，与目录文件锁独立；目录占用和释放已在仅一个实例启用 BRP 的配置下验证。

多实例运行时让其中一个实例在 `config/config.ron` 中设置 `brp_enabled: false`，即可保留普通应用行为。当前不升级既定 Bevy / Widgetry / BRP 组合，也不修改或维护上游源码；公开 API 暂未提供 Render 端口配置，因此保留此限制。
