// PRISM config.rs — YAML config loader + .prismrc parser
// Supports 3-tier memory (recall/core/archive), 17 MCP tools, GraphRAG

use serde::{Deserialize, Serialize};
use serde_yaml;
use std::path::PathBuf;

/// PRISM configuration.
///
/// **Why the feature flags are `Option<bool>` and not `bool`.** `merge_config` layers
/// default -> global -> project -> hub, and with a plain `bool` an unset field and an
/// explicit `false` are indistinguishable, so the merge could only ever turn a feature
/// *on*: `if project.x { project.x } else { global.x }` can never propagate a `false`.
/// That made hub-distributed policy one-way, which is useless for a control plane whose
/// job includes switching things off. `None` now means "not stated at this layer", and
/// the `*_enabled()` accessors resolve to the documented default.
///
/// The container is `#[serde(default)]` so a *partial* document parses — the hub sends
/// only the keys a policy actually sets, and previously every absent field was a hard
/// deserialization error, which is why `prism hub config` could never ingest a real
/// response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrismConfig {
    pub data_dir: PathBuf,
    pub toon_enabled: Option<bool>,
    pub tron_enabled: Option<bool>,
    pub graph_enabled: Option<bool>,
    pub compression_ratio: Option<f64>,
    pub cache_enabled: Option<bool>,
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
    /// Per-tool filter switches, keyed by tool name (`{"docker": false}`).
    ///
    /// A **hub feature**, on the same mechanism as [`Self::graph_enabled`]: a licensed
    /// PRISM Hub distributes policy containing these, and prism honours what arrives.
    /// prism evaluates no entitlement and checks no licence — that decision is the
    /// hub's, gated on a licencia entitlement server-side.
    ///
    /// Deliberately additive. The sixteen numeric caps in [`FilterLimits`] stay local
    /// and free, so nobody loses configuration they already have; this only adds the
    /// ability to switch an individual tool's filter off fleet-wide.
    ///
    /// `Option` rather than a bare map: with a plain map, "unset" and "explicitly
    /// empty" are indistinguishable, and policy could then only ever add switches,
    /// never clear them — useless for a control plane whose job includes undoing.
    /// No `skip_serializing_if`: [`known_field_names`] derives the legal wire keys from
    /// the *serialized* default, so a skipped field would be missing from that set and
    /// prism would reject the very policy the hub sends — the feature would fail closed
    /// with a confusing "unknown key" error.
    #[serde(default)]
    pub filter_toggles: Option<std::collections::BTreeMap<String, bool>>,
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
                if let Ok(n) = v.trim().parse::<usize>() {
                    *slot = n
                }
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
            // None = "not stated"; the documented defaults live in the accessors below
            // so that `Default` can still round-trip through the merge chain.
            toon_enabled: None,
            tron_enabled: None,
            graph_enabled: None,
            compression_ratio: Some(0.55),
            cache_enabled: None,
            cache_dir: None,
            proxy_port: None,
            mcp_port: None,
            tiktoken_model: Some("gpt-4".to_string()),
            llmlingua_path: None,
            memory_tier: Some("3".to_string()),
            log_level: Some("info".to_string()),
            filters: FilterLimits::default(),
            filter_toggles: None,
        }
    }
}

impl PrismConfig {
    /// TOON array encoding. On unless a layer says otherwise.
    pub fn toon_enabled(&self) -> bool {
        self.toon_enabled.unwrap_or(true)
    }

    /// TRON box-drawing table rendering. Off by default — it costs more tokens than TOON
    /// and exists for human-readable output.
    pub fn tron_enabled(&self) -> bool {
        self.tron_enabled.unwrap_or(false)
    }

    /// GraphRAG codebase intelligence. **Off** unless a layer states otherwise.
    ///
    /// This is a hub feature: a licensed PRISM Hub distributes policy that sets
    /// `graph_enabled: true`, which lands in `<data>/hub-config.yaml` via
    /// [`crate::hub::fetch_config`] and is picked up here as the top layer of
    /// [`resolve`]. "Community" is therefore just *no policy saying otherwise* —
    /// prism honours the flag and never evaluates an entitlement, checks a licence, or
    /// makes a network call to decide. See [`disabled_by_policy`].
    pub fn graph_enabled(&self) -> bool {
        self.graph_enabled.unwrap_or(false)
    }

    /// Semantic cache. **Off** unless a layer states otherwise — a hub feature, on the
    /// same mechanism as [`Self::graph_enabled`].
    ///
    /// This governs the *store*. Whether the proxy may additionally *serve* a stored
    /// response in place of a live model call stays separately opt-in via
    /// `PRISM_CACHE_SERVE`, and `PRISM_CACHE_RECORD=0` remains the privacy kill switch
    /// for writing. Both are preferences *within* an enabled cache: with this `false`
    /// the proxy never keys a request at all, so neither env var can reach the store.
    pub fn cache_enabled(&self) -> bool {
        self.cache_enabled.unwrap_or(false)
    }

    /// Whether `prism cmd` should filter this tool's output.
    ///
    /// Defaults to **true**: with no hub policy, every filter behaves exactly as it
    /// always has. Only an explicit `false` from a policy layer turns one off, so a
    /// community install is unaffected and an unreachable hub cannot silently disable
    /// filtering.
    pub fn filter_enabled_for(&self, tool: &str) -> bool {
        self.filter_toggles
            .as_ref()
            .and_then(|t| t.get(tool))
            .copied()
            .unwrap_or(true)
    }
}

/// What to do about a hub feature that no configuration layer enables, and the
/// reassurance that the rest of prism is unaffected.
///
/// One text, in one place, so the remedy a user is handed cannot drift between
/// `prism graph`, `prism cache`, the MCP tools and the proxy.
pub const HUB_FEATURE_HELP: &str = "\
GraphRAG (`prism graph`) and the semantic cache (`prism cache`) are PRISM Hub features,
switched on by the policy a licensed hub distributes. Enrol this agent to turn them on:

    prism hub enroll --url <hub> --token <join-token>

Nothing else needs a hub. `prism cmd` and its filters, the PATH shims, `read`, `count`,
`compress`, `toon`, `memory`, `gain`, `mcp` and the local `serve` proxy are unaffected.";

/// The message a disabled hub feature reports: which feature, that it is *disabled*
/// rather than empty or broken, and [`HUB_FEATURE_HELP`].
///
/// Every gate routes its wording through here. The distinction matters because both
/// chokepoints return `Option` — a naive gate makes "off" indistinguishable from "no
/// graph indexed" or "cache unreachable", which sends the user to fix the wrong thing.
pub fn disabled_by_policy(feature: &str) -> String {
    format!("{feature} is disabled: no configuration layer enables it.\n\n{HUB_FEATURE_HELP}")
}

/// Every key `PrismConfig` understands on the wire, derived from the struct itself so it
/// cannot drift from the field list.
///
/// Used to reject a hub policy that names keys we would otherwise ignore in silence:
/// `#[serde(default)]` means an unrecognised key deserializes to the default instead of
/// erroring, so a camelCase or misspelled policy field would quietly do nothing.
pub fn known_field_names() -> std::collections::BTreeSet<String> {
    match serde_json::to_value(PrismConfig::default()) {
        Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        _ => std::collections::BTreeSet::new(),
    }
}

/// Load global config from ~/.config/prism/config.yaml
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

/// Load project-local config from `.prismrc`.
///
/// A missing file is `Ok(None)` — most projects do not have one. A file that exists but
/// fails to read or parse is `Err`, never a silent `None`: this used to be
/// `if let Ok(...)`, which swallowed a parse failure indistinguishably from "no file
/// here" — the exact failure mode that let a stray, invalid `.prismrc` (a shell `source`
/// line, not YAML) sit unnoticed. Callers decide how loud to be with the error (see
/// [`resolve`]), matching how `filter::rules::load_from` reports a bad rule file instead
/// of skipping it quietly.
pub fn load_project<P: AsRef<std::path::Path>>(dir: P) -> Result<Option<PrismConfig>, String> {
    let file = dir.as_ref().join(".prismrc");
    if !file.is_file() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&file).map_err(|e| format!("{}: {}", file.display(), e))?;
    serde_yaml::from_str::<PrismConfig>(&s)
        .map(Some)
        .map_err(|e| format!("{}: {}", file.display(), e))
}

/// Load hub-enforced policy from `<data>/hub-config.yaml`, written by `prism hub config`
/// (see `hub::fetch_config`). `None` when this agent has never fetched one.
pub fn load_hub() -> Option<PrismConfig> {
    let file = crate::prism_data_dir().join("hub-config.yaml");
    if !file.is_file() {
        return None;
    }
    match std::fs::read_to_string(&file) {
        Ok(s) => match serde_yaml::from_str::<PrismConfig>(&s) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                eprintln!(
                    "prism: warning: {} did not parse as YAML: {}",
                    file.display(),
                    e
                );
                None
            }
        },
        Err(e) => {
            eprintln!("prism: warning: could not read {}: {}", file.display(), e);
            None
        }
    }
}

/// Full configuration resolution: hub-enforced > project `.prismrc` > global
/// `config.yaml` > defaults. This is the one place that should be called from `proxy.rs`
/// / `filter::common::limits()` / `cli::config` — everywhere else that only ever read
/// `load_global()` was blind to both the project layer and the hub layer.
pub fn resolve() -> PrismConfig {
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let global = load_global().unwrap_or_default();
    let project = match load_project(&dir) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => PrismConfig::default(),
        Err(e) => {
            eprintln!("prism: warning: {e}");
            PrismConfig::default()
        }
    };
    let merged = merge_config(&global, &project);
    match load_hub() {
        Some(hub_cfg) => merge_config(&merged, &hub_cfg),
        None => merged,
    }
}

/// Merge project config over global (project keys take precedence)
pub fn merge_config(global: &PrismConfig, project: &PrismConfig) -> PrismConfig {
    let default = PrismConfig::default();
    PrismConfig {
        data_dir: if project.data_dir.as_os_str().is_empty()
            && global.data_dir.as_os_str().is_empty()
        {
            default.data_dir
        } else if project.data_dir.as_os_str().is_empty() {
            global.data_dir.clone()
        } else {
            project.data_dir.clone()
        },
        // `.or()`, not a truthiness test: the higher layer wins whenever it states a
        // value, including an explicit `false`. This is what lets hub policy disable a
        // feature rather than only enable one.
        toon_enabled: project.toon_enabled.or(global.toon_enabled),
        tron_enabled: project.tron_enabled.or(global.tron_enabled),
        graph_enabled: project.graph_enabled.or(global.graph_enabled),
        compression_ratio: project.compression_ratio.or(global.compression_ratio),
        cache_enabled: project.cache_enabled.or(global.cache_enabled),
        cache_dir: project.cache_dir.clone().or(global.cache_dir.clone()),
        proxy_port: project.proxy_port.or(global.proxy_port),
        mcp_port: project.mcp_port.or(global.mcp_port),
        tiktoken_model: project
            .tiktoken_model
            .clone()
            .or(global.tiktoken_model.clone()),
        llmlingua_path: project
            .llmlingua_path
            .clone()
            .or(global.llmlingua_path.clone()),
        memory_tier: project.memory_tier.clone().or(global.memory_tier.clone()),
        log_level: project.log_level.clone().or(global.log_level.clone()),
        filters: if project.filters != FilterLimits::default() {
            project.filters.clone()
        } else {
            global.filters.clone()
        },
        // `.or()`, like every other flag here: the higher layer wins when it states a
        // value, and silence falls through. Using `.unwrap_or_default()` would let a
        // project file with no toggles erase a hub policy that has them.
        filter_toggles: project
            .filter_toggles
            .clone()
            .or_else(|| global.filter_toggles.clone()),
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

/// Initialize prism configuration directory
pub fn init_global(data_dir: &Option<PathBuf>) -> std::io::Result<()> {
    let dir = data_dir.clone().unwrap_or_else(default_config_dir);
    ensure_config_dir(&dir)?;
    let layers = ["recall", "core", "archive", "cache", "graphs", "sessions"];
    for layer in layers {
        std::fs::create_dir_all(dir.join(layer))?;
    }
    let cfg = PrismConfig::default();
    let yaml = serde_yaml::to_string(&cfg).map_err(std::io::Error::other)?;
    std::fs::write(dir.join("config.yaml"), yaml)?;
    Ok(())
}

/// Get default data directory
pub fn default_data_dir() -> PathBuf {
    let mut d = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    d.push("prism");
    d
}

pub fn is_initialized(data_dir: &Option<PathBuf>) -> bool {
    let dir = data_dir.clone().unwrap_or_else(default_config_dir);
    dir.join("config.yaml").is_file()
}

fn default_config_dir() -> PathBuf {
    let mut d = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The merge had to be able to express *off*, not just *on*.
    ///
    /// With plain `bool` fields the old merge read `if project.x { project.x } else
    /// { global.x }`, so an explicit `false` at a higher layer was indistinguishable from
    /// an unset field and could never override a lower `true`. Hub-distributed policy was
    /// therefore one-way — fine for enabling a feature, useless for a control plane whose
    /// job includes switching one off.
    #[test]
    fn a_higher_layer_can_turn_a_feature_off() {
        let global = PrismConfig {
            graph_enabled: Some(true),
            cache_enabled: Some(true),
            ..PrismConfig::default()
        };
        let hub = PrismConfig {
            graph_enabled: Some(false),
            ..PrismConfig::default()
        };

        let merged = merge_config(&global, &hub);

        assert!(
            !merged.graph_enabled(),
            "hub `false` must win over global `true`"
        );
        assert!(
            merged.cache_enabled(),
            "a field the hub does not mention must keep the lower layer's value"
        );
    }

    #[test]
    fn unset_falls_through_to_the_documented_default() {
        let merged = merge_config(&PrismConfig::default(), &PrismConfig::default());

        assert_eq!(merged.toon_enabled, None, "default states nothing");
        assert!(merged.toon_enabled(), "TOON defaults on");
        assert!(!merged.tron_enabled(), "TRON defaults off");
        assert!(
            !merged.graph_enabled(),
            "GraphRAG defaults off — a hub feature"
        );
        assert!(
            !merged.cache_enabled(),
            "the semantic cache defaults off — a hub feature"
        );
    }

    /// Community *is* the default: an unlicensed agent has no hub policy, so both paid
    /// features resolve off with nothing on disk saying so.
    #[test]
    fn community_defaults_leave_both_paid_features_off() {
        let community = PrismConfig::default();

        assert!(!community.graph_enabled());
        assert!(!community.cache_enabled());
        // …and the two free flags are untouched by the gating change.
        assert!(community.toon_enabled(), "TOON stays on");
        assert!(!community.tron_enabled(), "TRON stays off");
    }

    /// The commercial mechanism, end to end through the merge: a hub layer that *states*
    /// `true` turns on a feature whose default is off. This is the direction the old
    /// truthiness merge could express and the `Option<bool>` merge must keep — the whole
    /// point of `policy_push` is distributing flags that change behaviour.
    #[test]
    fn a_hub_layer_enables_a_default_off_feature() {
        let local = PrismConfig::default();
        let hub_policy: PrismConfig =
            serde_yaml::from_str("graph_enabled: true\ncache_enabled: true\n")
                .expect("a hub policy document must parse");

        let merged = merge_config(&local, &hub_policy);

        assert!(
            merged.graph_enabled(),
            "hub policy must be able to enable GraphRAG"
        );
        assert!(
            merged.cache_enabled(),
            "hub policy must be able to enable the semantic cache"
        );
        // Turning one on must not turn the other on by accident.
        let graph_only: PrismConfig =
            serde_yaml::from_str("graph_enabled: true\n").expect("partial policy");
        let merged = merge_config(&local, &graph_only);
        assert!(merged.graph_enabled());
        assert!(
            !merged.cache_enabled(),
            "a feature the policy does not mention stays at its default"
        );
    }

    /// The message a gate reports has to name the feature, say *disabled* (not empty,
    /// not broken), carry the exact remedy, and not imply the rest of prism is crippled.
    #[test]
    fn the_disabled_message_names_the_feature_and_the_remedy() {
        let msg = disabled_by_policy("GraphRAG codebase intelligence");

        assert!(msg.contains("GraphRAG codebase intelligence"));
        assert!(msg.contains("disabled"), "{msg}");
        assert!(
            msg.contains("prism hub enroll --url <hub> --token <join-token>"),
            "the message must carry the command that fixes it: {msg}"
        );
        assert!(
            msg.contains("prism cmd") && msg.contains("Nothing else needs a hub"),
            "the message must say what still works: {msg}"
        );
        assert!(
            msg.contains("licensed hub"),
            "the message must name who enables it: {msg}"
        );
    }

    /// The hub sends only the keys a policy actually sets. Every absent field used to be
    /// a hard deserialization error, which is the second half of why `prism hub config`
    /// could never ingest a real response.
    #[test]
    fn a_partial_policy_document_parses() {
        let cfg: PrismConfig = serde_json::from_str(r#"{"graph_enabled":false}"#)
            .expect("a one-key policy must parse");

        assert_eq!(cfg.graph_enabled, Some(false));
        assert!(cfg.toon_enabled(), "unmentioned fields keep their defaults");
    }

    /// `PrismConfig` is snake_case on the wire, matching the `config.yaml` and `.prismrc`
    /// files users already have on disk. The hub's *envelope* is camelCase, so it is easy
    /// to assume the payload is too — and because `#[serde(default)]` ignores unknown
    /// keys, a camelCase policy key would be silently dropped rather than rejected. That
    /// is precisely the failure mode that hid the snake_case/camelCase telemetry bug for
    /// three months, so `known_field_names` exists to make it loud instead.
    /// The hub must be able to push per-tool filter toggles.
    ///
    /// `known_field_names` derives the legal key set from the *serialized* default, so
    /// a `skip_serializing_if` on this field would drop it from that set and prism
    /// would reject the policy with "unknown key" — the feature failing closed for a
    /// reason nobody could see from the hub.
    #[test]
    fn filter_toggles_is_a_known_wire_key() {
        let known = known_field_names();
        assert!(
            known.contains("filter_toggles"),
            "hub policy could not carry filter_toggles: {known:?}"
        );
        // snake_case on the wire, like every other policy key. The envelope around it
        // is camelCase, which is exactly how `graphEnabled` would silently no-op.
        assert!(!known.contains("filterToggles"));
    }

    /// Absent policy means every filter behaves as it always has.
    #[test]
    fn filters_are_enabled_unless_policy_says_otherwise() {
        let cfg = PrismConfig::default();
        for tool in ["git", "docker", "grep", "anything-at-all"] {
            assert!(
                cfg.filter_enabled_for(tool),
                "{tool} must filter by default"
            );
        }
    }

    /// Only an explicit `false` disables, and only for the named tool.
    #[test]
    fn a_policy_toggle_disables_exactly_one_tool() {
        let cfg = PrismConfig {
            filter_toggles: Some(
                [("docker".to_string(), false), ("git".to_string(), true)]
                    .into_iter()
                    .collect(),
            ),
            ..PrismConfig::default()
        };
        assert!(!cfg.filter_enabled_for("docker"));
        assert!(cfg.filter_enabled_for("git"));
        assert!(cfg.filter_enabled_for("grep"), "untouched tools stay on");
    }

    /// A project layer without toggles must not erase a hub policy that has them.
    ///
    /// `.or()`, not `unwrap_or_default()` — the same mistake that once made feature
    /// flags able to turn things on but never off.
    #[test]
    fn a_silent_layer_does_not_clear_hub_toggles() {
        let hub = PrismConfig {
            filter_toggles: Some([("docker".to_string(), false)].into_iter().collect()),
            ..PrismConfig::default()
        };
        let merged = merge_config(&hub, &PrismConfig::default());
        assert!(
            !merged.filter_enabled_for("docker"),
            "a project file with no toggles wiped the hub policy"
        );
    }

    #[test]
    fn camel_case_policy_keys_are_not_silently_accepted() {
        let cfg: PrismConfig =
            serde_json::from_str(r#"{"graphEnabled":false}"#).expect("unknown keys are ignored");
        assert_eq!(
            cfg.graph_enabled, None,
            "camelCase is NOT understood — which is why the hub path must reject it loudly"
        );

        let known = known_field_names();
        assert!(known.contains("graph_enabled"));
        assert!(!known.contains("graphEnabled"));
        assert!(known.contains("filters"), "the FilterLimits nesting key");
    }
}
