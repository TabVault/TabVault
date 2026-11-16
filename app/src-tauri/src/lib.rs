//! TabVault desktop app library entry point.
//!
//! The real UI and tray behavior are wired up in milestone M3; this is the
//! minimal boot path that proves the shell builds and opens the vault.

use std::path::PathBuf;

pub fn vault_path() -> Result<PathBuf, String> {
    if let Some(override_path) = std::env::var_os("TABVAULT_DB") {
        return Ok(PathBuf::from(override_path));
    }
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| "APPDATA is not set".to_string())?;
    Ok(PathBuf::from(appdata).join("TabVault").join("tabvault.db"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|_app| {
            // Open (and migrate) the vault at startup so problems surface at
            // boot rather than on first UI interaction.
            match vault_path().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)) {
                Ok(path) => {
                    if let Err(e) = tv_core::db::open_vault(&path) {
                        eprintln!("[tabvault] could not open the vault at {}: {e}", path.display());
                    }
                }
                Err(e) => eprintln!("[tabvault] vault path unavailable: {e}"),
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TabVault");
}
