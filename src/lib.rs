//! Metadata control plane and a separate process for gateway-managed execution.
pub mod admin;
pub mod backup;
pub mod catalog;
pub mod core;
pub mod discovery;
pub mod gateway;
pub mod gateway_process;
pub mod host_process;
pub mod identity;
pub mod network;
pub mod protocol;
mod provider;
pub mod service;
pub mod shared_work;
pub mod upstream_auth;
mod web_admin;
