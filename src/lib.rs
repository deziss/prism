pub mod analytics;
pub mod cache;
pub mod cli;
pub mod compress;
pub mod config;
pub mod encode;
pub mod filter;
pub mod hook;
pub mod knowledge;
pub mod mcp;
pub mod memory;
pub mod proxy;
pub mod utils;
pub mod vector;
pub mod vscode;

use std::path::PathBuf;

pub fn prism_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prism")
}

pub fn prism_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prism")
}

pub fn init_prism_dirs() -> anyhow::Result<()> {
    std::fs::create_dir_all(prism_data_dir().join("memory"))?;
    std::fs::create_dir_all(prism_data_dir().join("graph"))?;
    std::fs::create_dir_all(prism_data_dir().join("cache"))?;
    std::fs::create_dir_all(prism_data_dir().join("analytics"))?;
    std::fs::create_dir_all(prism_data_dir().join("hooks"))?;
    Ok(())
}
