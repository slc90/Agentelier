# 绘制由同一参数定义的曲线

参数曲线通过同一参数 t 的多个坐标函数表达路径，例如 x(t)=cos(t)、y(t)=sin(t)。Scene 接受各坐标表达式、参数定义域及采样/显示语义，按同一 t 同步求值后绘制真实路径；不能将两套各自排序的数据误配。闭合曲线、回环、自交和不连续区间依实际关系显示，采样失败局部反馈。

复用现有数学求值、路径、坐标和参数状态机制；可用 Slider 改变其他参数或显示当前 t 对应点。重开时使用实际定义及已保存参数。

## 验收

绘制圆、参数螺线或 Lissajous 曲线，核对同一 t 的坐标与路径；BRP 改变参数、查看闭合和断点，重开历史后保持原关系。

## 与相邻方案的关系

与极坐标共享求值和路径，但一般参数曲线不依赖极坐标轴。下一项增加数学空间图形。

设计依据：[数学与数据可视化](../../docs/design/assistant-content/05-visualization.md)、[动态 Scene](../../docs/design/assistant-content/04-dynamic-scenes.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
