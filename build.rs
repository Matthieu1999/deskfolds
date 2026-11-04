// Embeds the application manifest: common controls v6 (themed controls in the
// settings window), per-monitor DPI awareness v2 and UTF-8.
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("deskfolds")).expect("unable to embed the manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
