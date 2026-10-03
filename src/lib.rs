pub mod eslogger;
pub mod history;
pub mod model;
pub mod rules;
pub mod runtime;
pub mod service;
pub mod storage;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
