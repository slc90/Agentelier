# 已知问题

## BRP 特殊按键缺少逻辑键值

当前锁定的 `bevy_brp_extras` 0.4.2（Git revision `25e014bb`）在键盘事件生成中，只为可映射字符填写 `logical_key`；Control、Backspace 等特殊键得到 `Key::Unidentified`。Bevy 0.20.1 文本输入使用逻辑键识别编辑与快捷键，因此标准 `send_keys` 的 Ctrl+A、Backspace 等操作不能等价于完整键盘输入。已在真实聊天输入中观察到 Ctrl+A 仅插入字符 a、Backspace 未删除，并核对上游 `keyboard/events.rs` 与 Bevy `text_input.rs` 确认原因。

验证此类输入时可使用标准 BRP `world.write_message` 提交完整的 `bevy_input::keyboard::KeyboardInput`，先通过 Pointer 建立焦点，再发送有正确物理键、逻辑键及窗口的 Press/Release。消息仍经过应用的输入、焦点和文本编辑系统，不修改编辑器组件或业务状态；该证据属于 BRP 键盘输入，不能视为原生 OS 或 IME 验证。当前不引入修改版上游源码。

## 多个 BRP 实例争用固定 Render 端口

当前锁定的 `bevy_brp_runtime` 0.4.2（Git revision `25e014bb`）只允许通过 `with_port` 或 `BRP_EXTRAS_PORT` 改变 Main World 端口；Render World HTTP listener 固定使用 Bevy 默认的 `15703`。即使两个实例使用不同 Main 端口，同时启用 BRP 时，后启动者仍会因 Render listener 绑定失败记录错误并退出。

已通过真实双实例运行确认，日志包含 `endpoint="Render"` 和 Windows `os error 10048`，并核对上游 `runtime/src/lib.rs` 的公开配置与 `runtime/src/http.rs` 的 Render listener 装配。该限制影响多实例调试，与目录文件锁独立；目录占用和释放已在仅一个实例启用 BRP 的配置下验证。

多实例运行时让其中一个实例在 `config/config.ron` 中设置 `brp_enabled: false`，即可保留普通应用行为。当前不升级既定 Bevy / Widgetry / BRP 组合，也不修改或维护上游源码；公开 API 暂未提供 Render 端口配置，因此保留此限制。

## 切页后部分静态文字偶发未绘制

在 Bevy 0.20.1 / Widgetry 430ddb44 的真实窗口中，切换设置页再返回后，曾观察到部分账号静态文字未绘制；对应 Text、颜色、布局尺寸仍存在，诊断时改变 Text 可恢复绘制。账号凭据与聊天会话不受影响。原因尚未确认，可能涉及新建文本的布局或绘制时序，不能确定属于上游缺陷；目前不修改上游源码，也不通过逐帧重置文字掩盖现象。
