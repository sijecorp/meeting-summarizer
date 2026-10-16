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
    sidecar();
    tauri_build::build()
}

/// The llama-server sidecar (bundle.externalBin, M6.5 plan 5.1, DX2, ENG14). tauri-build fails
/// when the file is missing, so a debug build makes an empty placeholder and warns: `cargo check`,
/// `cargo test` and rust-analyzer work in a fresh clone or worktree. A release or bundling build
/// fails unless the binary's stamp names the pinned llama.cpp commit and the current CMake options,
/// so neither a placeholder nor a binary left over from another branch is bundled.
fn sidecar() {
    use sha2::{Digest, Sha256};
    use std::fs;

    const REV: &str = "../scripts/llama.cpp.rev";
    const FLAGS: &str = "../scripts/llama-server.flags";
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let windows = target.contains("windows");
    let ext = if windows { ".exe" } else { "" };
    let bin = format!("binaries/llama-server-{target}{ext}");
    let stamp = format!("binaries/llama-server-{target}.stamp");
    for path in [bin.as_str(), stamp.as_str(), REV, FLAGS] {
        println!("cargo:rerun-if-changed={path}");
    }
    // `tauri build --debug` has PROFILE=debug but still bundles. is_dev() is false when tauri has the
    // custom-protocol feature, which `tauri build` turns on (tauri-cli interface/rust.rs) and
    // `tauri dev` and plain cargo do not.
    let strict = std::env::var("PROFILE").as_deref() == Ok("release") || !tauri_build::is_dev();

    // What the build scripts write: the commit line, and the first 12 hex digits of the flags
    // file's sha256.
    let rev = fs::read_to_string(REV).expect("read scripts/llama.cpp.rev");
    let rev = rev
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .expect("scripts/llama.cpp.rev has a commit line");
    let digest = Sha256::digest(fs::read(FLAGS).expect("read scripts/llama-server.flags"));
    let hash: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let expected = format!("rev={rev}\nflags={}", &hash[..12]);

    let problem = match fs::metadata(&bin) {
        Err(_) if strict => Some("is missing"),
        Err(_) => {
            fs::create_dir_all("binaries").expect("create src-tauri/binaries");
            fs::write(&bin, "").expect("write the llama-server placeholder");
            Some("was missing, so an empty placeholder stands in")
        }
        Ok(m) if m.len() == 0 => Some("is an empty placeholder"),
        Ok(_) => match fs::read_to_string(&stamp) {
            Err(_) => Some("has no stamp"),
            // replace: a stamp written on Windows may end its lines with \r\n.
            Ok(s) if s.replace('\r', "").trim() != expected => {
                Some("was built from another llama.cpp commit or other CMake options")
            }
            Ok(_) => None,
        },
    };
    if let Some(problem) = problem {
        let script = if windows {
            r"pwsh -File scripts\build-llama-server.ps1 (in the x64 MSVC developer environment)"
        } else {
            "scripts/build-llama-server.sh"
        };
        let msg = format!("src-tauri/{bin} {problem}. Build it from the repository root: {script}");
        if strict {
            panic!("{msg}");
        }
        println!("cargo:warning={msg}");
    }
}
