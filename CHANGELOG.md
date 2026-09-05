# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-09-05

### Added
- **Anthropic Prompt Cache Invariant Normalization**:
  - Automatic TTL non-increasing order enforcement: automatically detects if a `ttl='1h'` cache breakpoint follows any `ttl='5m'` (or default ephemeral) block across the evaluation sequence (`tools` -> `system` -> `messages`), promoting preceding blocks to `1h` to eliminate Anthropic 400 Bad Request errors.
  - Breakpoint cap enforcement: strictly caps active cache breakpoints at Anthropic's maximum of 4 across tools, system, and messages, safely pruning excess breakpoints.
  - Diagnostic tracing: added structured `warn!` and `info!` logs when breakpoints are capped, promoted, or skipped.
- **Client Cache Strategy Awareness**:
  - Detects if upstream clients or IDE tools (such as Claude Code, Cursor, or custom SDK clients) have already configured their own `cache_control` blocks. PRISM preserves caller strategies without injecting redundant duplicate breakpoints.
- **Systemd User Service Integration**:
  - Packaged and enabled `prism-proxy.service` running on `:8081` with auto-restart policies and journald logging integration.
- **Comprehensive Unit Test Suite**:
  - Added unit test coverage for Anthropic cache TTL promotion, caller cache preservation, and 4-breakpoint limit invariants (48 total tests passing).

### Changed
- **Licensing**: Re-licensed the project under the GNU Affero General Public License v3.0 (`AGPL-3.0-only`), updating all metadata, Cargo manifests, and documentation.
- **Benchmark Suite**: Updated benchmark suite runner (`scripts/benchmark.sh`) to dynamically adapt to available sample files and avoid hardcoded document path dependencies.
- **Project Branding**: Standardized project nomenclature and expansion to *PRISM — Prompt Reduction, Indexing & Semantic Memory*.

### Fixed
- **Anthropic 400 Error in Long Sessions**: Fixed `a ttl='1h' cache_control block must not come after a ttl='5m' cache_control block` triggered during multi-turn agent sessions in Claude Code and VS Code.
- **MITM Proxy Streaming & TLS Interception**: Fixed HTTP CONNECT tunnel socket polling and TLS certificate generation to prevent handshake failures and connection drops during long-lived SSE streams.

### Security & Privacy
- Updated `.gitignore` to prevent tracking private design documents, agent prompt instructions (`AGENTS.md`, `PRISM_PLAN.md`), and local graph indexing exclusion configs (`.graphifyignore`).
