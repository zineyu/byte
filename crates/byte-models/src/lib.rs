//! 模型提供者的抽象与配置。
//!
//! 该 crate 封装了不同大模型服务提供者的统一接口、配置加载以及
//! `OpenAI` 兼容提供者的实现。
#![deny(rustdoc::broken_intra_doc_links)]

/// 模型提供者配置定义与加载。
pub mod config;
/// `OpenAI` 兼容提供者实现。
pub mod openai;
/// 模型提供者 trait 与通用类型。
pub mod provider;
/// 模型提供者工厂函数。
pub mod provider_factory;

pub use config::{ModelProviderConfig, load_config, load_config_at_path};
pub use openai::OpenAiCompatibleProvider;
pub use provider::{EchoProvider, ModelProvider, ProviderError, ProviderEvent, ProviderStream};
pub use provider_factory::{ProviderFactoryError, create_provider};
