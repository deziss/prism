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
    /// Output-filter caps (`prism cmd`). Every cap is announced with a `[+N more …]` marker.
    #[serde(default)]
    pub filters: FilterLimits,
}

/// Caps used by the output filters. Override per key in `config.yaml` under
/// `filters:` or via env `PRISM_FILTER_<UPPER_NAME>` (e.g. `PRISM_FILTER_GREP_MAX_PER_FILE=50`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FilterLimits {
    /// grep/rg: matches shown per file before `[+N more matches]`
    pub grep_max_per_file: usize,
    /// grep/rg: total matches shown
    pub grep_max_results: usize,
    /// grep/rg: match line width before `…`
    pub grep_line_width: usize,
    /// find/fd: entries listed per directory
    pub find_max_per_dir: usize,
    /// find/fd: directories listed
    pub find_max_dirs: usize,
    /// ls: entries listed per directory
    pub ls_max_entries: usize,
    /// dependency trees / tabular lists (npm ls, cargo tree, docker images…)
    pub list_max_lines: usize,
    /// JSON renderer: lines
    pub json_max_lines: usize,
    /// JSON renderer: items per array
    pub json_max_array: usize,
    /// log tails (docker logs, kubectl logs, stern)
    pub log_tail: usize,
    /// unknown commands: lines kept
    pub passthrough_max_lines: usize,
    /// diffs: unchanged context lines kept around each change
    pub diff_context: usize,
    /// git status: files listed per section
    pub status_max_files: usize,
    /// test runners: failures shown in full
    pub test_max_failures: usize,
    /// compiler/linter diagnostics shown in full
    pub max_diagnostics: usize,
    /// process/socket rows kept (`ps`, `ss`), largest first
    pub ps_max_rows: usize,
}

impl Default for FilterLimits {
    fn default() -> Self {
        FilterLimits {
            grep_max_per_file: 25,
            grep_max_results: 200,
            grep_line_width: 160,
            find_max_per_dir: 40,
            find_max_dirs: 80,
            ls_max_entries: 200,
            list_max_lines: 200,
            json_max_lines: 300,
            json_max_array: 50,
            log_tail: 100,
            passthrough_max_lines: 400,
            diff_context: 2,
            status_max_files: 30,
            test_max_failures: 10,
            max_diagnostics: 40,
            ps_max_rows: 40,
        }
    }
}

impl FilterLimits {
    /// Apply `PRISM_FILTER_<NAME>` env overrides.
    pub fn apply_env(&mut self) {
        let set = |name: &str, slot: &mut usize| {
            if let Ok(v) = std::env::var(format!("PRISM_FILTER_{}", name.to_ascii_uppercase())) {
                if let Ok(n) = v.trim().parse::<usize>() { *slot = n }
            }
        };
        set("grep_max_per_file", &mut self.grep_max_per_file);
        set("grep_max_results", &mut self.grep_max_results);
        set("grep_line_width", &mut self.grep_line_width);
        set("find_max_per_dir", &mut self.find_max_per_dir);
        set("find_max_dirs", &mut self.find_max_dirs);
        set("ls_max_entries", &mut self.ls_max_entries);
        set("list_max_lines", &mut self.list_max_lines);
        set("json_max_lines", &mut self.json_max_lines);
        set("json_max_array", &mut self.json_max_array);
        set("log_tail", &mut self.log_tail);
        set("passthrough_max_lines", &mut self.passthrough_max_lines);
        set("diff_context", &mut self.diff_context);
        set("status_max_files", &mut self.status_max_files);
        set("test_max_failures", &mut self.test_max_failures);
        set("max_diagnostics", &mut self.max_diagnostics);
        set("ps_max_rows", &mut self.ps_max_rows);
    }
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
            filters: FilterLimits::default(),
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
        filters: if project.filters != FilterLimits::default() { project.filters.clone() } else { global.filters.clone() },
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
