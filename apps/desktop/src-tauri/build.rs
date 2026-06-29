//! Build script for the Byte Agent desktop application.
#![deny(rustdoc::broken_intra_doc_links)]

/// Generates Tauri mobile/ desktop assets and resource metadata.
fn main() {
    tauri_build::build();
}
