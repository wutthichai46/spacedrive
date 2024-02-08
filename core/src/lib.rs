#![warn(clippy::unwrap_used, clippy::panic)]

use crate::{
	api::{CoreEvent, Router},
	location::LocationManagerError,
	object::media::thumbnail::actor::Thumbnailer,
};

#[cfg(feature = "ai")]
use sd_ai::image_labeler::{DownloadModelError, ImageLabeler, YoloV8};

use api::notifications::{Notification, NotificationData, NotificationId};
use chrono::{DateTime, Utc};
use node::config;
use notifications::Notifications;
use reqwest::{RequestBuilder, Response};

use std::{
	fmt,
	path::{Path, PathBuf},
	sync::{atomic::AtomicBool, Arc},
};

use thiserror::Error;
use tokio::{fs, sync::broadcast};
use tracing::{error, info, warn};
use tracing_appender::{
	non_blocking::{NonBlocking, WorkerGuard},
	rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{filter::FromEnvError, prelude::*, EnvFilter};

pub mod api;
mod cloud;
pub mod custom_uri;
mod env;
pub(crate) mod job;
pub mod library;
pub(crate) mod location;
pub(crate) mod node;
pub(crate) mod notifications;
pub(crate) mod object;
pub(crate) mod p2p;
pub(crate) mod preferences;
#[doc(hidden)] // TODO(@Oscar): Make this private when breaking out `utils` into `sd-utils`
pub mod util;
pub(crate) mod volume;

pub use env::Env;

pub(crate) use sd_core_sync as sync;

/// Represents a single running instance of the Spacedrive core.
/// Holds references to all the services that make up the Spacedrive core.
pub struct Node {
	pub data_dir: PathBuf,
	pub config: Arc<config::Manager>,
	pub libraries: Arc<library::Libraries>,
	pub jobs: Arc<job::Jobs>,
	pub locations: location::Locations,
	pub p2p: Arc<p2p::P2PManager>,
	pub event_bus: (broadcast::Sender<CoreEvent>, broadcast::Receiver<CoreEvent>),
	pub notifications: Notifications,
	pub thumbnailer: Thumbnailer,
	pub files_over_p2p_flag: Arc<AtomicBool>,
	pub cloud_sync_flag: Arc<AtomicBool>,
	pub env: Arc<env::Env>,
	pub http: reqwest::Client,
	#[cfg(feature = "ai")]
	pub image_labeller: ImageLabeler,
}

impl fmt::Debug for Node {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Node")
			.field("data_dir", &self.data_dir)
			.finish()
	}
}

impl Node {
	pub async fn new(
		data_dir: impl AsRef<Path>,
		env: env::Env,
	) -> Result<(Arc<Node>, Arc<Router>), NodeError> {
		let data_dir = data_dir.as_ref();

		info!("Starting core with data directory '{}'", data_dir.display());

		let env = Arc::new(env);

		#[cfg(debug_assertions)]
		let init_data = util::debug_initializer::InitConfig::load(data_dir).await?;

		// This error is ignored because it's throwing on mobile despite the folder existing.
		let _ = fs::create_dir_all(&data_dir).await;

		let event_bus = broadcast::channel(1024);
		let config = config::Manager::new(data_dir.to_path_buf())
			.await
			.map_err(NodeError::FailedToInitializeConfig)?;

		if let Some(url) = config.get().await.sd_api_origin {
			*env.api_url.lock().await = url;
		}

		#[cfg(feature = "ai")]
		let image_labeler_version = {
			sd_ai::init()?;
			config.get().await.image_labeler_version
		};

		let (locations, locations_actor) = location::Locations::new();
		let (jobs, jobs_actor) = job::Jobs::new();
		let libraries = library::Libraries::new(data_dir.join("libraries")).await?;

		let (p2p, p2p_actor) = p2p::P2PManager::new(config.clone(), libraries.clone()).await?;
		let node = Arc::new(Node {
			data_dir: data_dir.to_path_buf(),
			jobs,
			locations,
			notifications: notifications::Notifications::new(),
			p2p,
			thumbnailer: Thumbnailer::new(
				data_dir,
				libraries.clone(),
				event_bus.0.clone(),
				config.preferences_watcher(),
			)
			.await,
			config,
			event_bus,
			libraries,
			files_over_p2p_flag: Arc::new(AtomicBool::new(false)),
			cloud_sync_flag: Arc::new(AtomicBool::new(false)),
			http: reqwest::Client::new(),
			env,
			#[cfg(feature = "ai")]
			image_labeller: ImageLabeler::new(YoloV8::model(image_labeler_version)?, data_dir)
				.await
				.map_err(sd_ai::Error::from)?,
		});

		// Restore backend feature flags
		for feature in node.config.get().await.features {
			feature.restore(&node);
		}

		// Setup start actors that depend on the `Node`
		#[cfg(debug_assertions)]
		if let Some(init_data) = init_data {
			init_data.apply(&node.libraries, &node).await?;
		}

		// Be REALLY careful about ordering here or you'll get unreliable deadlock's!
		locations_actor.start(node.clone());
		node.libraries.init(&node).await?;
		jobs_actor.start(node.clone());
		p2p_actor.start(node.clone());

		let router = api::mount();

		info!("Spacedrive online.");
		Ok((node, router))
	}

	pub fn init_logger(data_dir: impl AsRef<Path>) -> Result<WorkerGuard, FromEnvError> {
		let (logfile, guard) = NonBlocking::new(
			RollingFileAppender::builder()
				.filename_prefix("sd.log")
				.rotation(Rotation::DAILY)
				.max_log_files(4)
				.build(data_dir.as_ref().join("logs"))
				.expect("Error setting up log file!"),
		);

		// Set a default if the user hasn't set an override
		if std::env::var("RUST_LOG") == Err(std::env::VarError::NotPresent) {
			let level = if cfg!(debug_assertions) {
				"debug"
			} else {
				"info"
			};

			std::env::set_var(
				"RUST_LOG",
				format!("info,sd_core={level},sd_core::location::manager=info,sd_ai={level}"),
			);
		}

		tracing_subscriber::registry()
			.with(
				tracing_subscriber::fmt::layer()
					.with_file(true)
					.with_line_number(true)
					.with_ansi(false)
					.with_writer(logfile)
					.with_filter(EnvFilter::from_default_env()),
			)
			.with(
				tracing_subscriber::fmt::layer()
					.with_file(true)
					.with_line_number(true)
					.with_writer(std::io::stdout)
					.with_filter(EnvFilter::from_default_env()),
			)
			.init();

		std::panic::set_hook(Box::new(move |panic| {
			if let Some(location) = panic.location() {
				tracing::error!(
					message = %panic,
					panic.file = format!("{}:{}", location.file(), location.line()),
					panic.column = location.column(),
				);
			} else {
				tracing::error!(message = %panic);
			}
		}));

		Ok(guard)
	}

	pub async fn shutdown(&self) {
		info!("Spacedrive shutting down...");
		self.thumbnailer.shutdown().await;
		self.jobs.shutdown().await;
		self.p2p.shutdown().await;
		#[cfg(feature = "ai")]
		self.image_labeller.shutdown().await;
		info!("Spacedrive Core shutdown successful!");
	}

	pub(crate) fn emit(&self, event: CoreEvent) {
		if let Err(e) = self.event_bus.0.send(event) {
			warn!("Error sending event to event bus: {e:?}");
		}
	}

	pub async fn emit_notification(&self, data: NotificationData, expires: Option<DateTime<Utc>>) {
		let notification = Notification {
			id: NotificationId::Node(self.notifications._internal_next_id()),
			data,
			read: false,
			expires,
		};

		match self
			.config
			.write(|cfg| cfg.notifications.push(notification.clone()))
			.await
		{
			Ok(_) => {
				self.notifications._internal_send(notification);
			}
			Err(err) => {
				error!("Error saving notification to config: {:?}", err);
			}
		}
	}

	pub async fn add_auth_header(&self, mut req: RequestBuilder) -> RequestBuilder {
		if let Some(auth_token) = self.config.get().await.auth_token {
			req = req.header("authorization", auth_token.to_header());
		};

		req
	}

	pub async fn authed_api_request(&self, req: RequestBuilder) -> Result<Response, rspc::Error> {
		let Some(auth_token) = self.config.get().await.auth_token else {
			return Err(rspc::Error::new(
				rspc::ErrorCode::Unauthorized,
				"No auth token".to_string(),
			));
		};

		let req = req.header("authorization", auth_token.to_header());

		req.send().await.map_err(|_| {
			rspc::Error::new(
				rspc::ErrorCode::InternalServerError,
				"Request failed".to_string(),
			)
		})
	}

	pub async fn api_request(&self, req: RequestBuilder) -> Result<Response, rspc::Error> {
		req.send().await.map_err(|_| {
			rspc::Error::new(
				rspc::ErrorCode::InternalServerError,
				"Request failed".to_string(),
			)
		})
	}

	pub async fn cloud_api_config(&self) -> sd_cloud_api::RequestConfig {
		sd_cloud_api::RequestConfig {
			client: self.http.clone(),
			api_url: self.env.api_url.lock().await.clone(),
			auth_token: self.config.get().await.auth_token,
		}
	}
}

impl sd_cloud_api::RequestConfigProvider for Node {
	async fn get_request_config(self: &Arc<Self>) -> sd_cloud_api::RequestConfig {
		Node::cloud_api_config(self).await
	}
}

/// Error type for Node related errors.
#[derive(Error, Debug)]
pub enum NodeError {
	#[error("NodeError::FailedToInitializeConfig({0})")]
	FailedToInitializeConfig(config::NodeConfigError),
	#[error("failed to initialize library manager: {0}")]
	FailedToInitializeLibraryManager(#[from] library::LibraryManagerError),
	#[error("failed to initialize location manager: {0}")]
	LocationManager(#[from] LocationManagerError),
	#[error("failed to initialize p2p manager: {0}")]
	P2PManager(#[from] sd_p2p::ManagerError),
	#[error("invalid platform integer: {0}")]
	InvalidPlatformInt(u8),
	#[cfg(debug_assertions)]
	#[error("init config error: {0}")]
	InitConfig(#[from] util::debug_initializer::InitConfigError),
	#[error("logger error: {0}")]
	Logger(#[from] FromEnvError),
	#[cfg(feature = "ai")]
	#[error("ai error: {0}")]
	AI(#[from] sd_ai::Error),
	#[cfg(feature = "ai")]
	#[error("Failed to download model: {0}")]
	DownloadModel(#[from] DownloadModelError),
}
