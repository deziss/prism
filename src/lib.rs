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

/// Test-only: serialise every test that redirects the store, and restore the
/// environment afterwards.
///
/// `PRISM_DATA_DIR` and `PRISM_SPOOL_MAX_BYTES` are process-global, but `hub` and
/// `analytics` each guarded them with their *own* mutex, and `proxy` mutated them
/// under none at all while claiming "single-threaded env mutation" — which is not
/// true of `cargo test`. When the race landed, `prism_data_dir()` fell back to the
/// user's real store and a test wrote fake `tool_N` events straight into the live
/// `hub_spool.jsonl`, which then shipped to the hub. Worse, a test's
/// `PRISM_SPOOL_MAX_BYTES=1000` applied to the real spool, whose cap logic would trim
/// it to a few hundred bytes — silently destroying un-shipped telemetry.
///
/// One lock, and a guard that restores on drop so a panicking test cannot leak its
/// override into whatever runs next.
#[cfg(test)]
pub mod test_env {
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());

    const VARS: &[&str] = &["PRISM_DATA_DIR", "PRISM_SPOOL_MAX_BYTES"];

    pub struct TestEnv {
        _guard: MutexGuard<'static, ()>,
        prior: Vec<(&'static str, Option<OsString>)>,
    }

    impl TestEnv {
        /// Point the store at `dir` for as long as the returned guard lives.
        pub fn redirect(dir: &Path) -> Self {
            let guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let prior = VARS.iter().map(|v| (*v, std::env::var_os(v))).collect();
            // SAFETY: LOCK is held, so no other test in this process is reading or
            // writing these variables, and `prior` restores them on drop.
            unsafe { std::env::set_var("PRISM_DATA_DIR", dir) };
            Self {
                _guard: guard,
                prior,
            }
        }

        /// Cap the hub spool, for tests that exercise the drop-oldest path.
        pub fn spool_max_bytes(self, bytes: u64) -> Self {
            // SAFETY: LOCK is held for the lifetime of `self`.
            unsafe { std::env::set_var("PRISM_SPOOL_MAX_BYTES", bytes.to_string()) };
            self
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (var, value) in &self.prior {
                // SAFETY: LOCK is still held until `_guard` drops after this.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(var, v),
                        None => std::env::remove_var(var),
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The guard must leave the environment exactly as it found it.
        ///
        /// The old tests set `PRISM_DATA_DIR` and removed it by hand at the end of the
        /// body, so any early return or panic leaked the override — and a leaked
        /// `PRISM_SPOOL_MAX_BYTES=1000` applies its cap to the user's real spool.
        fn unique_dir(tag: &str) -> std::path::PathBuf {
            std::env::temp_dir().join(format!(
                "prism-test-env-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
            ))
        }

        #[test]
        fn restores_the_environment_on_drop() {
            // Deliberately NOT compared against a snapshot taken before the guard is
            // acquired: that read races other tests, which is how the first version of
            // this test failed. The property that matters is that our override is gone.
            let dir = unique_dir("restore");
            {
                let _env = TestEnv::redirect(&dir).spool_max_bytes(1000);
                assert_eq!(crate::prism_data_dir(), dir);
                assert_eq!(
                    std::env::var("PRISM_SPOOL_MAX_BYTES").ok().as_deref(),
                    Some("1000")
                );
            }
            assert_ne!(
                crate::prism_data_dir(),
                dir,
                "the redirect outlived its guard"
            );
            assert_ne!(
                std::env::var("PRISM_SPOOL_MAX_BYTES").ok().as_deref(),
                Some("1000"),
                "a leaked spool cap would truncate the real hub spool"
            );
        }

        /// And it must restore even when the test body panics.
        #[test]
        fn restores_the_environment_after_a_panic() {
            let dir = unique_dir("panic");
            let probe = dir.clone();
            let result = std::panic::catch_unwind(move || {
                let _env = TestEnv::redirect(&probe);
                panic!("boom");
            });
            assert!(result.is_err());
            assert_ne!(
                crate::prism_data_dir(),
                dir,
                "a panicking test leaked its redirect into the rest of the suite"
            );
        }
    }
}
