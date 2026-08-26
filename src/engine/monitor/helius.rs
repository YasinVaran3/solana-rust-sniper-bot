//! Helius-compatible monitor. Uses the same logsSubscribe path; point RPC_WSS at a Helius
//! enhanced websocket for lower latency.
pub use super::ws::{pumpfun_monitor, raydium_monitor};
