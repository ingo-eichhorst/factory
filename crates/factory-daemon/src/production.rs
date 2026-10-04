//! Outside-stack production endpoint: the exact-scope L4 fact read.
use crate::engine::Engine;
use chrono::Utc;
use factory_core::{
    error::Result,
    protocol::{Production, ProductionBin},
};
use std::sync::Arc;

impl Engine {
    pub async fn production(
        self: &Arc<Self>,
        minutes: Option<u32>,
        bin: Option<ProductionBin>,
        scope: Option<String>,
    ) -> Result<Production> {
        crate::facts::Facts::<factory_kernel::People>::new(self)
            .get::<factory_kernel::ProductionFact>(&crate::facts::ProductionQuery {
                scope,
                now: Utc::now(),
                minutes,
                bin: bin.unwrap_or(ProductionBin::Day),
                subtree: false,
            })
            .await
    }
}
