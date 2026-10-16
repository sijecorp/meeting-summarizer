fn main() {
    // ggml-metal guards newer Metal APIs with `@available`. `tauri build` sets the deployment target
    // to bundle.macOS.minimumSystemVersion, older than the SDK, so clang lowers those checks to
    // `__isPlatformVersionAtLeast` from libclang_rt.osx.a, which rustc does not link
    // (`-nodefaultlibs`). Plain `cargo build` targets the host macOS and never needs it.
    // Same fix as Handy's build.rs (https://github.com/cjpais/Handy, 5197faf, MIT).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // xcrun picks the toolchain from DEVELOPER_DIR (or xcode-select).
        println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
        let out = std::process::Command::new("xcrun")
            .args(["clang", "--print-resource-dir"])
            .output()
            .expect("run `xcrun clang --print-resource-dir` to locate libclang_rt.osx.a");
        assert!(
            out.status.success(),
            "`xcrun clang --print-resource-dir` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        let resource_dir = String::from_utf8(out.stdout).expect("clang resource dir is UTF-8");
        let darwin = std::path::Path::new(resource_dir.trim()).join("lib/darwin");
        assert!(
            darwin.join("libclang_rt.osx.a").exists(),
            "libclang_rt.osx.a not found in {}",
            darwin.display()
        );
        println!("cargo:rustc-link-search=native={}", darwin.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
    tauri_build::build()
}
