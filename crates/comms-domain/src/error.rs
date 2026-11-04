use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("invalid identifier: {0}")]
    InvalidId(String),
    #[error("unknown value `{value}` for {what}")]
    UnknownValue { what: &'static str, value: String },
}
