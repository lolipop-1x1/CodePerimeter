pub mod eslogger;
pub mod history;
pub mod model;
pub mod native;
#[cfg(target_os = "macos")]
pub mod notifications;
pub mod rules;
pub mod runtime;
pub mod service;
pub mod storage;
pub mod web;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub mod console;
