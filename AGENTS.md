# AGENTS.md - InsituTranslate 项目规则

本文档作为参与本项目的全体 AI 编码智能体（Agent）的系统规则和约束条件。在执行命令、修改文件或生成代码之前，必须仔细阅读并严格遵守这些指南。

---

## 📖 项目概述与使命
本项目是一个本地桌面端多格式文档翻译工具，基于 Tauri (v2) 和 React (TypeScript) 构建。

**核心工作流：**
该工具通过复制输入文件，然后对其进行翻译后保持原格式和样式回写来运行。它直接用目标语言文本替换副本文件内部的源语言文本，在此过程中必须严格保留原始格式、文档布局和语法结构。

### 支持的文件格式及具体行为：
- **PDF：** 在翻译前，必须先将其转换为 Markdown 格式（通过 MinerU 命令行或 API 集成），翻译完成后再进行结构重建。
- **Markdown (`.md`)：** 翻译文本，同时完整保留 Markdown 语法（标题、列表、加粗等）。
- **EPUB (`.epub`)：** 解包（Unpack），就地翻译内部的 HTML/XHTML 内容，然后重新打包（Repack）。
- **HTML (`.html` / `.htm`)：** 翻译可见的文本节点内容，同时完全保留 HTML 标签和属性。
- **TXT (`.txt`)：** 直接进行文本翻译。
- **DOCX (`.docx`)：** 翻译段落（Paragraph）和表格中的文本块（Run），同时保留 Word XML 的样式。
- **XLSX (`.xlsx`)：** 翻译单元格中的字符串，同时保留工作表（Sheets）、公式和列布局。
- **SRT (`.srt`)：** 字幕翻译。**必须**严格保留字幕索引和时间轴行（例如：`00:01:20,000 --> 00:01:23,000`）。
- **ASS (`.ass`)：** 高级字幕翻译。**必须**保留样式头信息（Style Headers）、时间块（Timing Blocks）和事件属性（Event Properties）。
- **LRC (`.lrc`)：** 歌词翻译。**必须**严格保留时间标签（例如：`[00:12.34]`）。

---

## 项目启动与命令
- **首选包管理器：** `pnpm`
- **前端技术栈：** React (Vite)、TypeScript、Tailwind CSS、shadcn/ui、motion（原 Framer Motion）
- **后端技术栈：** Tauri (v2)、Rust
- **数据库：** SQLite（通过 `tauri-plugin-sql` 或 Rust 原生驱动）
- **安装依赖：** `pnpm install`
- **启动开发服务器：** `pnpm tauri dev`
- **构建发布版本：** `pnpm tauri build`

### 动画库规范：
- **库的选择：** 使用 **`motion`**（原 `framer-motion`）。
- **安装命令：** `pnpm add motion`（**切勿**安装已废弃的 `framer-motion` 包）。
- **React 引入路径：** 务必从 **`motion/react`** 引入动画组件和 Hooks（例如：`import { motion } from "motion/react"`）。

---

## 安全约束（严格遵守）
1. **禁止前端暴露 API 密钥与直接请求：**
   - **切勿**在 React 前端中直接编写或执行针对外部翻译 API、MinerU 或其他第三方翻译服务的网络请求（如使用 `fetch` 或 `axios`）。
   - 所有需要 API 密钥的网络请求，必须封装在 **Rust 后端** 中并安全执行。
2. **防止凭证泄露：**
   - **切勿**在前端的状态（State）或 TypeScript 文件中硬编码、存储或引用敏感凭证、API 密钥或 JWT 令牌（Tokens）。
   - 前端只能通过 Tauri IPC（`invoke`）向 Rust 后端传递用户输入和本地文件路径。
3. **禁止无序兜底措施和防御性编程**
   - **切勿**在前端或后端代码中随意添加兜底逻辑（如 try-catch 捕获所有异常、返回默认值或空字符串等），以免掩盖潜在的错误和问题。所有异常必须被明确捕获、记录并上报，以便开发者及时发现和修复。
   - **切勿**在前端或后端代码中随意添加防御性编程逻辑（如对所有输入进行过度验证、过滤或默认处理），以免破坏原始数据的完整性和翻译的准确性。所有输入必须被严格验证，但不应随意修改或丢弃。

---

### 严格只读目录（参考代码库）
- **目标路径：** `/0_repo_references/`（包括其所有嵌套的文件、文件夹和子目录）。
  1. 该目录是**严格只读**的。它仅用于阅读代码实现、架构设计模式和获取项目上下文。
  2. **切勿**在 `/0_repo_references/` 内部进行写入、修改、覆写、重命名、删除或创建任何文件/目录。

---

## 架构与组件策略

### 1. 组件查找与高度复用原则（严禁重复造轮子）
- **shadcn/ui 优先：** 必须优先查找 `shadcn/ui` 已有的组件（如 `Button`, `Dialog`, `Popover` 等），**严禁随意手写自定义的基础 UI 控件**。
- **复用优先与防止代码分化：** 在创建或更改组件前，**必须优先查找项目中已经使用过、已经实现的组件进行复用**。
- **全局同步修改：** 对已有的复用组件进行更改时，**必须**考虑到该组件在其他所有页面/位置的调用。**尽量每一次修改都进行全局应用，禁止为了省事而单独复制出一个副本组件进行局部修改（例如禁止出现 `Button-v2` 等导致组件分化的行为）** 。
- **组件库一致性：** 保证所有使用到的通用组件，逐步规范化为 **「基础组件 + motion 动画包装」** 的通用组件库形式，并且这些组件必须在全局范围内表现出完全一致的外观、主题和动画轨迹，**禁止在不同页面出现同一个组件外观和动画表现不一致的问题**。

### 2. 反单体文件与模块化（严禁在 App.tsx 中堆砌代码）
- **切勿**将主要的 UI 布局、视图或复杂的业务逻辑直接塞进 `App.tsx` 中。保持 `App.tsx` 绝对整洁，仅用于高级全局布局、路由或全局状态 Provider。
- 一旦某个视图、侧边栏或设置面板功能稳定，应立即将其逻辑、UI 子结构和局部组件提取到 `src/components/` 或 `src/views/` 目录下的独立文件中。

### 3. 解耦且可复用的动画组件
- 所有 UI 组件必须具有高度解耦、自包含和可复用的特性。
- 当创建需要动画的组件（使用 `motion/react`）时，**务必将动画变体（Variants）、过渡效果（Transitions）和动画状态封装在组件文件本身内部**。
- 不要将动画配置剥离到外部，以免破坏组件的自包含性。开发者（或 Agent）应该能够将组件作为一个解耦的、带动画的「乐高积木」，直接引入到任何地方使用。

---

## UI 与设计规范

### 1. 布局密度与扁平化结构 (Layout Density & DOM Flatness)
- **紧凑高密度布局：** 采用**紧凑且高密度的布局**。尽量减少不必要的空白或过大的间距。
- **紧凑参数：** 保持 padding（如 `p-2` 或 `p-3`）、margin 和组件间距（`gap-2` 或 `gap-3`）小而紧凑，优化开发人员控制面板的视觉体验。
- **避免容器过度嵌套（严禁大套小）：** 尽量避免出现无意义的「大容器包裹小容器」的多层嵌套 div 结构。保持 DOM 树扁平、清爽。
- **防止间距累加：** 严禁在父容器和子元素上同时叠加、累加不必要的 `margin` 或 `padding`。在父级布局中，**优先使用 Flexbox 或 Grid 的 `gap` 属性**来统一控制子元素的间距，而不是让每个子元素各自手写 `margin` 导致间距失控。

### 2. 全局圆角
- 所有组件、卡片、输入框和按钮，强制使用严格的 **6px** 圆角（`rounded-[6px]`）。

### 3. 图标系统
- 使用 **`lucide-react`** 图标包（这是 shadcn/ui 的默认图标集）。如果项目缺少该图标包，请进行安装。

### 4. 字体与显示比例规范 (Typography & Scale Tokens)
- **统一使用相对单位变量：** 所有大小字体，**必须**使用 Tailwind CSS / shadcn/ui 的相对单位变量（如 `text-xs`, `text-sm`, `text-base`, `text-lg` 等）进行规范。
- **严禁硬编码绝对像素大小**（例如绝对禁止使用 `text-[13px]` 或 `text-[15px]`）。这样可以确保后期只需要在全局通过简单修改 `html { font-size: … }` 的根值，就能完美实现整个应用界面的全局比例缩放。

### 5. 全局颜色变量与双色模式适配 (Color Tokens & Themes)
- **全局颜色变量优先：** 尽量使用 shadcn/ui 默认提供的全局 CSS 颜色变量（如 `bg-background`, `text-foreground`, `border-border` 等）。
- **自定义变量双色适配：** 如果根据业务需求必须创建新的自定义颜色变量（例如自定义 toast 颜色、任务状态颜色），**必须确保在全局样式中同时配置了浅色（light）和深色（dark）两套模式下的变量适配**。

### 6. 动画与过渡性能原则 (Animation & Performance)
- **简单动画用 CSS 实现：** 对于简单的悬浮状态、淡入淡出、微小的点击交互等，**不需要使用复杂的 `motion` (Framer Motion) 库来实现**。应当直接使用 Tailwind 自带的过渡类（如 `transition-all duration-150 ease-out`）通过 CSS 性能级触发，以减轻 JavaScript 运行时的开销。
- **动画时长分级（严格遵守）：**
  - 为了保证桌面端响应的清爽敏捷，动画**必须**干净利落。**切勿**编写超过 **300ms** 的过渡动画。
  - *微交互（悬浮 Hover、点击 Active、切换开关 Toggle）：* **100ms 至 150ms**（`duration` 设为 `0.10` 至 `0.15`）。
  - *标准浮层（下拉菜单 Dropdowns、选择菜单 Select、气泡提示 Tooltips）：* **150ms 至 200ms**（`duration` 设为 `0.15` 至 `0.20`）。
  - *布局变动（折叠面板 Accordion、侧边栏折叠 Sidebar、弹窗 Dialog）：* **200ms 至 300ms**（`duration` 设为 `0.20` 至 `0.30`）。
- **缓动与弹簧效果：**
  - 避免生硬的线性过渡。优先使用平滑的 Ease-Out 减速曲线。
  - 推荐的自定义贝塞尔缓动曲线：`ease: [0.22, 0.61, 0.36, 0.99]`（平滑的减速体验）。
  - 面板展开或拖拽手势推荐的弹簧物理参数：`{ type: "spring", stiffness: 300, damping: 30 }`。
 - **交互效果: **
  - 所有按钮必须有按下悬停高亮和点击按下的反馈。按下按钮最多只允许轻微缩放，不允许有位移动画，按钮内的图标和文字也不允许有位移动画。

---

## 💡 TypeScript 指南
- 为所有的 props 和状态变量声明明确的 TypeScript 接口（Interface）和类型（Type）。
- 严格避免使用 `any`，以确保代码库在未来的模块化重构中保持可维护性。

---

## DOCX/XLSX Hybrid Parser Boundary
- DOCX/XLSX parsing must follow a read/write split: use mature crates for safe reading and validation, but keep the original ZIP/XML package as the source of truth for write-back.
- DOCX uses `docx-rs` for structured document validation/reading. Lossless render must patch only targeted `word/document.xml` text nodes and preserve runs, styles, relationships, media, and unknown XML.
- XLSX uses `calamine` for workbook validation/reading. Because `calamine` is read-only, render must not regenerate workbooks through it.
- XLSX v1 translation/write-back is intentionally limited to `xl/sharedStrings.xml`; worksheet XML files must remain untouched so formulas, cells, sheets, dimensions, and layout are preserved.
- Do not add `path` dependencies pointing at `0_repo_references`; that directory remains read-only reference material only.
