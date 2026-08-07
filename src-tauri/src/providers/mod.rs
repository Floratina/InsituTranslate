pub mod budget;
pub mod capabilities;
pub mod codec;
pub mod compatibility;
pub mod config_schema;
mod headers;
pub mod negotiation;
pub mod protocols;
mod rate_limits;
pub mod registry;
pub mod runtime;
pub mod shared;

#[cfg(test)]
pub mod test_support;

#[cfg(test)]
mod golden_tests;

#[cfg(test)]
mod behavior_tests;

pub use budget::CompletionBudget;
pub use codec::{
    EncodedRequest, EndpointPreview, HeaderDirective, HeaderMode, HttpMethod, ProtocolCodec,
};
pub use rate_limits::RateLimitTelemetry;
pub use registry::descriptor_for;
pub use runtime::{
    finish_reason_is_truncation, ChatAttemptContext, ChatAttemptGate, ChatAttemptKind,
    ChatAttemptOutcome, PreparedChatRequest, ProviderChatError, ProviderChatMeta, RequestCost,
    RuntimeAdapter,
};
