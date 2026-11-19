// Embebe un manifiesto Win32: Common Controls v6 (EDIT con tema, tooltips),
// DPI per-monitor y longPathAware. Equivale al /MANIFEST:EMBED del build C++.
use embed_manifest::{embed_manifest, new_manifest};
use embed_manifest::manifest::{ActiveCodePage, DpiAwareness, Setting};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(
            new_manifest("OptimizedShell")
                .dpi_awareness(DpiAwareness::PerMonitorV2)
                .active_code_page(ActiveCodePage::Utf8)
                .long_path_aware(Setting::Enabled),
        )
        .expect("no se pudo embeber el manifiesto");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
