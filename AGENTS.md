# InsituTranslate 协作规则

## 项目

Tauri 2 桌面翻译工具。前端：React、Vite、TypeScript、Tailwind CSS、shadcn/ui、`motion`；后端：Rust、SQLite；包管理器：pnpm。

工作流：复制源文件，只替换可翻译内容，保留格式、结构、样式、布局、元数据和非文本资源。

- PDF：通过 MinerU CLI 或 API 转为 Markdown，翻译后重建。
- Markdown：翻译文本，保留标题、列表、强调、链接、代码等语法。
- HTML：只翻译可见文本节点，保留标签和属性。
- EPUB：解包，翻译内部 HTML/XHTML，再重新打包。
- JSON：只翻译字符串值，保留键和数据结构。
- TXT：直接翻译文本。
- SRT：保留字幕索引和时间轴；ASS：保留样式头、时间块和事件属性；LRC：保留时间标签。
- DOCX、XLSX：原 ZIP/XML 是回写依据；成熟 crate 只负责读取、校验。
- DOCX：用 `docx-rs` 读取；只修改目标 `word/document.xml` 文本节点，保留 run、样式、关系、媒体和未知 XML。
- XLSX：用 `calamine` 读取；v1 只修改 `xl/sharedStrings.xml`，不重建工作簿，不改 worksheet XML。

## 实施原则

- 先读需求、相关代码和调用点；复用现有模式；只改完成当前需求所需的部分。
- KISS 指最小完整实现，不是草率实现。处理真实成功路径、错误路径和必要调用点；不为假设需求增加抽象、配置、状态、兼容层、缓存、重试或回退。
- 具体反馈默认只约束所述问题。除非 Floratina 明确要求普遍规则，不追加全局排除逻辑、额外状态、说明文案或仅用于证明某功能不存在的测试。
- 修复原因，不围绕单个现象建立长期禁令。新增复杂度必须对应当前需求、既有约定或可复现问题。
- 按职责拆分；`App.tsx` 只保留应用壳、导航和 Provider。不要堆积业务，也不要按文件行数机械拆分。
- 修改共享组件前检查全部调用点。全局意图才全局修改；局部差异用现有 props 或组合，不复制 `v2` 组件。
- TypeScript 明确类型，避免 `any`；Rust 保持现有模块和错误类型边界。

## 错误、数据与安全

- 禁止 catch-all、吞错、空字符串、默认成功或静默降级。错误应在对应边界明确捕获，并携带上下文返回、记录或展示。
- 只在输入、IPC、文件、数据库、外部协议等信任边界按契约校验。内部不重复校验；除非契约要求，不 trim、过滤、纠正、替换或丢弃原始数据。
- 回退、重试、兼容分支仅用于明确需求、协议规定或已复现故障；行为必须可见、可定位、可测试。
- API 密钥、JWT 等凭证不得硬编码或存入前端代码、状态。前端不得直连翻译服务、MinerU 或需凭证的第三方 API；通过 Tauri IPC 调用 Rust。
- `D:\AppData\0_repo_references\` 及其子目录严格只读；不得创建、修改、移动或删除，也不得作为 path dependency。

## 前端

- 优先复用 `src/components/ui/`、现有 feature 组件和 shadcn/ui；图标用 `lucide-react`。
- 沿用现有颜色、字号、圆角、间距 token；不要为局部需求另造近似 token。新增颜色 token 必须同时定义浅色和深色。字体用相对单位工具类。
- 紧凑布局；减少无意义容器；父布局优先用 flex/grid `gap`，避免间距叠加。
- 动画使用仓库已有的 `motion`（由 `framer-motion` 更名），React API 从 `motion/react` 导入；不要重复添加动画依赖。
- 简单 hover、active、淡入淡出用 CSS；结构动画、重排复用 `src/lib/motion.ts`。交互须有反馈，不位移按钮文字或图标。
- 通用滚动区复用 `ScrollArea`；横向或双轴滚动使用 `axis="horizontal"` 或 `axis="both"`。原生输入内部滚动用 `scrollbar-native`。
- 第三方控件可保留其滚动宿主，但须复用共享滚动条 token。不要用全局 `preventDefault()` 或手动距离模拟滚轮；浮层使用实际 viewport 和 `overscroll-contain`，避免滚动穿透。

## 测试

- 常规改动只运行最相关的检查；行为改变或真实回归风险才新增测试。优先表驱动用例、共享 fixture；不重复覆盖同一行为，不测试实现细节或未提出的排除项。
- Floratina 明确要求“重写/重构测试体系”前，不迁移或重排现有测试。
- 仅在该要求出现后：建立分层 Rust 测试矩阵。根层评估整套软件的关键工作流与子系统组合；各子目录用集中 `tests.rs` 评估本子系统的能力和可用状态；共享 fixture，合并散落、重复用例。

## 命令

```text
pnpm install
pnpm tauri dev
pnpm build
pnpm tauri build
cargo test --manifest-path src-tauri/Cargo.toml
```
