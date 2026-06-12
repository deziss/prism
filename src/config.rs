// PRISM config.rs — YAML config loader + .prismrc parser
// Supports 7-layer memory, 5 MCP tools, GraphRAG

use serde::{Deserialize, Serialize};
use serde_yaml;
use std::path::PathBuf;

/// PRISM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrismConfig {
    pub data_dir: PathBuf,
    pub toon_enabled: bool,
    pub tron_enabled: bool,
    pub graph_enabled: bool,
    pub compression_ratio: Option<f64>,
    pub cache_enabled: bool,
    pub cache_dir: Option<PathBuf>,
    pub proxy_port: Option<u16>,
    pub mcp_port: Option<u16>,
    pub tiktoken_model: Option<String>,
    pub llmlingua_path: Option<String>,
    pub memory_tier: Option<String>,
    pub log_level: Option<String>,
}

impl Default for PrismConfig {
    fn default() -> Self {
        PrismConfig {
            data_dir: PathBuf::new(),
            toon_enabled: true,
            tron_enabled: false,
            graph_enabled: true,
            compression_ratio: Some(0.55),
            cache_enabled: true,
            cache_dir: None,
            proxy_port: None,
            mcp_port: None,
            tiktoken_model: Some("gpt-4".to_string()),
            llmlingua_path: None,
            memory_tier: Some("3".to_string()),
            log_level: Some("info".to_string()),
        }
    }
}

/// Load global config from ~/.prism/config.yaml
pub fn load_global() -> Option<PrismConfig> {
    if let Some(dir) = dirs::config_dir() {
        let file = dir.join("prism").join("config.yaml");
        if file.is_file() {
            if let Ok(s) = std::fs::read_to_string(&file) {
                if let Ok(cfg) = serde_yaml::from_str::<PrismConfig>(&s) {
                    return Some(cfg);
                }
            }
        }
    }
    None
}

/// Load project-local config from .prismrc
pub fn load_project<P: AsRef<std::path::Path>>(dir: P) -> Option<PrismConfig> {
    let file = dir.as_ref().join(".prismrc");
    if file.is_file() {
        if let Ok(s) = std::fs::read_to_string(&file) {
            if let Ok(cfg) = serde_yaml::from_str::<PrismConfig>(&s) {
                return Some(cfg);
            }
        }
    }
    None
}

/// Merge project config over global (project keys take precedence)
pub fn merge_config(global: &PrismConfig, project: &PrismConfig) -> PrismConfig {
    let default = PrismConfig::default();
    PrismConfig {
        data_dir: if project.data_dir.as_os_str().is_empty() && global.data_dir.as_os_str().is_empty() {
            default.data_dir
        } else if project.data_dir.as_os_str().is_empty() {
            global.data_dir.clone()
        } else {
            project.data_dir.clone()
        },
        toon_enabled: if project.toon_enabled { project.toon_enabled } else { global.toon_enabled },
        tron_enabled: if project.tron_enabled { project.tron_enabled } else { global.tron_enabled },
        graph_enabled: if project.graph_enabled { project.graph_enabled } else { global.graph_enabled },
        compression_ratio: project.compression_ratio.or(global.compression_ratio),
        cache_enabled: if project.cache_enabled { project.cache_enabled } else { global.cache_enabled },
        cache_dir: project.cache_dir.clone().or(global.cache_dir.clone()),
        proxy_port: project.proxy_port.or(global.proxy_port),
        mcp_port: project.mcp_port.or(global.mcp_port),
        tiktoken_model: project.tiktoken_model.clone().or(global.tiktoken_model.clone()),
        llmlingua_path: project.llmlingua_path.clone().or(global.llmlingua_path.clone()),
        memory_tier: project.memory_tier.clone().or(global.memory_tier.clone()),
        log_level: project.log_level.clone().or(global.log_level.clone()),
    }
}

/// Save global config
pub fn save_global(cfg: &PrismConfig) -> std::io::Result<()> {
    let dir = default_config_dir();
    ensure_config_dir(&dir)?;
    let yaml = serde_yaml::to_string(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join("config.yaml"), yaml)
}

/// Initialize ~/.prism directory
pub fn init_global(data_dir: &Option<PathBuf>) -> std::io::Result<()> {
    let dir = data_dir
        .clone()
        .unwrap_or_else(|| default_config_dir());
    ensure_config_dir(&dir)?;
    let layers = ["recall", "core", "archive", "cache", "graphs", "sessions"];
    for layer in layers {
        std::fs::create_dir_all(dir.join(layer))?;
    }
    let cfg = PrismConfig::default();
    let yaml = serde_yaml::to_string(&cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(dir.join("config.yaml"), yaml)?;
    Ok(())
}

/// Get default data directory
pub fn default_data_dir() -> PathBuf {
    let mut d = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    d.push("prism");
    d
}

pub fn is_initialized(data_dir: &Option<PathBuf>) -> bool {
    let dir = data_dir
        .clone()
        .unwrap_or_else(|| default_config_dir());
    dir.join("config.yaml").is_file()
}

fn default_config_dir() -> PathBuf {
    let mut d = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    d.push("prism");
    d
}

fn ensure_config_dir(dir: &PathBuf) -> std::io::Result<()> {
    if !dir.is_dir() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Serialize config to JSON
pub fn config_to_json(cfg: &PrismConfig) -> String {
    serde_json::to_string_pretty(cfg).unwrap_or_default()
}

/// Deserialize config from JSON
pub fn config_from_json(s: &str) -> Option<PrismConfig> {
    serde_json::from_str(s).ok()
}
