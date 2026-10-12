//! Shared types for the Gen-Audio desktop app, MCP server, and harness.
//!
//! Local synthesis is delegated to a bounded Python adapter and independently
//! verified before returning speech. Model weights are not shipped.

pub mod asset;
pub mod asset_catalog;
pub mod asset_migrate;
pub mod authoring;
pub mod audio_metadata;
pub mod benchmark;
pub mod bridge;
pub mod cards;
pub mod catalog;
pub mod engines;
pub mod execution;
pub mod fixture;
pub mod library_store;
pub mod generation_store;
pub mod mastering;
pub mod model_worker;
pub mod paths;
pub mod redact;
pub mod runtime_config;
pub mod runtime_owner;
pub mod script;
pub mod serve;
pub mod viewport;
pub mod subprocess;
pub mod studio;
pub mod voice_library;
pub mod semantic_cube;

pub use engines::{engine, engines, EngineSpec};
pub use serve::{health_payload, health_url, node_base_url, shared_gateway_row, SHARED_GATEWAY_HOST};
