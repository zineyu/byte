use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::{SessionContext, ToolCall};
use tokio_util::sync::CancellationToken;

use crate::{Tool, ToolError, ToolPolicy, ToolRegistry};

/// A simple in-memory tool registry used in the MVP.
pub struct MvpToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    policies: HashMap<String, Arc<dyn ToolPolicy>>,
}

impl std::fmt::Debug for MvpToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MvpToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .field("policies", &self.policies.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl MvpToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            policies: HashMap::new(),
        }
    }
}

impl Default for MvpToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolRegistry for MvpToolRegistry {
    fn register(&mut self, name: String, tool: Arc<dyn Tool>, policy: Arc<dyn ToolPolicy>) {
        let _ = self.tools.insert(name.clone(), tool);
        let _ = self.policies.insert(name, policy);
    }

    fn definitions(&self) -> Vec<byte_protocol::ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    fn get(&self, name: &str) -> Option<(Arc<dyn Tool>, Arc<dyn ToolPolicy>)> {
        let tool = self.tools.get(name)?.clone();
        let policy = self.policies.get(name)?.clone();
        Some((tool, policy))
    }

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &SessionContext,
        cancel: &CancellationToken,
    ) -> Result<String, ToolError> {
        let (tool, policy) = self
            .get(&call.name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {}", call.name)))?;
        policy.check(call, ctx)?;
        tool.invoke(call, ctx, cancel).await
    }
}
