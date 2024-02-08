use directories::BaseDirs;
use tokio::fs;
use tracing::{info, warn};

#[cfg(target_os = "linux")]
const EXTRA_DIRS: [&str; 1] = [".cache/spacedrive"];
#[cfg(target_os = "macos")]
const EXTRA_DIRS: [&str; 2] = ["Library/WebKit/Spacedrive", "Library/Caches/Spacedrive"];

pub async fn clear_localstorage() {
	if let Some(base_dir) = BaseDirs::new() {
		let data_dir = base_dir.data_dir().join("com.spacedrive.desktop"); // maybe tie this into something static?

		fs::remove_dir_all(&data_dir)
			.await
			.map_err(|_| warn!("Unable to delete the `localStorage` primary directory."))
			.ok();

		// Windows needs both AppData/Local and AppData/Roaming clearing as it stores data in both
		#[cfg(target_os = "windows")]
		fs::remove_dir_all(&base_dir.data_local_dir().join("com.spacedrive.desktop"))
			.await
			.map_err(|_| warn!("Unable to delete the `localStorage` directory in Local AppData."))
			.ok();

		info!("Deleted {}", data_dir.display());

		let home_dir = base_dir.home_dir();

		#[cfg(any(target_os = "linux", target_os = "macos"))]
		for path in EXTRA_DIRS {
			fs::remove_dir_all(home_dir.join(path))
				.await
				.map_err(|_| warn!("Unable to delete a `localStorage` cache: {path}"))
				.ok();

			info!("Deleted {path}");
		}

		info!("Successfully wiped `localStorage` and related caches.")
	} else {
		warn!("Unable to source `BaseDirs` in order to clear `localStorage`.")
	}
}
