use crate::{
	node::{config::NodeConfig, Platform},
	util::version_manager::{Kind, ManagedVersion, VersionManager, VersionManagerError},
};

use sd_p2p::spacetunnel::{Identity, IdentityOrRemoteIdentity};
use sd_prisma::prisma::{file_path, indexer_rule, instance, location, node, PrismaClient};
use sd_utils::{db::maybe_missing, error::FileIOError};

use std::path::Path;

use chrono::Utc;
use int_enum::IntEnum;
use prisma_client_rust::not;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use serde_repr::{Deserialize_repr, Serialize_repr};
use specta::Type;
use thiserror::Error;
use tokio::fs;
use tracing::error;
use uuid::Uuid;

use super::name::LibraryName;

/// LibraryConfig holds the configuration for a specific library. This is stored as a '{uuid}.sdlibrary' file.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LibraryConfig {
	/// name is the display name of the library. This is used in the UI and is set by the user.
	pub name: LibraryName,
	/// description is a user set description of the library. This is used in the UI and is set by the user.
	pub description: Option<String>,
	/// id of the current instance so we know who this `.db` is. This can be looked up within the `Instance` table.
	pub instance_id: i32,
	/// cloud_id is the ID of the cloud library this library is linked to.
	/// If this is set we can assume the library is synced with the Cloud.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cloud_id: Option<String>,
	version: LibraryConfigVersion,
}

#[derive(
	IntEnum,
	Debug,
	Clone,
	Copy,
	Eq,
	PartialEq,
	strum::Display,
	Serialize_repr,
	Deserialize_repr,
	Type,
)]
#[repr(u64)]
pub enum LibraryConfigVersion {
	V0 = 0,
	V1 = 1,
	V2 = 2,
	V3 = 3,
	V4 = 4,
	V5 = 5,
	V6 = 6,
	V7 = 7,
	V8 = 8,
	V9 = 9,
}

impl ManagedVersion<LibraryConfigVersion> for LibraryConfig {
	const LATEST_VERSION: LibraryConfigVersion = LibraryConfigVersion::V9;

	const KIND: Kind = Kind::Json("version");

	type MigrationError = LibraryConfigError;
}

impl LibraryConfig {
	pub(crate) async fn new(
		name: LibraryName,
		description: Option<String>,
		instance_id: i32,
		path: impl AsRef<Path>,
	) -> Result<Self, LibraryConfigError> {
		let this = Self {
			name,
			description,
			instance_id,
			version: Self::LATEST_VERSION,
			cloud_id: None,
		};

		this.save(path).await.map(|()| this)
	}

	pub(crate) async fn load(
		path: impl AsRef<Path>,
		node_config: &NodeConfig,
		db: &PrismaClient,
	) -> Result<Self, LibraryConfigError> {
		let path = path.as_ref();

		VersionManager::<Self, LibraryConfigVersion>::migrate_and_load(
			path,
			|current, next| async move {
				match (current, next) {
					(LibraryConfigVersion::V0, LibraryConfigVersion::V1) => {
						let rules = vec![
							String::from("No OS protected"),
							String::from("No Hidden"),
							String::from("No Git"),
							String::from("Only Images"),
						];

						db._batch(
							rules
								.into_iter()
								.enumerate()
								.map(|(i, name)| {
									db.indexer_rule().update_many(
										vec![indexer_rule::name::equals(Some(name))],
										vec![indexer_rule::pub_id::set(sd_utils::uuid_to_bytes(
											Uuid::from_u128(i as u128),
										))],
									)
								})
								.collect::<Vec<_>>(),
						)
						.await?;
					}

					(LibraryConfigVersion::V1, LibraryConfigVersion::V2) => {
						let mut config = serde_json::from_slice::<Map<String, Value>>(
							&fs::read(path).await.map_err(|e| {
								VersionManagerError::FileIO(FileIOError::from((path, e)))
							})?,
						)
						.map_err(VersionManagerError::SerdeJson)?;

						config.insert(
							String::from("identity"),
							Value::Array(
								Identity::new()
									.to_bytes()
									.into_iter()
									.map(Into::into)
									.collect(),
							),
						);

						fs::write(
							path,
							&serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?,
						)
						.await
						.map_err(|e| VersionManagerError::FileIO(FileIOError::from((path, e))))?;
					}

					(LibraryConfigVersion::V2, LibraryConfigVersion::V3) => {
						// The fact I have to migrate this hurts my soul
						if db.node().count(vec![]).exec().await? != 1 {
							return Err(LibraryConfigError::TooManyNodes);
						}

						db.node()
							.update_many(
								vec![],
								vec![
									node::pub_id::set(node_config.id.as_bytes().to_vec()),
									node::node_peer_id::set(Some(
										node_config.keypair.peer_id().to_string(),
									)),
								],
							)
							.exec()
							.await?;

						let mut config = serde_json::from_slice::<Map<String, Value>>(
							&fs::read(path).await.map_err(|e| {
								VersionManagerError::FileIO(FileIOError::from((path, e)))
							})?,
						)
						.map_err(VersionManagerError::SerdeJson)?;

						config.insert(String::from("node_id"), json!(node_config.id.to_string()));

						fs::write(
							path,
							&serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?,
						)
						.await
						.map_err(|e| VersionManagerError::FileIO(FileIOError::from((path, e))))?;
					}

					(LibraryConfigVersion::V3, LibraryConfigVersion::V4) => {
						// -_-
					}

					(LibraryConfigVersion::V4, LibraryConfigVersion::V5) => loop {
						let paths = db
							.file_path()
							.find_many(vec![not![file_path::size_in_bytes::equals(None)]])
							.take(500)
							.select(file_path::select!({ id size_in_bytes }))
							.exec()
							.await?;

						if paths.is_empty() {
							break;
						}

						db._batch(
							paths
								.into_iter()
								.filter_map(|path| {
									maybe_missing(path.size_in_bytes, "file_path.size_in_bytes")
										.map_or_else(
											|e| {
												error!("{e:#?}");
												None
											},
											Some,
										)
										.map(|size_in_bytes| {
											let size =
												if let Ok(size) = size_in_bytes.parse::<u64>() {
													Some(size.to_be_bytes().to_vec())
												} else {
													error!(
											"File path <id='{}'> had invalid size: '{}'",
											path.id, size_in_bytes
										);
													None
												};

											db.file_path().update(
												file_path::id::equals(path.id),
												vec![
													file_path::size_in_bytes_bytes::set(size),
													file_path::size_in_bytes::set(None),
												],
											)
										})
								})
								.collect::<Vec<_>>(),
						)
						.await?;
					},

					(LibraryConfigVersion::V5, LibraryConfigVersion::V6) => {
						let nodes = db.node().find_many(vec![]).exec().await?;
						if nodes.is_empty() {
							error!("6 - No nodes found... How did you even get this far? but this is fine we can fix it.");
						} else if nodes.len() > 1 {
							error!("6 - More than one node found in the DB... This can't be automatically reconciled!");
							return Err(LibraryConfigError::TooManyNodes);
						}

						let node = nodes.first();
						let now = Utc::now().fixed_offset();
						let instance_id = Uuid::new_v4();

						instance::Create {
							pub_id: instance_id.as_bytes().to_vec(),
							identity: node
								.and_then(|n| n.identity.clone())
								.unwrap_or_else(|| Identity::new().to_bytes()),
							node_id: node_config.id.as_bytes().to_vec(),
							node_name: node_config.name.clone(),
							node_platform: Platform::current() as i32,
							last_seen: now,
							date_created: node.map(|n| n.date_created).unwrap_or_else(|| now),
							_params: vec![],
						}
						.to_query(db)
						.exec()
						.await?;

						let mut config = serde_json::from_slice::<Map<String, Value>>(
							&fs::read(path).await.map_err(|e| {
								VersionManagerError::FileIO(FileIOError::from((path, e)))
							})?,
						)
						.map_err(VersionManagerError::SerdeJson)?;

						config.remove("node_id");
						config.remove("identity");

						config.insert(String::from("instance_id"), json!(instance_id.to_string()));

						fs::write(
							path,
							&serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?,
						)
						.await
						.map_err(|e| VersionManagerError::FileIO(FileIOError::from((path, e))))?;
					}

					(LibraryConfigVersion::V6, LibraryConfigVersion::V7) => {
						let instances = db.instance().find_many(vec![]).exec().await?;

						if instances.len() > 1 {
							error!("7 - More than one instance found in the DB... This can't be automatically reconciled!");
							return Err(LibraryConfigError::TooManyInstances);
						}

						let Some(instance) = instances.first() else {
							error!("7 - No instance found... How did you even get this far?!");
							return Err(LibraryConfigError::MissingInstance);
						};

						let mut config = serde_json::from_slice::<Map<String, Value>>(
							&fs::read(path).await.map_err(|e| {
								VersionManagerError::FileIO(FileIOError::from((path, e)))
							})?,
						)
						.map_err(VersionManagerError::SerdeJson)?;

						config.remove("instance_id");
						config.insert(String::from("instance_id"), json!(instance.id));

						fs::write(
							path,
							&serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?,
						)
						.await
						.map_err(|e| VersionManagerError::FileIO(FileIOError::from((path, e))))?;

						// We are relinking all locations to the current instance.
						// If you have more than one node in your database and you're not @Oscar, something went horribly wrong so this is fine.
						db.location()
							.update_many(
								vec![],
								vec![location::instance_id::set(Some(instance.id))],
							)
							.exec()
							.await?;
					}

					(LibraryConfigVersion::V7, LibraryConfigVersion::V8) => {
						let instances = db.instance().find_many(vec![]).exec().await?;
						let Some(instance) = instances.first() else {
							error!("8 - No nodes found... How did you even get this far?!");
							return Err(LibraryConfigError::MissingInstance);
						};

						// This should be in 7 but it's added to ensure to hell it runs.
						let mut config = serde_json::from_slice::<Map<String, Value>>(
							&fs::read(path).await.map_err(|e| {
								VersionManagerError::FileIO(FileIOError::from((path, e)))
							})?,
						)
						.map_err(VersionManagerError::SerdeJson)?;

						config.remove("instance_id");
						config.insert(String::from("instance_id"), json!(instance.id));

						fs::write(
							path,
							&serde_json::to_vec(&config).map_err(VersionManagerError::SerdeJson)?,
						)
						.await
						.map_err(|e| VersionManagerError::FileIO(FileIOError::from((path, e))))?;
					}

					(LibraryConfigVersion::V8, LibraryConfigVersion::V9) => {
						db._batch(
							db.instance()
								.find_many(vec![])
								.exec()
								.await?
								.into_iter()
								.map(|i| {
									db.instance().update(
										instance::id::equals(i.id),
										vec![instance::identity::set(
									// This code is assuming you only have the current node.
									// If you've paired your node with another node, reset your db.
									IdentityOrRemoteIdentity::Identity(
										Identity::from_bytes(&i.identity).expect(
											"Invalid identity detected in DB during migrations",
										),
									)
									.to_bytes(),
								)],
									)
								})
								.collect::<Vec<_>>(),
						)
						.await?;
					}

					_ => {
						error!("Library config version is not handled: {:?}", current);
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

	pub(crate) async fn save(&self, path: impl AsRef<Path>) -> Result<(), LibraryConfigError> {
		let path = path.as_ref();
		fs::write(path, &serde_json::to_vec(self)?)
			.await
			.map_err(|e| FileIOError::from((path, e)).into())
	}
}

#[derive(Error, Debug)]
pub enum LibraryConfigError {
	#[error("database error: {0}")]
	Database(#[from] prisma_client_rust::QueryError),
	#[error("there are too many nodes in the database, this should not happen!")]
	TooManyNodes,
	#[error("there are too many instances in the database, this should not happen!")]
	TooManyInstances,
	#[error("missing instances")]
	MissingInstance,

	#[error(transparent)]
	SerdeJson(#[from] serde_json::Error),
	#[error(transparent)]
	VersionManager(#[from] VersionManagerError<LibraryConfigVersion>),
	#[error(transparent)]
	FileIO(#[from] FileIOError),
}
