//! Metadata control plane and a separate process for gateway-managed execution.
pub mod admin;
pub mod catalog;
pub mod core;
pub mod discovery;
pub mod gateway;
pub mod gateway_process;
pub mod host_process;
pub mod network;
pub mod protocol;
mod provider;
pub mod service;
mod web_admin;
