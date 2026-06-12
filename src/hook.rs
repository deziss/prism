//! Claude Code / shell hook registration for PRISM.

use anyhow::{Context, Result};

pub async fn install(global: bool) -> Result<()> {
    let shell_type = detect_shell()?;
    match shell_type.as_str() {
        "bash" => install_bash(global).await,
        "zsh" => install_zsh(global).await,
        _ => anyhow::bail!("Unsupported shell: {shell_type}"),
    }
}

async fn install_bash(global: bool) -> Result<()> {
    let rc_path = if global {
        dirs::home_dir().context("no home dir")?.join(".bashrc")
    } else {
        std::env::current_dir()?.join(".prismrc")
    };

    let hook_line = if global {
        format!("\n# PRISM hook (auto-generated)\n. \"{}/prism_hook.sh\"\n", prism_hook_dir())
    } else {
        format!(
            "\n# PRISM local hook (auto-generated)\n. \"{}/prism_hook.sh\"\n",
            std::env::current_dir()?.join(".prism").display()
        )
    };

    let content = std::fs::read_to_string(&rc_path).unwrap_or_default();
    if !content.contains("# PRISM hook") {
        std::fs::write(&rc_path, content + &hook_line)?;
    }
    write_hook_script(global).await
}

async fn install_zsh(global: bool) -> Result<()> {
    let rc_path = if global {
        dirs::home_dir().context("no home dir")?.join(".zshrc")
    } else {
        std::env::current_dir()?.join(".prismrc")
    };

    let hook_line = if global {
        format!("\n# PRISM hook (auto-generated)\n. \"{}/prism_hook.sh\"\n", prism_hook_dir())
    } else {
        format!(
            "\n# PRISM local hook (auto-generated)\n. \"{}/prism_hook.sh\"\n",
            std::env::current_dir()?.join(".prism").display()
        )
    };

    let content = std::fs::read_to_string(&rc_path).unwrap_or_default();
    if !content.contains("# PRISM hook") {
        std::fs::write(&rc_path, content + &hook_line)?;
    }
    write_hook_script(global).await
}

fn prism_hook_dir() -> String {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("prism")
        .join("hooks")
        .to_string_lossy()
        .to_string()
}

async fn write_hook_script(_global: bool) -> Result<()> {
    let hook_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("prism")
        .join("hooks");
    std::fs::create_dir_all(&hook_dir)?;

    let hook_path = hook_dir.join("prism_hook.sh");
    let script = format!(
        r#"#!/bin/bash
# PRISM hook script — auto-generated. DO NOT EDIT.
PRISM_HOOK() {{
  local cmd="$1"; shift
  case "$cmd" in
    git|cargo|pytest|py.test|grep|find|ls|tsc|eslint|docker|kubectl|psql|pnpm|npm|aws|gh|dotnet|jest|vitest|mypy|ruff|rake|rubocop|rspec|pip|next|lint|prism)
      exec prism "$cmd" "$@" ;;
    *) command "$cmd" "$@" ;;
  esac
}}
export PRISM_ENABLED=1
export PRISM_DATA_DIR="{}"
"#,
        hook_dir.display()
    );
    std::fs::write(&hook_path, &script)?;
    #[cfg(unix)]
    {
        std::process::Command::new("chmod")
            .args(["+x", &hook_path.to_string_lossy()])
            .output()
            .ok();
    }
    Ok(())
}

fn detect_shell() -> Result<String> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    if shell.contains("zsh") { Ok("zsh".into()) } else { Ok("bash".into()) }
}

pub async fn uninstall(global: bool) -> Result<()> {
    let shell = detect_shell()?;
    let rc_path = if global {
        dirs::home_dir()
            .context("no home dir")?
            .join(if shell == "zsh" { ".zshrc" } else { ".bashrc" })
    } else {
        std::env::current_dir()?.join(".prismrc")
    };
    if rc_path.exists() {
        let content = std::fs::read_to_string(&rc_path)?;
        let filtered = content
            .lines()
            .filter(|l| !l.contains("# PRISM hook"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&rc_path, filtered)?;
    }
    Ok(())
}
