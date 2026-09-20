fn main() {
    // `CARGO_CFG_WINDOWS` describes the *target*; `cfg!(target_os = "windows")`
    // inside a build script describes the *host*, which silently skipped the
    // manifest when cross-compiling from Linux.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    // Low-level input hooks run unelevated, so the executable must not ask for
    // administrator rights: a UAC prompt before every launch would be a worse
    // trade-off than the capability needs (see docs/design.md, Windows 提权).
    // The optional USB-storage capability would need its own elevation.
    use embed_manifest::manifest::ExecutionLevel;
    use embed_manifest::{embed_manifest, new_manifest};
    let manifest = new_manifest("stasis").requested_execution_level(ExecutionLevel::AsInvoker);
    embed_manifest(manifest).expect("unable to embed the Windows manifest");
    println!("cargo:rerun-if-changed=build.rs");
}
