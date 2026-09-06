fn main() {
    #[cfg(feature = "tauri-desktop")]
    tauri_build::build();
}
