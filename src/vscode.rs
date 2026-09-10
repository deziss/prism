// PRISM vscode/extension.rs — VS Code extension scaffolding for PRISM CLI integration
// Generates the VS Code extension manifest and extension entry point

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// VS Code extension metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub publisher: String,
    pub engine: String,
    pub main: String,
    pub contributes: Value,
}

/// Extension configuration options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionConfig {
    pub prism_binary_path: String,
    pub log_level: String,
    pub telemetry_enabled: bool,
}

impl ExtensionManifest {
    /// Generate the full VS Code extension manifest JSON
    pub fn generate() -> String {
        let manifest = serde_json::json!({
            "name": "prism-cli",
            "displayName": "PRISM CLI — Token Optimizer Hub",
            "description": "PRISM CLI extension: TOON/TRON encoding, auto-filtering, semantic cache, Memory Palace, GraphRAG",
            "version": env!("CARGO_PKG_VERSION"),
            "publisher": "prism-team",
            "engines": {
                "vscode": "^1.85.0"
            },
            "main": "./out/extension.js",
            "contributes": {
                "commands": [
                    {
                        "command": "prism.init",
                        "title": "PRISM: Initialize Workspace"
                    },
                    {
                        "command": "prism.encode",
                        "title": "PRISM: Encode Selection (TOON/TRON)"
                    },
                    {
                        "command": "prism.filter",
                        "title": "PRISM: Filter Terminal Output"
                    },
                    {
                        "command": "prism.memory",
                        "title": "PRISM: Query Memory Palace"
                    },
                    {
                        "command": "prism.graph",
                        "title": "PRISM: Visualize Code Graph"
                    },
                    {
                        "command": "prism.count",
                        "title": "PRISM: Count Tokens in File"
                    },
                    {
                        "command": "prism.serve",
                        "title": "PRISM: Start Proxy Server"
                    },
                    {
                        "command": "prism.mcp",
                        "title": "PRISM: Start MCP Server"
                    },
                    {
                        "command": "prism.config",
                        "title": "PRISM: Open Configuration"
                    },
                    {
                        "command": "prism.adopt",
                        "title": "PRISM: Adopt All Workspace Files"
                    },
                    {
                        "command": "prism.discover",
                        "title": "PRISM: Discover Workspace Structure"
                    },
                    {
                        "command": "prism.gain",
                        "title": "PRISM: Gain Project Context"
                    },
                    {
                        "command": "prism.toon",
                        "title": "PRISM: Encode as TOON"
                    },
                    {
                        "command": "prism.tron",
                        "title": "PRISM: Encode as TRON"
                    }
                ],
                "configuration": {
                    "title": "PRISM",
                    "properties": {
                        "prism.binary": {
                            "type": "string",
                            "default": "",
                            "description": "Path to prism binary (empty = auto-detect)"
                        },
                        "prism.logLevel": {
                            "type": "string",
                            "enum": ["debug", "info", "warn", "error"],
                            "default": "info"
                        },
                        "prism.autoFilter": {
                            "type": "boolean",
                            "default": true,
                            "description": "Auto-filter terminal output"
                        },
                        "prism.cacheEnabled": {
                            "type": "boolean",
                            "default": true,
                            "description": "Enable semantic cache"
                        }
                    }
                }
            },
            "activationEvents": ["onStartupFinished", "onCommand:prism.*"]
        });

        serde_json::to_string_pretty(&manifest).unwrap_or_default()
    }

    /// Generate the VS Code extension entry point JavaScript
    pub fn generate_extension_js() -> String {
        r#"// PRISM VS Code Extension — entry point
const vscode = require('vscode');
const { spawn } = require('child_process');
const path = require('path');

let prismBin = '';
let context = null;

function activate(ctx) {
    context = ctx;

    // Register all PRISM commands
    const commands = [
        'prism.init', 'prism.encode', 'prism.filter',
        'prism.memory', 'prism.graph', 'prism.count',
        'prism.serve', 'prism.mcp', 'prism.config',
        'prism.adopt', 'prism.discover', 'prism.gain',
        'prism.toon', 'prism.tron'
    ];

    commands.forEach(cmd => {
        ctx.subscriptions.push(
            vscode.commands.registerCommand(cmd, async (...args) => {
                prismBin = await findPrism();
                if (!prismBin) {
                    vscode.window.showErrorMessage('PRISM binary not found');
                    return;
                }
                return runPrism(cmd.replace('prism.', ''), args);
            })
        );
    });

    vscode.window.showInformationMessage('PRISM CLI extension activated');
}

async function findPrism() {
    // Priority: config > PATH > workspace
    const config = vscode.workspace.getConfiguration('prism');
    if (config.get('binary')) return config.get('binary');
    return 'prism';
}

async function runPrism(subcommand, args) {
    return new Promise((resolve, reject) => {
        const proc = spawn(prismBin, [subcommand, ...args]);
        let output = '';
        let error = '';
        proc.stdout.on('data', d => output += d.toString());
        proc.stderr.on('data', d => error += d.toString());
        proc.on('close', code => {
            if (error) vscode.window.showErrorMessage('PRISM: ' + error);
            if (output) vscode.window.showInformationMessage(output.trim());
            resolve(output.trim());
        });
    });
}

function deactivate() {}

module.exports = { activate, deactivate };
"#
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::ExtensionManifest;

    /// `Cargo.toml`'s `package.version` is the single source of truth for the
    /// crate version. Every runtime version string must derive from it via
    /// `env!("CARGO_PKG_VERSION")` rather than repeating a literal, so a
    /// `cargo release`-style bump cannot leave a stale number behind in a
    /// generated manifest, a banner, or a health probe.
    ///
    /// This is the same failure mode as the snake_case/camelCase telemetry
    /// mismatch this release fixes: a value duplicated by hand drifts silently.
    /// Pinning it in a test is what makes the invariant hold.
    #[test]
    fn generated_manifest_version_tracks_crate_version() {
        let manifest: serde_json::Value = serde_json::from_str(&ExtensionManifest::generate())
            .expect("manifest must be valid JSON");

        assert_eq!(
            manifest["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION")),
            "the generated VS Code manifest must report the crate version, not a hard-coded literal"
        );
    }

    /// The human-facing banners and both `/health` probes are the other places a
    /// literal used to live. Guard the whole set at once: no source file outside
    /// the filter/reader test fixtures may contain a bare `v?<version>` string.
    ///
    /// Fixtures are excluded deliberately — they are captured `cargo`/`npm`
    /// output used as parser input, so their version numbers are data, not this
    /// crate's identity, and must stay literal.
    #[test]
    fn no_source_file_hardcodes_the_crate_version() {
        let version = env!("CARGO_PKG_VERSION");
        let mut offenders = Vec::new();

        for entry in walkdir::WalkDir::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
        {
            let path = entry.path();
            // Captured tool output used as parser input — version numbers here
            // are test data, not our own version.
            let rel = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(path);
            let rel = rel.to_string_lossy();
            if rel.contains("filter/") || rel.ends_with("reader.rs") {
                continue;
            }

            let src = std::fs::read_to_string(path).unwrap_or_default();
            for (i, line) in src.lines().enumerate() {
                // Skip the assertion lines in this very test.
                if line.contains("CARGO_PKG_VERSION") {
                    continue;
                }
                if line.contains(&format!("\"{version}\"")) || line.contains(&format!("v{version}"))
                {
                    offenders.push(format!("{rel}:{}", i + 1));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "these lines hard-code the crate version instead of using \
             env!(\"CARGO_PKG_VERSION\"): {offenders:?}"
        );
    }
}
