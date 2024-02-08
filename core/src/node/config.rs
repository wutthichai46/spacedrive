use crate::{
	api::{notifications::Notification, BackendFeature},
	object::media::thumbnail::preferences::ThumbnailerPreferences,
	util::version_manager::{Kind, ManagedVersion, VersionManager, VersionManagerError},
};

use sd_p2p::{Keypair, ManagerConfig};
use sd_utils::error::FileIOError;

use std::{
	path::{Path, PathBuf},
	sync::Arc,
};

use int_enum::IntEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use serde_repr::{Deserialize_repr, Serialize_repr};
use specta::Type;
use thiserror::Error;
use tokio::{
	fs,
	sync::{watch, RwLock},
};
use tracing::error;
use uuid::Uuid;

/// NODE_STATE_CONFIG_NAME is the name of the file which stores the NodeState
pub const NODE_STATE_CONFIG_NAME: &str = "node_state.sdconfig";

/// NodeConfig is the configuration for a node. This is shared between all libraries and is stored in a JSON file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)] // If you are adding `specta::Type` on this your probably about to leak the P2P private key
pub struct NodeConfig {
	/// id is a unique identifier for the current node. Each node has a public identifier (this one) and is given a local id for each library (done within the library code).
	pub id: Uuid,
	/// name is the display name of the current node. This is set by the user and is shown in the UI. // TODO: Length validation so it can fit in DNS record
	pub name: String,
	/// core level notifications
	#[serde(default)]
	pub notifications: Vec<Notification>,
	/// The p2p identity keypair for this node. This is used to identify the node on the network.
	/// This keypair does effectively nothing except for provide libp2p with a stable peer_id.
	pub keypair: Keypair,
	/// P2P config
	#[serde(default)]
	pub p2p: ManagerConfig,
	/// Feature flags enabled on the node
	#[serde(default)]
	pub features: Vec<BackendFeature>,
	/// Authentication for Spacedrive Accounts
	pub auth_token: Option<sd_cloud_api::auth::OAuthToken>,
	/// URL of the Spacedrive API
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sd_api_origin: Option<String>,
	/// The aggreagation of many different preferences for the node
	pub preferences: NodePreferences,
	// Model version for the image labeler
	pub image_labeler_version: Option<String>,

	version: NodeConfigVersion,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Type)]
pub struct NodePreferences {
	pub thumbnailer: ThumbnailerPreferences,
}

#[derive(
	IntEnum, Debug, Clone, Copy, Eq, PartialEq, strum::Display, Serialize_repr, Deserialize_repr,
)]
#[repr(u64)]
pub enum NodeConfigVersion {
	V0 = 0,
	V1 = 1,
	V2 = 2,
}

impl ManagedVersion<NodeConfigVersion> for NodeConfig {
	const LATEST_VERSION: NodeConfigVersion = NodeConfigVersion::V2;
	const KIND: Kind = Kind::Json("version");
	type MigrationError = NodeConfigError;

	fn from_latest_version() -> Option<Self> {
		let mut name = match hostname::get() {
			// SAFETY: This is just for display purposes so it doesn't matter if it's lossy
			Ok(hostname) => hostname.to_string_lossy().into_owned(),
			Err(e) => {
				error!("Falling back to default node name as an error occurred getting your systems hostname: '{e:#?}'");
				"my-spacedrive".into()
			}
		};
		name.truncate(250);

		#[cfg(feature = "ai")]
		let image_labeler_version = Some(sd_ai::image_labeler::DEFAULT_MODEL_VERSION.to_string());
		#[cfg(not(feature = "ai"))]
		let image_labeler_version = None;

		Some(Self {
			id: Uuid::new_v4(),
			name,
			keypair: Keypair::generate(),
			version: Self::LATEST_VERSION,
			p2p: ManagerConfig::default(),
			features: vec![],
			notifications: vec![],
			auth_token: None,
			sd_api_origin: None,
			preferences: NodePreferences::default(),
			image_labeler_version,
		})
	}
}

impl NodeConfig {
	pub async fn load(path: impl AsRef<Path>) -> Result<Self, NodeConfigError> {
		let path = path.as_ref();
		VersionManager::<Self, NodeConfigVersion>::migrate_and_load(
			path,
			|current, next| async move {
				match (current, next) {
					(NodeConfigVersion::V0, NodeConfigVersion::V1) => {
						let mut config: Map<String, Value> =
							serde_json::from_slice(&fs::read(path).await.map_err(|e| {
								FileIOError::from((
									path,
									e,
									"Failed to read node config file for migration",
								))
							})?)
							.map_err(VersionManagerError::SerdeJson)?;

						// All were never hooked up to the UI
						config.remove("p2p_email");
						config.remove("p2p_img_url");
						config.remove("p2p_port");

						// In a recent PR I screwed up Serde `default` so P2P was disabled by default, prior it was always enabled.
						// Given the config for it is behind a feature flag (so no one would have changed it) this fixes the default.
						if let Some(Value::Object(obj)) = config.get_mut("p2p") {
							obj.insert("enabled".into(), Value::Bool(true));
						}

						fs::write(
							path,
							serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?,
						)
						.await
						.map_err(|e| FileIOError::from((path, e)))?;
					}

					(NodeConfigVersion::V1, NodeConfigVersion::V2) => {
						let mut config: Map<String, Value> =
							serde_json::from_slice(&fs::read(path).await.map_err(|e| {
								FileIOError::from((
									path,
									e,
									"Failed to read node config file for migration",
								))
							})?)
							.map_err(VersionManagerError::SerdeJson)?;

						config.insert(
							String::from("preferences"),
							json!(NodePreferences::default()),
						);

						let a =
							serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?;

						fs::write(path, a)
							.await
							.map_err(|e| FileIOError::from((path, e)))?;
					}

					_ => {
						error!("Node config version is not handled: {:?}", current);
						return Err(VersionManagerError::UnexpectedMigration {
							current_version: current.int_value(),
							next_version: next.int_value(),
						}
						.into());
					}
				}

				Ok(())
			},
		)
		.await
	}

	async fn save(&self, path: impl AsRef<Path>) -> Result<(), NodeConfigError> {
		let path = path.as_ref();
		fs::write(path, serde_json::to_vec(self)?)
			.await
			.map_err(|e| FileIOError::from((path, e)))?;

		Ok(())
	}
}

pub struct Manager {
	config: RwLock<NodeConfig>,
	data_directory_path: PathBuf,
	config_file_path: PathBuf,
	preferences_watcher_tx: watch::Sender<NodePreferences>,
}

impl Manager {
	/// new will create a new NodeConfigManager with the given path to the config file.
	pub(crate) async fn new(
		data_directory_path: impl AsRef<Path>,
	) -> Result<Arc<Self>, NodeConfigError> {
		let data_directory_path = data_directory_path.as_ref().to_path_buf();
		let config_file_path = data_directory_path.join(NODE_STATE_CONFIG_NAME);

		let mut config = NodeConfig::load(&config_file_path).await?;

		#[cfg(feature = "ai")]
		if config.image_labeler_version.is_none() {
			config.image_labeler_version =
				Some(sd_ai::image_labeler::DEFAULT_MODEL_VERSION.to_string());
		}

		#[cfg(not(feature = "ai"))]
		{
			config.image_labeler_version = None;
		}

		let (preferences_watcher_tx, _preferences_watcher_rx) =
			watch::channel(config.preferences.clone());

		Ok(Arc::new(Self {
			config: RwLock::new(config),
			data_directory_path,
			config_file_path,
			preferences_watcher_tx,
		}))
	}

	/// get will return the current NodeConfig in a read only state.
	pub(crate) async fn get(&self) -> NodeConfig {
		self.config.read().await.clone()
	}

	/// get a node config preferences watcher receiver
	pub(crate) fn preferences_watcher(&self) -> watch::Receiver<NodePreferences> {
		self.preferences_watcher_tx.subscribe()
	}

	/// data_directory returns the path to the directory storing the configuration data.
	pub(crate) fn data_directory(&self) -> PathBuf {
		self.data_directory_path.clone()
	}

	/// write allows the user to update the configuration. This is done in a closure while a Mutex lock is held so that the user can't cause a race condition if the config were to be updated in multiple parts of the app at the same time.
	pub(crate) async fn write<F: FnOnce(&mut NodeConfig)>(
		&self,
		mutation_fn: F,
	) -> Result<NodeConfig, NodeConfigError> {
		let mut config = self.config.write().await;

		mutation_fn(&mut config);

		self.preferences_watcher_tx.send_if_modified(|current| {
			let modified = current != &config.preferences;
			if modified {
				*current = config.preferences.clone();
			}
			modified
		});

		config
			.save(&self.config_file_path)
			.await
			.map(|()| config.clone())
	}

	/// update_preferences allows the user to update the preferences of the node
	pub(crate) async fn update_preferences(
		&self,
		update_fn: impl FnOnce(&mut NodePreferences),
	) -> Result<(), NodeConfigError> {
		let mut config = self.config.write().await;

		update_fn(&mut config.preferences);

		self.preferences_watcher_tx
			.send_replace(config.preferences.clone());

		config.save(&self.config_file_path).await
	}
}

#[derive(Error, Debug)]
pub enum NodeConfigError {
	#[error(transparent)]
	SerdeJson(#[from] serde_json::Error),
	#[error(transparent)]
	VersionManager(#[from] VersionManagerError<NodeConfigVersion>),
	#[error(transparent)]
	FileIO(#[from] FileIOError),
}
