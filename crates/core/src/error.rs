use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("LLM error: {0}")]
    Llm(String),
    #[error("Storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("Tool execution error: {0}")]
    Tool(String),
    #[error("Agent planning error: {0}")]
    Planning(String),
    #[error("Knowledge error: {0}")]
    Knowledge(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;
