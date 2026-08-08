use std::sync::Arc;

use serde_json::Value;

use crate::runtime::backend::BackendClient;
use crate::runtime::tools::domain_engine;
use crate::runtime::tools::domain_schema::DomainRegistry;
use crate::runtime::tools::registry::{ToolImpact, ToolRegistry};

pub fn register(registry: &mut ToolRegistry, backend: Arc<BackendClient>) {
    let domains = DomainRegistry::load();
    let description = domains.build_tool_description();
    let schema = domains.build_tool_schema();

    let b = Arc::clone(&backend);
    registry.register_with_impact(
        "data_crud",
        &description,
        schema,
        ToolImpact::ReversibleMutation,
        Box::new(move |args: &Value| {
            let b = Arc::clone(&b);
            let args = args.clone();
            Box::pin(async move { domain_engine::execute(&b, &args).await })
        }),
    );
}
