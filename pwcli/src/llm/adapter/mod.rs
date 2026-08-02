//! Protocol adapters that translate the internal LLM contract to vendor APIs.
//!
//! Business code should depend on [`crate::llm::LlmClient`] or the shared
//! request/response types. Vendor wire shapes stay inside this module.

mod anthropic_messages;
mod google_generative;
mod openai_chat;
mod openai_responses;
mod registry;

pub use registry::{create_adapter, LlmAdapter};
