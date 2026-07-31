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
