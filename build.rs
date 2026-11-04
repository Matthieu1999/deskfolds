// Embeds the application manifest: common controls v6 (themed controls in the
// settings window), per-monitor DPI awareness v2 and UTF-8.
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("deskfolds")).expect("unable to embed the manifest");
        // The DLLs deskfolds links to are loaded from System32 only, never from
        // its own folder, where a file of the same name could have been put.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
