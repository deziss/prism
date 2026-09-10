// PRISM build.rs — portable BLAS/CBLAS linker setup for turbovec
// turbovec -> ndarray(blas) -> cblas-sys -> needs cblas_sgemm at link time.
// openblas-src emits cargo:rustc-link-lib=openblas but may not set search path.
// This build.rs emits both search path AND link lib, which only affects the final
// binary (not build script binaries), avoiding the chicken-and-egg problem.
use std::{env, fs, path::PathBuf};

fn main() {
    // 1. pkg-config openblas — works on servers with libopenblas-dev
    if try_pkg_config("openblas") {
        return;
    }
    if try_pkg_config("cblas") {
        return;
    }

    // 2. Fallback: create .blas-link/libopenblas.so symlink → libgslcblas / libblas
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let blas_dir = manifest_dir.join(".blas-link");
    fs::create_dir_all(&blas_dir).ok();

    let link_so = blas_dir.join("libopenblas.so");
    if !link_so.exists() {
        let candidates = [
            "/usr/lib/x86_64-linux-gnu/libgslcblas.so.0",
            "/usr/lib/aarch64-linux-gnu/libgslcblas.so.0",
            "/usr/lib/x86_64-linux-gnu/libblas.so.3",
            "/usr/lib64/libgslcblas.so.0",
            "/usr/local/lib/libgslcblas.so",
        ];
        for candidate in &candidates {
            if std::path::Path::new(candidate).exists() {
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(candidate, &link_so).ok();
                }
                break;
            }
        }
    }

    if link_so.exists() {
        // Both flags here propagate to the final binary only, not build scripts.
        println!("cargo:rustc-link-search=native={}", blas_dir.display());
        println!("cargo:rustc-link-lib=openblas");
    } else {
        println!("cargo:warning=PRISM: BLAS library not found. turbovec requires CBLAS.");
        println!("cargo:warning=  Ubuntu/Debian: sudo apt-get install libopenblas-dev");
        println!("cargo:warning=  Fedora/RHEL:   sudo dnf install openblas-devel");
        println!("cargo:warning=  macOS:         brew install openblas");
        println!("cargo:warning=  Also works:    sudo apt-get install libgsl-dev");
    }
}

fn try_pkg_config(lib: &str) -> bool {
    let out = std::process::Command::new("pkg-config")
        .args(["--libs-only-L", "--libs-only-l", lib])
        .output();
    if let Ok(out) = out {
        if out.status.success() {
            let flags = String::from_utf8_lossy(&out.stdout);
            for flag in flags.split_whitespace() {
                if let Some(path) = flag.strip_prefix("-L") {
                    println!("cargo:rustc-link-search=native={path}");
                } else if let Some(lib) = flag.strip_prefix("-l") {
                    println!("cargo:rustc-link-lib={lib}");
                }
            }
            return true;
        }
    }
    false
}
