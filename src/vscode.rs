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
            "version": "0.1.0",
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
