# 扩展专业数学记号、字体与宏环境

普通公式已能排版，此增量针对专业数学的记号覆盖。利用选定引擎支持的字形、数学字体、宏和环境配置，补齐逻辑与集合、线性代数、分析、概率、抽象代数、拓扑/几何和数理物理的常用表达；不为每个学科创建独立渲染器。

| 范围 | 实际检查内容 |
| --- | --- |
| 逻辑与集合 | `\forall`、`\exists`、`\models`、`\mathbb{N}`、蕴含与集合关系 |
| 线性代数 | 向量、矩阵、行列式、分块、转置和省略号 |
| 分析及统计 | 多重上下标、积分/梯度、条件概率、期望与指示函数、多行对齐 |
| 抽象代数与拓扑 | `\operatorname{Hom}`、花体、商结构、箭头、映射与微分形式 |
| 数理物理 | 张量、多重指标、算符、Dirac bra-ket、常用单位与物理记号 |

MathJax 的 `ams`、`mathtools`、`braket`、`physics`、`unicode`、`units`、`amscd` 和 `bussproofs` 提供覆盖参照。候选原生引擎实际支持多少宏需要依据运行结果确认，使用可扩展记号配置，不假定整个 TeX 生态可用。复杂交换图和证明树交给后续语义图形组件；完整 LaTeX 文档、TikZ、化学排版不自动纳入。未知宏、缺失字形及环境保留源文和明确局部反馈。

## 验收

在真实应用按上表输出实际公式及 `\begin{pmatrix}`、`\langle\psi|\hat{H}|\psi\rangle` 等，检查符号、字体、间距和对齐；核对 Unicode 和宏支持及不支持时的原文回退，保持代码与复制原文。不能把引擎 README 的覆盖宣称当作验收证据；仍缺失的专业记号注明真实原因。

## 与相邻方案的关系

依赖独立与行内公式的排版路径；与动态 Scene 发布仅顺序相邻，图形模板后续可复用公式显示能力。

设计依据：[数学排版](../../docs/design/assistant-content/03-math-typesetting.md)。

## 工程约束

遵循 Windows x64 / DX12、PowerShell 7 与锁定 Workspace 依赖。UI 使用 BSN、Scene / SceneList、Widgetry 公开 API；Props 仅初始化，运行期状态由 Component 与相应业务所有者维护。Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不依赖 UI；现有进程内通信与权限边界不变。按实际影响进行 Cargo 检查与真实应用验证，GUI 变化用 BRP 真实输入、截图和 ECS 状态确认，结束后释放 Pointer 并核对 inactive。禁止新增测试、Benchmark、Example 代码及仅用于这些目的的依赖；未经授权不改变登录、凭据或数据目录。实施中同步受影响的当前设计文档，不提前宣称其他增量已实现。
