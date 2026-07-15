# InsituTranslate


> <mark style="background:rgba(5, 117, 197, 0.2)">此项目仍然在开发中，很多功能尚不完善。</mark>

InsituTranslate 是一款的本地桌面文档翻译工具，基于 Tauri 2、React、TypeScript 和 Rust 构建。

项目的核心目标是：在翻译文档内容的同时，尽可能保留原文件的格式、结构、样式和非文本数据，并将翻译结果重新写回对应格式。

> 当前版本：0.1.0  
> 项目状态：开发中  
> 当前主要开发平台：Windows

## 功能简介

### 多格式文档翻译

目前支持导入以下格式：

| 格式 | 当前处理方式 |
| --- | --- |
| PDF | 使用 `pdf_oxide` 本地解析或 MinerU 解析为 Markdown；当前导出格式为 Markdown |
| Markdown | 翻译文本并保留标题、列表、链接、强调等 Markdown 结构 |
| EPUB | 解析内部 HTML/XHTML 页面，翻译后重新打包 |
| HTML / HTM | 翻译文档文本内容并保留标签及页面结构 |
| TXT | 直接进行纯文本翻译 |
| DOCX | 修改目标 Word XML 文本节点，尽量保留样式、关系、媒体和文档结构 |
| XLSX | 当前主要翻译 `sharedStrings.xml` 中的共享字符串，不修改工作表公式与布局 |
| JSON | 翻译 JSON 中的文本内容并保留数据结构 |
| SRT | 保留字幕序号和时间轴，只翻译字幕文本 |
| ASS | 保留样式头、时间信息和事件属性 |
| LRC | 保留歌词时间标签，只翻译歌词内容 |

支持一次批量添加多个文件，每个文件会创建为独立翻译任务。

### 翻译提供商

后端目前支持以下协议：

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages
- Gemini API
- Google Vertex AI
- Ollama Chat
- OpenAI 兼容接口及自定义 Base URL
- MinerU PDF 解析服务

API Key、服务账号和自定义请求头由 Rust 后端处理，不应写入前端源码或提交到 Git 仓库。

### 翻译任务管理

- 创建和批量管理翻译任务
- 任务开始、暂停、继续和重新翻译
- 实时查看解析、分块、翻译和恢复进度
- 配置最大并发数、重试次数和失败比例阈值
- 动态请求速率与 Token 速率限制
- 按名称、标签、语言或模型检索任务
- 编辑任务名称和标签
- 导入、导出 `.inp` 任务文件
- 将完成的任务导出为对应文档格式

### 翻译配置

- 源语言自动检测或手动选择
- 目标语言选择
- 自定义单块 Token 数
- 无上下文、串行滑动窗口、并行滑动窗口和全局背景等上下文模式
- 模型推理强度配置
- 模型联网搜索配置
- 自定义模型请求参数
- 规则校对与翻译置信度检测配置

### 术语表

- 导入 CSV 或 JSON 术语表
- 管理、搜索、排序和分页查看术语
- 新增、编辑和删除术语
- 导出 CSV 或 JSON
- 将已有术语表应用到翻译任务
- 使用模型自动建立任务术语表
- 为自动术语表配置独立的模型、并发数、重试次数和失败阈值

CSV 术语表必须只包含以下两列：

```csv
src,dst
source term,target term
````

JSON 术语表使用类似结构：

```
[
  {
    "src": "source term",
    "dst": "target term"
  }
]
```

### 其他功能

- 自定义翻译助手和系统提示词
- 自定义助手请求参数
- 浅色、深色和跟随系统模式
- 内置主题及自定义主题色
- 系统字体选择
- 后端日志记录和 PowerShell 实时日志控制台

## 当前开发状态

InsituTranslate 仍处于开发阶段，部分功能尚未完成：

- PDF 可以完成文本提取和翻译，但暂未实现最终 PDF 原格式重建，当前导出为 Markdown。
- 校对页面目前只按任务分块顺序显示原文和译文，尚未接入可编辑校对器及格式化预览。
- 安装包生成目前没有启用，Tauri 配置中的 `bundle.active` 为 `false`。
- 不同文档软件和文件生成器产生的 DOCX、XLSX、EPUB 文件可能存在兼容性差异。
- XLSX 当前仅处理共享字符串，使用内联字符串或其他存储方式的文本可能不会被翻译。
- 项目接口和 `.inp` 任务格式仍可能在后续开发中调整。

不建议当前版本直接用于不可恢复的重要文件。请始终保留原文件备份。

## 技术栈

### 前端

- React
- TypeScript
- Vite
- Tailwind CSS
- shadcn/ui 风格组件
- Radix UI
- Motion
- Lucide React

### 后端

- Tauri 2
- Rust
- Tokio
- SQLite / SQLx
- Reqwest
- docx-rs
- calamine
- pdf_oxide
- lib-epub
- quick-xml

## 开发环境要求

当前仓库的启动配置以 Windows 为主。

建议安装：

- Git
- Node.js 24 或兼容版本
- pnpm 10.12.1
- Rust stable 工具链
- Microsoft Visual Studio 2022 Build Tools
- Windows SDK
- Microsoft Edge WebView2 Runtime
- PowerShell 7

安装 Visual Studio Build Tools 时，请选择：

- Desktop development with C++
- MSVC C++ Build Tools
- Windows 10 或 Windows 11 SDK

Rust 推荐通过 rustup 安装：

```
winget install Rustlang.Rustup
```

安装完成后重新打开 PowerShell，并检查：

```
rustc --version
cargo --version
```

安装 pnpm：

```
corepack enable
corepack prepare pnpm@10.12.1 --activate
```

如果当前 Node.js 环境不包含 Corepack，也可以使用：

```
npm install --global pnpm@10.12.1
```

## 克隆和启动

### 1. 克隆仓库

```
git clone https://github.com/Floratina/InsituTranslate.git
Set-Location InsituTranslate
```

### 2. 检查开发环境

```
node --version
pnpm --version
rustc --version
cargo --version
```

### 3. 安装前端依赖

推荐根据锁文件安装：

```
pnpm install --frozen-lockfile
```

如果正在主动更新依赖，可以使用：

```
pnpm install
```

### 4. 启动桌面开发环境

```
pnpm tauri dev
```

首次启动时，Cargo 需要下载和编译 Rust 依赖，因此耗时会明显长于后续启动。

只启动 Vite 前端可以使用：

```
pnpm dev
```

但浏览器环境无法完整使用文件选择、数据库、翻译任务和其他 Tauri 后端功能。完整开发应使用 `pnpm tauri dev`。

## 首次启动后的配置

### 1. 添加翻译提供商

进入「提供商」页面：

1. 添加提供商。
2. 选择对应协议。
3. 填写 Base URL。
4. 在应用内设置 API Key。
5. 获取远程模型或手动添加模型。
6. 使用连接测试确认模型可用。
7. 启用需要使用的提供商和模型。

对于 OpenAI 兼容服务，通常选择 OpenAI Chat Completions 或 OpenAI Responses，并填写服务商提供的 Base URL。

如果使用本地 Ollama，需要先启动 Ollama 服务并下载对应模型。

### 2. 配置 PDF 解析

普通文本型 PDF 可以使用本地 `pdf_oxide` 解析，不需要 MinerU。

扫描件或本地解析效果不理想的 PDF，可以在「提供商」页面配置 MinerU，然后选择：

- 优先本地解析
- 优先 MinerU
- 仅本地解析
- 仅 MinerU

没有配置 MinerU 时，请避免选择「仅 MinerU」。

### 3. 创建翻译助手

进入「助手」页面，可以配置：

- 助手名称和图标
- 系统提示词
- 自定义模型请求参数
- 助手启用状态

### 4. 创建翻译任务

进入「开始」页面：

1. 选择源语言和目标语言。
2. 选择翻译提供商、模型和助手。
3. 根据需要选择术语表。
4. 配置 Token 分块、并发数、重试次数和上下文模式。
5. 拖入或选择文档。
6. 创建任务。
7. 前往「任务」页面开始翻译并查看进度。

## 构建与检查

### 前端类型检查和构建

```
pnpm build
```

### Rust 测试

```
cargo test --manifest-path src-tauri/Cargo.toml
```

### 构建 Tauri Release

```
pnpm tauri build
```

当前 `src-tauri/tauri.conf.json` 中：

```
{
  "bundle": {
    "active": false
  }
}
```

因此当前配置主要生成 Release 程序，不生成 MSI、NSIS 等安装包。Release 构建结果位于：

```
src-tauri/target/release/
```

如需发布安装包，需要先补充签名、图标、安装器和更新策略，并启用 Tauri Bundle。

## 项目结构

```
InsituTranslate/
├─ public/                       静态资源和提供商图标
├─ src/
│  ├─ components/
│  │  ├─ layout/                 应用框架和标题栏
│  │  └─ ui/                     通用 UI 组件
│  ├─ features/                  按业务领域划分的前端模块
│  ├─ lib/                       通用前端工具
│  ├─ views/                     应用页面
│  ├─ App.tsx                    页面调度和全局布局入口
│  └─ main.tsx                   React 入口
├─ src-tauri/
│  ├─ capabilities/              Tauri 权限配置
│  ├─ icons/                     桌面端图标
│  ├─ src/
│  │  ├─ document_parsing/       文档解析与格式恢复
│  │  ├─ translation/            翻译任务与调度管线
│  │  ├─ adapters.rs             模型提供商协议适配
│  │  ├─ commands.rs             Tauri IPC 命令
│  │  ├─ db.rs                   提供商数据库
│  │  ├─ glossaries.rs           术语表管理
│  │  └─ settings.rs             应用设置
│  ├─ Cargo.toml
│  └─ tauri.conf.json
├─ package.json
├─ pnpm-lock.yaml
└─ vite.config.ts
```

## 本地数据

运行后，Tauri 会在应用数据目录中保存本地数据。Windows 下通常位于：

```
%APPDATA%\com.insitutranslate.desktop\
```

主要内容包括：

```
providers.sqlite3
settings.sqlite3
translation-workspace/
glossary-workspace/
logs/backend.log
```

这些文件属于本地运行数据，不应提交到 Git。

删除应用数据目录会导致本地提供商配置、任务索引、术语表和界面设置丢失。进行数据库或任务格式开发前，请先备份该目录。

## 开发注意事项

- 使用 `pnpm`，不要混用 npm、Yarn 或其他锁文件。
- 动画组件从 `motion/react` 导入，不要安装旧的 `framer-motion` 包。
- 外部翻译 API 请求必须由 Rust 后端执行。
- 不要在 React、TypeScript 或前端环境变量中存放 API Key。
- 修改文档格式解析器时，必须验证翻译后文件仍可被对应软件正常打开。
- DOCX 和 XLSX 写回时，应保留原 ZIP/XML 包并只修改目标文本节点。
- PDF、DOCX、XLSX、EPUB 和字幕格式的修改应同时增加对应测试。
- `/0_repo_references/` 如果存在，只允许作为参考代码读取，禁止修改或添加路径依赖。

## 非 Windows 平台

当前 Tauri 配置使用了 Windows 命令：

```
"beforeDevCommand": "pnpm.cmd dev",
"beforeBuildCommand": "pnpm.cmd build"
```

因此 macOS 和 Linux 不能直接使用当前配置启动。跨平台支持需要调整这些命令，并分别配置对应平台的 Tauri 系统依赖、权限、窗口行为和打包流程。

项目中的移动端图标不代表 Android 或 iOS 版本已经完成。

## 授权说明

仓库目前未包含 `LICENSE` 文件。

在项目作者明确添加开源许可证之前，请不要默认该仓库允许复制、修改、重新发布或商业使用。如需参考或使用代码，请先取得项目作者许可。
