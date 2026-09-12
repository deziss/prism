pub mod analytics;
pub mod cache;
pub mod cli;
pub mod compress;
pub mod config;
pub mod encode;
pub mod filter;
pub mod guide;
pub mod hook;
pub mod hooks;
pub mod hub;
pub mod image;
pub mod knowledge;
pub mod mcp;
pub mod memory;
pub mod proxy;
pub mod reader;
pub mod shim;
pub mod uninstall;
pub mod utils;
pub mod vector;
pub mod vscode;

use std::path::PathBuf;

/// Where prism keeps its store. `PRISM_DATA_DIR` relocates it — useful for keeping a
/// project's cache and analytics out of the user-wide store, and for tests that must not
/// write to the real one.
pub fn prism_data_dir() -> PathBuf {
    match std::env::var("PRISM_DATA_DIR") {
        Ok(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ => dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("prism"),
    }
}

/// Where prism reads its configuration. `PRISM_CONFIG_DIR` relocates it.
pub fn prism_config_dir() -> PathBuf {
    match std::env::var("PRISM_CONFIG_DIR") {
        Ok(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ => dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("prism"),
    }
}

pub fn init_prism_dirs() -> anyhow::Result<()> {
    std::fs::create_dir_all(prism_data_dir().join("memory"))?;
    std::fs::create_dir_all(prism_data_dir().join("graph"))?;
    std::fs::create_dir_all(prism_data_dir().join("cache"))?;
    std::fs::create_dir_all(prism_data_dir().join("analytics"))?;
    std::fs::create_dir_all(prism_data_dir().join("hooks"))?;
    Ok(())
}
