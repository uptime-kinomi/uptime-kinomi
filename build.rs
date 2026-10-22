fn main() {
    // The target triple selects the release asset for self-update and is
    // shown by `version`. Cargo always sets TARGET for build scripts.
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=KINOMI_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
