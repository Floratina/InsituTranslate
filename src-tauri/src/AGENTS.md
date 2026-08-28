# Rust 后端协作规则

本文件适用于 `src-tauri/src/` 及其子目录，并在仓库根目录 `AGENTS.md` 的通用规则之上补充后端约束。

## 提供商

- 设计和实现提供商时，以只读参考仓库 `D:\AppData\0_repo_references\pi\` 作为参考，其中 `README.md` 是项目入口，提供商相关实现主要位于 `packages/ai/`；本地资料不足或需要核对上游变化时，再参考官方远程仓库 <https://github.com/earendil-works/pi>。参考其统一接口、协议适配和分发思路，不照搬与本项目需求无关的能力、依赖或架构；本项目现有 Rust 边界和明确需求优先。
- 严格按照提供商的官方文档、SDK、示例和测试用例实现协议适配，避免自定义或猜测协议行为；如果官方文档不够明确，优先参考官方 SDK 或示例代码的实际行为。
- 提供商目前只需要支持推理和联网能力。除非 Floratina 明确提出新需求，不增加识图、多模态或其它能力，也不为这些未启用能力预留抽象、字段、配置或兼容分支。
- 沿用现有“统一领域模型与运行时 + 分协议编解码 + 集中注册”的结构：
  - 每种独立 wire protocol 在 `providers/protocols/<protocol>.rs` 中单独实现 `ProtocolCodec`，负责该协议的端点生成、请求编码、响应与错误解码，以及协议特有的参数校验；不要把多个协议堆进同一个文件，也不要在协议文件中实现通用 HTTP 调度。
  - `providers/registry.rs` 是协议元数据及 `ProtocolCodec` 绑定的唯一注册和分发来源；`providers/protocols/mod.rs` 只声明协议模块。新增协议时同步接入这两个位置以及对应的能力画像，不在 `commands.rs`、`db.rs` 或 `translation/` 中另建重复的协议映射或分发链。
  - `providers/runtime.rs` 统一负责传输和请求执行，业务调用方通过 `RuntimeAdapter`、`UnifiedChatRequest` 与 `UnifiedChatResponse` 工作，不直接依赖具体协议的请求或响应结构。协议差异留在 `protocols/`，能力差异留在 `capabilities/`，已复现且明确需要的兼容协商留在 `compatibility.rs` 与 `negotiation.rs`。
  - 跨协议确实共享的行为复用 `codec.rs`、`shared.rs`、`budget.rs`、`headers.rs`、`config_schema.rs` 等现有公共模块；只有在至少存在真实共享关系时才上提公共逻辑。使用相同 wire protocol 的服务商优先复用既有 codec 和集中描述信息，不复制一份近似协议实现；只有 wire contract 实质不同才新增协议文件。
