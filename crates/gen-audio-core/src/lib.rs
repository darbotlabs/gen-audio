//! Shared types for the Gen-Audio desktop app, MCP server, and harness.
//!
//! This crate does not synthesize speech and does not ship model weights.

pub mod asset;
pub mod asset_migrate;
pub mod benchmark;
pub mod bridge;
pub mod cards;
pub mod catalog;
pub mod engines;
pub mod fixture;
pub mod paths;
pub mod redact;
pub mod script;
pub mod serve;

pub use engines::{engine, engines, EngineSpec};
pub use serve::{health_payload, health_url, node_base_url, shared_gateway_row, SHARED_GATEWAY_HOST};
