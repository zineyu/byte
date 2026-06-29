//! Byte desktop application entry point.
#![deny(rustdoc::broken_intra_doc_links)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    byte_desktop_lib::run();
}
