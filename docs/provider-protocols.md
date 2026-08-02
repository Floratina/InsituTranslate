# Provider Protocols

协议实现位于 `src-tauri/src/providers/protocols/`。每个协议有一个主 `.rs` 文件，负责线路格式知识；`providers/runtime.rs` 只负责异步 HTTP、凭证、公共请求头、流读取、超时、遥测和受控重试。

## 新增协议清单

1. 新建一个协议文件，实现 `ProtocolCodec` 的同步方法：模型列表请求和解码、聊天请求编码、完整响应和错误解码、流事件解码、能力推断、思考强度映射和端点预览。
2. 只使用 `providers::shared` 中的无协议工具，例如 JSON 深度合并、消息文本提取、SSE/NDJSON framing、Usage 和 logprob 计算。协议文件不应依赖 `crate::adapters`，也不应发送网络请求。
3. 在 `providers/registry.rs` 增加一个 `ProtocolDescriptor` 注册项，填写唯一 ID、`wire_family`、默认 URL、模型列表能力、`AuthStrategy`、可选 `config_fields` 和帮助文本，并将 `codec` 指向该文件的静态 Codec。
4. 凭证优先选择已有鉴权策略：`None`、静态 Header、Bearer 或 Vertex Service Account。只有协议引入全新的 OAuth 流程时，才在 Runtime 增加独立鉴权模块。
5. 为 URL、协议头、请求体、模型列表、完整响应、错误响应和任意字节边界的流事件添加 golden 测试。测试应通过注册表调用 Codec，不能在 Runtime 增加协议分支。
6. 若协议需要普通配置字段，在描述中用 JSON Pointer 定义 `text`、`number`、`boolean` 或 `select` 字段、默认值、必填约束和选项；敏感值必须使用系统凭证存储，不写入 `config_json`。只有需要复杂交互的配置才使用专用 `configKind` 页面。
7. 若协议与已有线路存在确定性兼容差异，在 `providers/compatibility.rs` 增加明确的 `wire_family`、Host、模型和错误白名单规则。规则只能安排一次安全重试，必须保留审计信息，不得探测或持久学习。

完成后应运行：

- `cargo fmt --check`
- 协议、注册表、能力、数据库和 Runtime 定向测试
- `cargo test --lib`
- `pnpm build`
- `pnpm tauri build`

未知或废弃协议不能阻止数据库启动；其原始字符串通过 `protocolRawId` 展示，修复前所有联网、抓取模型、测试和启用操作都必须在 Runtime 之前被拒绝。

## 注册表完整性

注册表是协议身份、Codec、鉴权和配置 Schema 的单一事实源。首次查询以及 Tauri 应用启动时都会校验完整注册表；任何一项失败都会阻止启动，并在错误中指出协议或 Pointer：

- 协议 ID 必须非空、唯一，且不能使用保留值 `unknown`；`ProtocolDescriptor.id` 必须与 `ProtocolCodec::id()` 完全一致。
- 默认 Base URL 必须是具有 Host 的 HTTP(S) 绝对 URL，静态鉴权 Header 必须是合法 Header 名。
- 配置 Pointer 必须是指向对象字段的 RFC 6901 Pointer，不能指向文档根、重复、重叠或包含非法转义；RFC 6901 的空对象键段是合法的。
- 默认 JSON 必须能解析且符合字段类型；Select 必须有非空且唯一的选项，默认值必须属于选项。

所有注册表查询和面向 IPC 的 Descriptor 转换都返回 `Result`。不要使用 `expect` 解析 Descriptor 数据，也不要绕过注册表直接按 ID 选择 Codec。

## 配置 Schema 生命周期

普通配置字段支持 `text`、`number`、`boolean` 和 `select`。其值分别必须是 JSON string、number、boolean，以及属于已注册选项的 string。必填字段缺失、为 `null` 或空字符串时无效；其他字段为 `null` 时同样不符合其声明类型。未在 Schema 中声明的键必须原样保留。

配置按以下顺序处理：

1. 合并协议的专用配置默认值，例如 Vertex AI 或 MinerU 配置。
2. 物化 Schema 字段默认值。已有值优先，不覆盖已有键。
3. 按字段类型、required 和 Select 选项执行验证。

新建 Provider 时默认值会写入 `config_json`。读取、复制和构造 Runtime 时会在内存中补齐后来新增的默认值，使旧记录可以立即使用；普通读取不会改写数据库，用户显式保存后才会持久化这些新默认值。Schema 不完整的记录仍通过 `ProviderView.configIssues` 展示和编辑，但不能启用、抓取模型、测试连接或构造 Runtime。

## 协议身份与修复

Provider 创建后协议不可通过普通元数据编辑修改。`UpdateProviderMetadataInput` 只包含名称和头像。

只有持久化协议无法在注册表中解析的 Provider 才能调用 `repair_provider_protocol`。修复会保留 Base URL、配置中的未知键、模型、凭证引用和自定义 Header，补齐目标协议默认配置，同时禁用 Provider，并把全部模型测试状态重置为 `untested`。修复后必须重新检查配置和模型连接，再由用户显式启用。

Provider 复制会先严格读取源凭证，再写入目标凭证。若数据库事务失败，已写入的目标凭证和 Header 必须补偿删除；凭证掩码或 Header 键元数据存在但秘密缺失时，复制与 Runtime 构造都必须明确失败。

## Runtime 错误与流

聊天和模型列表的所有非成功 HTTP 响应都必须经过当前 `ProtocolCodec::decode_error`，同时保留 HTTP 状态、限流遥测和兼容性协商使用的错误文本。兼容性重试只能执行一次，并且只允许已登记的 wire family、Host、模型、参数和错误组合。

`JsonEventStreamDecoder` 同时处理规范 SSE 与 Ollama 式 NDJSON。SSE 支持 `data`、`event`、`id`、`retry`、注释、CRLF、多行 data 和 `[DONE]`；解析必须适用于任意字节边界、拆分的 UTF-8、多事件 Chunk 和结束 remainder。损坏 UTF-8、非法 JSON 或 Codec 解码失败必须立即返回带协议 ID 的错误，不能跳过事件或返回空成功。

通用 `runtime_chat` 和 `runtime_chat_stream` 不属于 Tauri IPC。`RuntimeAdapter` 仅供翻译、自动术语表等 Rust 内部调用和测试使用。
