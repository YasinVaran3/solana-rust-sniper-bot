use std::{sync::Arc, time::Instant};

use crate::common::{config::AppConfig, utils::AppState};
use crate::core::risk::RiskEngine;

#[derive(Clone)]
pub struct AppContext {
    pub state: AppState,
    pub config: Arc<AppConfig>,
    pub risk: Arc<RiskEngine>,
    pub http: reqwest::Client,
    pub started: Instant,
}
