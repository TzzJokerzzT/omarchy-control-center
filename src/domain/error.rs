//! Typed adapter errors. Every variant is meant to be shown to a human, so the
//! messages state what failed and what was not determined — never an opaque code.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AdapterError {
    #[error("{tool} is not available: {reason}")]
    ToolUnavailable { tool: String, reason: String },

    #[error("{context} failed ({command}): {detail}")]
    Command {
        context: String,
        command: String,
        detail: String,
    },

    #[error("could not parse {context}: {detail}")]
    Parse { context: String, detail: String },

    #[error("{capability} is not supported by {adapter}")]
    Unsupported { adapter: String, capability: String },

    #[error("refusing identifier {value:?} for {adapter}: {reason}")]
    InvalidId {
        adapter: String,
        value: String,
        reason: String,
    },

    #[error("{0}")]
    Other(String),
}

impl AdapterError {
    pub fn command(
        context: impl Into<String>,
        command: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::Command {
            context: context.into(),
            command: command.into(),
            detail: detail.into(),
        }
    }

    pub fn unsupported(adapter: impl Into<String>, capability: impl Into<String>) -> Self {
        Self::Unsupported {
            adapter: adapter.into(),
            capability: capability.into(),
        }
    }
}
