#[cfg(target_os = "windows")]
fn main() {
    use embed_manifest::{embed_manifest, new_manifest};
    embed_manifest(
        new_manifest("stasis")
            .request_administrator()
            .done(),
    )
    .unwrap();
}

#[cfg(not(target_os = "windows"))]
fn main() {}
