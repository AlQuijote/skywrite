//! Static musl builds of the on-device engine. transcribe-cpp-sys links the
//! C++ runtime as `dylib=stdc++`, which a static(-pie) musl binary cannot
//! load: it would start with an unresolvable `NEEDED libstdc++.so.6` and
//! crash before `main`. Put a directory holding only the static archive first
//! on the library search path, then resolve that archive's libc and libgcc
//! references after it (archives are searched once, in order). Other targets are
//! untouched. See CONTRIBUTING.md (static musl builds).
use std::path::PathBuf;

#[path = "build_cxx.rs"]
mod build_cxx;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_cxx.rs");
    let musl = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl");
    if !musl || std::env::var_os("CARGO_FEATURE_ONDEVICE").is_none() {
        return;
    }
    // The same C++ compiler and flags cc/cmake use for this target.
    let target = std::env::var("TARGET").expect("TARGET");
    let host = std::env::var("HOST").expect("HOST");
    for variable in build_cxx::variables(&target, &host) {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let (cxx, words) =
        build_cxx::compiler_and_flags(&target, &host, &|name| std::env::var(name).ok(), &|path| {
            std::path::Path::new(path).is_file()
        });
    let output = std::process::Command::new(&cxx)
        .args(&words)
        .arg("-print-file-name=libstdc++.a")
        .output()
        .unwrap_or_else(|error| panic!("running {cxx} to locate libstdc++.a: {error}"));
    let archive = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    assert!(
        archive.is_absolute() && archive.is_file(),
        "a static musl build with the on-device engine needs libstdc++.a from a musl C++ toolchain \
         ({cxx} -print-file-name=libstdc++.a printed {archive:?}); build in Alpine with g++, or \
         use --no-default-features"
    );
    let dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("static-cxx");
    std::fs::create_dir_all(&dir).expect("creating static-cxx directory");
    let link = dir.join("libstdc++.a");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&archive, &link).expect("linking libstdc++.a");
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-arg=-lc");
    // Rust links with -nodefaultlibs, and its compiler-builtins lack some
    // helpers libstdc++.a uses on aarch64 (outline atomics, 128-bit float).
    println!("cargo:rustc-link-arg=-lgcc");
}
