pub mod capabilities;
pub mod codec;
pub mod compatibility;
pub mod config_schema;
pub mod negotiation;
pub mod protocols;
pub mod registry;
pub mod runtime;
pub mod shared;
pub mod thinking;

#[cfg(test)]
pub mod test_support;

#[cfg(test)]
mod golden_tests;

#[cfg(test)]
mod behavior_tests;

pub use codec::{
    EncodedRequest, EndpointPreview, HeaderDirective, HeaderMode, HttpMethod, ProtocolCodec,
};
pub use registry::descriptor_for;
pub use runtime::{
    finish_reason_is_truncation, ProviderAdapter, ProviderChatError, ProviderChatMeta,
    RateLimitTelemetry, RuntimeAdapter,
};
