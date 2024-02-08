use crate::{
	api::{utils::InvalidateOperationEvent, CoreEvent},
	invalidate_query,
	location::{
		indexer,
		metadata::{LocationMetadataError, SpacedriveLocationMetadataFile},
	},
	node::Platform,
	object::tag,
	p2p::{self},
	sync,
	util::{mpscrr, MaybeUndefined},
	Node,
};

use sd_core_sync::SyncMessage;
use sd_p2p::spacetunnel::{Identity, IdentityOrRemoteIdentity};
use sd_prisma::prisma::{crdt_operation, instance, location, SortOrder};
use sd_utils::{
	db,
	error::{FileIOError, NonUtf8PathError},
	from_bytes_to_uuid,
};

use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	str::FromStr,
	sync::{atomic::AtomicBool, Arc},
	time::Duration,
};

use chrono::Utc;
use futures_concurrency::future::{Join, TryJoin};
use tokio::{
	fs, io,
	sync::{broadcast, RwLock},
	time::sleep,
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{Library, LibraryConfig, LibraryName};

mod error;

pub use error::*;

/// Event that is emitted to subscribers of the library manager.
#[derive(Debug, Clone)]
pub enum LibraryManagerEvent {
	Load(Arc<Library>),
	Edit(Arc<Library>),
	// TODO(@Oscar): Replace this with pairing -> ready state transitions
	InstancesModified(Arc<Library>),
	Delete(Arc<Library>),
}

/// is a singleton that manages all libraries for a node.
pub struct Libraries {
	/// libraries_dir holds the path to the directory where libraries are stored.
	pub libraries_dir: PathBuf,
	/// libraries holds the list of libraries which are currently loaded into the node.
	libraries: RwLock<HashMap<Uuid, Arc<Library>>>,
	// Transmit side of `self.rx` channel
	tx: mpscrr::Sender<LibraryManagerEvent, ()>,
	/// A channel for receiving events from the library manager.
	pub rx: mpscrr::Receiver<LibraryManagerEvent, ()>,
	pub emit_messages_flag: Arc<AtomicBool>,
}

impl Libraries {
	pub(crate) async fn new(libraries_dir: PathBuf) -> Result<Arc<Self>, LibraryManagerError> {
		fs::create_dir_all(&libraries_dir)
			.await
			.map_err(|e| FileIOError::from((&libraries_dir, e)))?;

		let (tx, rx) = mpscrr::unbounded_channel();
		Ok(Arc::new(Self {
			libraries_dir,
			libraries: Default::default(),
			tx,
			rx,
			emit_messages_flag: Arc::new(AtomicBool::new(false)),
		}))
	}

	/// Loads the initial libraries from disk.
	///
	/// `Arc<LibraryManager>` is constructed and passed to other managers for them to subscribe (`self.rx.subscribe`) then this method is run to load the initial libraries and trigger the subscriptions.
	pub async fn init(self: &Arc<Self>, node: &Arc<Node>) -> Result<(), LibraryManagerError> {
		let mut read_dir = fs::read_dir(&self.libraries_dir)
			.await
			.map_err(|e| FileIOError::from((&self.libraries_dir, e)))?;

		while let Some(entry) = read_dir
			.next_entry()
			.await
			.map_err(|e| FileIOError::from((&self.libraries_dir, e)))?
		{
			let config_path = entry.path();
			if config_path
				.extension()
				.map(|ext| ext == "sdlibrary")
				.unwrap_or(false)
				&& entry
					.metadata()
					.await
					.map_err(|e| FileIOError::from((&config_path, e)))?
					.is_file()
			{
				let Some(Ok(library_id)) = config_path
					.file_stem()
					.and_then(|v| v.to_str().map(Uuid::from_str))
				else {
					warn!(
						"Attempted to load library from path '{}' \
						but it has an invalid filename. Skipping...",
						config_path.display()
					);
					continue;
				};

				let db_path = config_path.with_extension("db");
				match fs::metadata(&db_path).await {
					Ok(_) => {}
					Err(e) if e.kind() == io::ErrorKind::NotFound => {
						warn!("Found library '{}' but no matching database file was found. Skipping...", config_path.display());
						continue;
					}
					Err(e) => return Err(FileIOError::from((db_path, e)).into()),
				}

				let _library_arc = self
					.load(library_id, &db_path, config_path, None, true, node)
					.await?;

				// FIX-ME: Linux releases crashes with *** stack smashing detected *** if spawn_volume_watcher is enabled
				// No ideia why, but this will be irrelevant after the UDisk API is implemented, so let's leave it disabled for now
				#[cfg(not(target_os = "linux"))]
				{
					use crate::volume::watcher::spawn_volume_watcher;
					spawn_volume_watcher(_library_arc.clone());
				}
			}
		}

		Ok(())
	}

	/// create creates a new library with the given config and mounts it into the running [LibraryManager].
	pub async fn create(
		self: &Arc<Self>,
		name: LibraryName,
		description: Option<String>,
		node: &Arc<Node>,
	) -> Result<Arc<Library>, LibraryManagerError> {
		self.create_with_uuid(Uuid::new_v4(), name, description, true, None, node)
			.await
	}

	pub(crate) async fn create_with_uuid(
		self: &Arc<Self>,
		id: Uuid,
		name: LibraryName,
		description: Option<String>,
		should_seed: bool,
		// `None` will fallback to default as library must be created with at least one instance
		instance: Option<instance::Create>,
		node: &Arc<Node>,
	) -> Result<Arc<Library>, LibraryManagerError> {
		if name.as_ref().is_empty() || name.as_ref().chars().all(|x| x.is_whitespace()) {
			return Err(LibraryManagerError::InvalidConfig(
				"name cannot be empty".to_string(),
			));
		}

		let config_path = self.libraries_dir.join(format!("{id}.sdlibrary"));

		let config = LibraryConfig::new(
			name,
			description,
			// First instance will be zero
			0,
			&config_path,
		)
		.await?;

		debug!(
			"Created library '{}' config at '{}'",
			id,
			config_path.display()
		);

		let node_cfg = node.config.get().await;
		let now = Utc::now().fixed_offset();
		let library = self
			.load(
				id,
				self.libraries_dir.join(format!("{id}.db")),
				config_path,
				Some({
					let mut create = instance.unwrap_or_else(|| instance::Create {
						pub_id: Uuid::new_v4().as_bytes().to_vec(),
						identity: IdentityOrRemoteIdentity::Identity(Identity::new()).to_bytes(),
						node_id: node_cfg.id.as_bytes().to_vec(),
						node_name: node_cfg.name.clone(),
						node_platform: Platform::current() as i32,
						last_seen: now,
						date_created: now,
						_params: vec![],
					});
					create._params.push(instance::id::set(config.instance_id));
					create
				}),
				should_seed,
				node,
			)
			.await?;

		debug!("Loaded library '{id:?}'");

		if should_seed {
			tag::seed::new_library(&library).await?;
			indexer::rules::seed::new_or_existing_library(&library).await?;
			debug!("Seeded library '{id:?}'");
		}

		invalidate_query!(library, "library.list");

		Ok(library)
	}

	/// `LoadedLibrary.id` can be used to get the library's id.
	pub async fn get_all(&self) -> Vec<Arc<Library>> {
		self.libraries
			.read()
			.await
			.iter()
			.map(|v| v.1.clone())
			.collect()
	}

	pub(crate) async fn edit(
		&self,
		id: Uuid,
		name: Option<LibraryName>,
		description: MaybeUndefined<String>,
		cloud_id: MaybeUndefined<String>,
	) -> Result<(), LibraryManagerError> {
		// check library is valid
		let libraries = self.libraries.read().await;
		let library = Arc::clone(
			libraries
				.get(&id)
				.ok_or(LibraryManagerError::LibraryNotFound)?,
		);

		library
			.update_config(
				|config| {
					// update the library
					if let Some(name) = name {
						config.name = name;
					}
					match description {
						MaybeUndefined::Undefined => {}
						MaybeUndefined::Null => config.description = None,
						MaybeUndefined::Value(description) => {
							config.description = Some(description)
						}
					}
					match cloud_id {
						MaybeUndefined::Undefined => {}
						MaybeUndefined::Null => config.cloud_id = None,
						MaybeUndefined::Value(cloud_id) => config.cloud_id = Some(cloud_id),
					}
				},
				self.libraries_dir.join(format!("{id}.sdlibrary")),
			)
			.await?;

		self.tx
			.emit(LibraryManagerEvent::Edit(Arc::clone(&library)))
			.await;

		invalidate_query!(library, "library.list");

		Ok(())
	}

	pub async fn delete(&self, id: &Uuid) -> Result<(), LibraryManagerError> {
		// As we're holding a write lock here, we know nothing will change during this function
		let mut libraries_write_guard = self.libraries.write().await;

		// TODO: Library go into "deletion" state until it's finished!

		let library = libraries_write_guard
			.get(id)
			.ok_or(LibraryManagerError::LibraryNotFound)?;

		self.tx
			.emit(LibraryManagerEvent::Delete(library.clone()))
			.await;

		if let Ok(location_paths) = library
			.db
			.location()
			.find_many(vec![])
			.select(location::select!({ path }))
			.exec()
			.await
			.map(|locations| locations.into_iter().filter_map(|location| location.path))
			.map_err(|e| error!("Failed to fetch locations for library deletion: {e:#?}"))
		{
			location_paths
				.map(|location_path| async move {
					if let Some(mut sd_metadata) =
						SpacedriveLocationMetadataFile::try_load(location_path).await?
					{
						sd_metadata.remove_library(*id).await?;
					}

					Ok::<_, LocationMetadataError>(())
				})
				.collect::<Vec<_>>()
				.join()
				.await
				.into_iter()
				.for_each(|res| {
					if let Err(e) = res {
						error!("Failed to remove library from location metadata: {e:#?}");
					}
				});
		}

		let db_path = self.libraries_dir.join(format!("{}.db", library.id));
		let sd_lib_path = self.libraries_dir.join(format!("{}.sdlibrary", library.id));

		(
			async {
				fs::remove_file(&db_path)
					.await
					.map_err(|e| LibraryManagerError::FileIO(FileIOError::from((db_path, e))))
			},
			async {
				fs::remove_file(&sd_lib_path)
					.await
					.map_err(|e| LibraryManagerError::FileIO(FileIOError::from((sd_lib_path, e))))
			},
		)
			.try_join()
			.await?;

		// We only remove here after files deletion
		let library = libraries_write_guard
			.remove(id)
			.expect("we have exclusive access and checked it exists!");

		info!("Removed Library <id='{}'>", library.id);

		invalidate_query!(library, "library.list");

		Ok(())
	}

	// get_ctx will return the library context for the given library id.
	pub async fn get_library(&self, library_id: &Uuid) -> Option<Arc<Library>> {
		self.libraries.read().await.get(library_id).cloned()
	}

	// get_ctx will return the library context for the given library id.
	pub async fn hash_library(&self, library_id: &Uuid) -> bool {
		self.libraries.read().await.get(library_id).is_some()
	}

	/// load the library from a given path.
	pub async fn load(
		self: &Arc<Self>,
		id: Uuid,
		db_path: impl AsRef<Path>,
		config_path: impl AsRef<Path>,
		create: Option<instance::Create>,
		should_seed: bool,
		node: &Arc<Node>,
	) -> Result<Arc<Library>, LibraryManagerError> {
		let db_path = db_path.as_ref();
		let config_path = config_path.as_ref();

		let db_url = format!(
			"file:{}?socket_timeout=15&connection_limit=1",
			db_path.as_os_str().to_str().ok_or_else(|| {
				LibraryManagerError::NonUtf8Path(NonUtf8PathError(db_path.into()))
			})?
		);
		let db = Arc::new(db::load_and_migrate(&db_url).await?);

		if let Some(create) = create {
			create.to_query(&db).exec().await?;
		}

		let node_config = node.config.get().await;
		let config = LibraryConfig::load(config_path, &node_config, &db).await?;

		let instances = db.instance().find_many(vec![]).exec().await?;

		let instance = instances
			.iter()
			.find(|i| i.id == config.instance_id)
			.ok_or_else(|| {
				LibraryManagerError::CurrentInstanceNotFound(config.instance_id.to_string())
			})?;

		let identity = Arc::new(
			match IdentityOrRemoteIdentity::from_bytes(&instance.identity)? {
				IdentityOrRemoteIdentity::Identity(identity) => identity,
				IdentityOrRemoteIdentity::RemoteIdentity(_) => {
					return Err(LibraryManagerError::InvalidIdentity)
				}
			},
		);

		let instance_id = Uuid::from_slice(&instance.pub_id)?;
		let curr_platform = Platform::current() as i32;
		let instance_node_id = Uuid::from_slice(&instance.node_id)?;
		if instance_node_id != node_config.id
			|| instance.node_platform != curr_platform
			|| instance.node_name != node_config.name
		{
			info!(
				"Detected that the library '{}' has changed node from '{}' to '{}'. Reconciling node data...",
				id, instance_node_id, node_config.id
			);

			db.instance()
				.update(
					instance::id::equals(instance.id),
					vec![
						instance::node_id::set(node_config.id.as_bytes().to_vec()),
						instance::node_platform::set(curr_platform),
						instance::node_name::set(node_config.name),
					],
				)
				.exec()
				.await?;
		}

		// TODO: Move this reconciliation into P2P and do reconciliation of both local and remote nodes.

		// let key_manager = Arc::new(KeyManager::new(vec![]).await?);
		// seed_keymanager(&db, &key_manager).await?;

		let sync = sync::Manager::new(&db, instance_id, &self.emit_messages_flag, {
			db._batch(
				instances
					.iter()
					.map(|i| {
						db.crdt_operation()
							.find_first(vec![crdt_operation::instance::is(vec![
								instance::id::equals(i.id),
							])])
							.order_by(crdt_operation::timestamp::order(SortOrder::Desc))
					})
					.collect::<Vec<_>>(),
			)
			.await?
			.into_iter()
			.zip(&instances)
			.map(|(op, i)| {
				(
					from_bytes_to_uuid(&i.pub_id),
					sd_sync::NTP64(op.map(|o| o.timestamp).unwrap_or_default() as u64),
				)
			})
			.collect()
		});

		let (tx, mut rx) = broadcast::channel(10);
		let library = Library::new(
			id,
			config,
			instance_id,
			identity,
			// key_manager,
			db,
			node,
			Arc::new(sync.manager),
			tx,
		)
		.await;

		// This is an exception. Generally subscribe to this by `self.tx.subscribe`.
		tokio::spawn(sync_rx_actor(library.clone(), node.clone(), sync.rx));

		crate::cloud::sync::declare_actors(&library, node).await;

		self.tx
			.emit(LibraryManagerEvent::Load(library.clone()))
			.await;

		self.libraries
			.write()
			.await
			.insert(library.id, Arc::clone(&library));

		if should_seed {
			// library.orphan_remover.invoke().await;
			indexer::rules::seed::new_or_existing_library(&library).await?;
		}

		for location in library
			.db
			.location()
			.find_many(vec![
				// TODO(N): This isn't gonna work with removable media and this will likely permanently break if the DB is restored from a backup.
				location::instance_id::equals(Some(instance.id)),
			])
			.exec()
			.await?
		{
			if let Err(e) = node.locations.add(location.id, library.clone()).await {
				error!("Failed to watch location on startup: {e}");
			};
		}

		if let Err(e) = node.jobs.clone().cold_resume(node, &library).await {
			error!("Failed to resume jobs for library. {:#?}", e);
		}

		tokio::spawn({
			let this = self.clone();
			let node = node.clone();
			let library = library.clone();
			async move {
				loop {
					debug!("Syncing library with cloud!");

					if let Some(_) = library.config().await.cloud_id {
						if let Ok(lib) =
							sd_cloud_api::library::get(node.cloud_api_config().await, library.id)
								.await
						{
							match lib {
								Some(lib) => {
									if let Some(this_instance) = lib
										.instances
										.iter()
										.find(|i| i.uuid == library.instance_uuid)
									{
										let node_config = node.config.get().await;
										let should_update = this_instance.node_id != node_config.id
											|| this_instance.node_platform
												!= (Platform::current() as u8)
											|| this_instance.node_name != node_config.name;

										if should_update {
											warn!("Library instance on cloud is outdated. Updating...");

											if let Err(err) =
												sd_cloud_api::library::update_instance(
													node.cloud_api_config().await,
													library.id,
													this_instance.uuid,
													Some(node_config.id),
													Some(node_config.name),
													Some(Platform::current() as u8),
												)
												.await
											{
												error!(
													"Failed to updating instance '{}' on cloud: {:#?}",
													this_instance.uuid, err
												);
											}
										}
									}

									if &lib.name != &*library.config().await.name {
										warn!("Library name on cloud is outdated. Updating...");

										if let Err(err) = sd_cloud_api::library::update(
											node.cloud_api_config().await,
											library.id,
											Some(lib.name),
										)
										.await
										{
											error!(
												"Failed to update library name on cloud: {:#?}",
												err
											);
										}
									}

									for instance in lib.instances {
										if let Err(err) =
											crate::cloud::sync::receive::create_instance(
												&library,
												&node.libraries,
												instance.uuid,
												instance.identity,
												instance.node_id,
												instance.node_name,
												instance.node_platform,
											)
											.await
										{
											error!(
												"Failed to create instance from cloud: {:#?}",
												err
											);
										}
									}
								}
								None => {
									warn!(
										"Library not found on cloud. Removing from local node..."
									);

									let _ = this
										.edit(
											library.id.clone(),
											None,
											MaybeUndefined::Undefined,
											MaybeUndefined::Null,
										)
										.await;
								}
							}
						}
					}

					tokio::select! {
						// Update instances every 2 minutes
						_ = sleep(Duration::from_secs(120)) => {}
						// Or when asked by user
						Ok(_) = rx.recv() => {}
					};
				}
			}
		});

		Ok(library)
	}

	pub async fn update_instances(&self, library: Arc<Library>) {
		self.tx
			.emit(LibraryManagerEvent::InstancesModified(library))
			.await;
	}
}

async fn sync_rx_actor(
	library: Arc<Library>,
	node: Arc<Node>,
	mut sync_rx: broadcast::Receiver<SyncMessage>,
) {
	loop {
		let Ok(msg) = sync_rx.recv().await else {
			continue;
		};

		match msg {
			// TODO: Any sync event invalidates the entire React Query cache this is a hacky workaround until the new invalidation system.
			SyncMessage::Ingested => node.emit(CoreEvent::InvalidateOperation(
				InvalidateOperationEvent::all(),
			)),
			SyncMessage::Created => {
				p2p::sync::originator(library.id, &library.sync, &node.p2p).await
			}
		}
	}
}
