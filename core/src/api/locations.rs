use crate::{
	invalidate_query,
	job::StatefulJob,
	location::{
		delete_location, find_location,
		indexer::{rules::IndexerRuleCreateArgs, IndexerJobInit},
		light_scan_location, location_with_indexer_rules,
		non_indexed::NonIndexedPathItem,
		relink_location, scan_location, scan_location_sub_path, LocationCreateArgs, LocationError,
		LocationUpdateArgs,
	},
	object::file_identifier::file_identifier_job::FileIdentifierJobInit,
	p2p::PeerMetadata,
	util::AbortOnDrop,
};

use sd_cache::{CacheNode, Model, Normalise, NormalisedResult, NormalisedResults, Reference};
use sd_prisma::prisma::{
	file_path, indexer_rule, indexer_rules_in_location, location, object, SortOrder,
};

use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, Utc};
use directories::UserDirs;
use rspc::{self, alpha::AlphaRouter, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;
use tracing::{debug, error};

use super::{labels::label_with_objects, utils::library, Ctx, R};

// it includes the shard hex formatted as ([["f02", "cab34a76fbf3469f"]])
// Will be None if no thumbnail exists
pub type ThumbnailKey = Vec<String>;

#[derive(Serialize, Type, Debug)]
#[serde(tag = "type")]
pub enum ExplorerItem {
	Path {
		thumbnail: Option<ThumbnailKey>,
		item: file_path_with_object::Data,
	},
	Object {
		thumbnail: Option<ThumbnailKey>,
		item: object_with_file_paths::Data,
	},
	Location {
		item: location::Data,
	},
	NonIndexedPath {
		thumbnail: Option<ThumbnailKey>,
		item: NonIndexedPathItem,
	},
	SpacedropPeer {
		item: PeerMetadata,
	},
	Label {
		thumbnails: Vec<ThumbnailKey>,
		item: label_with_objects::Data,
	},
}

// TODO: Really this shouldn't be a `Model` but it's easy for now.
// In the future we should store the inner data of the variant on behalf of it's existing model so it works cross queries.
impl Model for ExplorerItem {
	fn name() -> &'static str {
		"ExplorerItem"
	}
}

impl ExplorerItem {
	pub fn id(&self) -> String {
		let ty = match self {
			ExplorerItem::Path { .. } => "FilePath",
			ExplorerItem::Object { .. } => "Object",
			ExplorerItem::Location { .. } => "Location",
			ExplorerItem::NonIndexedPath { .. } => "NonIndexedPath",
			ExplorerItem::SpacedropPeer { .. } => "SpacedropPeer",
			ExplorerItem::Label { .. } => "Label",
		};
		match self {
			ExplorerItem::Path { item, .. } => format!("{ty}:{}", item.id),
			ExplorerItem::Object { item, .. } => format!("{ty}:{}", item.id),
			ExplorerItem::Location { item, .. } => format!("{ty}:{}", item.id),
			ExplorerItem::NonIndexedPath { item, .. } => format!("{ty}:{}", item.path),
			ExplorerItem::SpacedropPeer { item, .. } => format!("{ty}:{}", item.name), // TODO: Use a proper primary key
			ExplorerItem::Label { item, .. } => format!("{ty}:{}", item.name),
		}
	}
}

#[derive(Serialize, Type, Debug)]
pub struct SystemLocations {
	desktop: Option<PathBuf>,
	documents: Option<PathBuf>,
	downloads: Option<PathBuf>,
	pictures: Option<PathBuf>,
	music: Option<PathBuf>,
	videos: Option<PathBuf>,
}

impl From<UserDirs> for SystemLocations {
	fn from(value: UserDirs) -> Self {
		Self {
			desktop: value.desktop_dir().map(Path::to_path_buf),
			documents: value.document_dir().map(Path::to_path_buf),
			downloads: value.download_dir().map(Path::to_path_buf),
			pictures: value.picture_dir().map(Path::to_path_buf),
			music: value.audio_dir().map(Path::to_path_buf),
			videos: value.video_dir().map(Path::to_path_buf),
		}
	}
}

impl ExplorerItem {
	pub fn name(&self) -> &str {
		match self {
			ExplorerItem::Path {
				item: file_path_with_object::Data { name, .. },
				..
			}
			| ExplorerItem::Location {
				item: location::Data { name, .. },
				..
			} => name.as_deref().unwrap_or(""),
			ExplorerItem::NonIndexedPath { item, .. } => item.name.as_str(),
			_ => "",
		}
	}

	pub fn size_in_bytes(&self) -> u64 {
		match self {
			ExplorerItem::Path {
				item: file_path_with_object::Data {
					size_in_bytes_bytes,
					..
				},
				..
			} => size_in_bytes_bytes
				.as_ref()
				.map(|size| {
					u64::from_be_bytes([
						size[0], size[1], size[2], size[3], size[4], size[5], size[6], size[7],
					])
				})
				.unwrap_or(0),

			ExplorerItem::NonIndexedPath {
				item: NonIndexedPathItem {
					size_in_bytes_bytes,
					..
				},
				..
			} => u64::from_be_bytes([
				size_in_bytes_bytes[0],
				size_in_bytes_bytes[1],
				size_in_bytes_bytes[2],
				size_in_bytes_bytes[3],
				size_in_bytes_bytes[4],
				size_in_bytes_bytes[5],
				size_in_bytes_bytes[6],
				size_in_bytes_bytes[7],
			]),
			_ => 0,
		}
	}

	pub fn date_created(&self) -> DateTime<Utc> {
		match self {
			ExplorerItem::Path {
				item: file_path_with_object::Data { date_created, .. },
				..
			}
			| ExplorerItem::Object {
				item: object_with_file_paths::Data { date_created, .. },
				..
			}
			| ExplorerItem::Location {
				item: location::Data { date_created, .. },
				..
			} => date_created.map(Into::into).unwrap_or_default(),

			ExplorerItem::NonIndexedPath { item, .. } => item.date_created,
			_ => Default::default(),
		}
	}

	pub fn date_modified(&self) -> DateTime<Utc> {
		match self {
			ExplorerItem::Path { item, .. } => {
				item.date_modified.map(Into::into).unwrap_or_default()
			}
			ExplorerItem::NonIndexedPath { item, .. } => item.date_modified,
			_ => Default::default(),
		}
	}
}

file_path::include!(file_path_with_object { object });
object::include!(object_with_file_paths { file_paths });

pub(crate) fn mount() -> AlphaRouter<Ctx> {
	R.router()
		.procedure("list", {
			R.with2(library()).query(|(_, library), _: ()| async move {
				let locations = library
					.db
					.location()
					.find_many(vec![])
					.order_by(location::date_created::order(SortOrder::Desc))
					.exec()
					.await?;

				let (nodes, items) = locations.normalise(|i| i.id.to_string());

				Ok(NormalisedResults { items, nodes })
			})
		})
		.procedure("get", {
			R.with2(library())
				.query(|(_, library), location_id: location::id::Type| async move {
					Ok(library
						.db
						.location()
						.find_unique(location::id::equals(location_id))
						.exec()
						.await?
						.map(|i| NormalisedResult::from(i, |i| i.id.to_string())))
				})
		})
		.procedure("getWithRules", {
			#[derive(Type, Serialize)]
			struct LocationWithIndexerRule {
				pub id: i32,
				pub pub_id: Vec<u8>,
				pub name: Option<String>,
				pub path: Option<String>,
				pub total_capacity: Option<i32>,
				pub available_capacity: Option<i32>,
				pub size_in_bytes: Option<Vec<u8>>,
				pub is_archived: Option<bool>,
				pub generate_preview_media: Option<bool>,
				pub sync_preview_media: Option<bool>,
				pub hidden: Option<bool>,
				pub date_created: Option<DateTime<FixedOffset>>,
				pub instance_id: Option<i32>,
				pub indexer_rules: Vec<Reference<indexer_rule::Data>>,
			}

			impl Model for LocationWithIndexerRule {
				fn name() -> &'static str {
					"Location" // This is a duplicate identifier as `location::Data` but it's fine because because they are the same entity
				}
			}

			impl LocationWithIndexerRule {
				pub fn from_db(
					nodes: &mut Vec<CacheNode>,
					value: location_with_indexer_rules::Data,
				) -> Reference<Self> {
					let this = Self {
						id: value.id,
						pub_id: value.pub_id,
						name: value.name,
						path: value.path,
						total_capacity: value.total_capacity,
						available_capacity: value.available_capacity,
						size_in_bytes: value.size_in_bytes,
						is_archived: value.is_archived,
						generate_preview_media: value.generate_preview_media,
						sync_preview_media: value.sync_preview_media,
						hidden: value.hidden,
						date_created: value.date_created,
						instance_id: value.instance_id,
						indexer_rules: value
							.indexer_rules
							.into_iter()
							.map(|i| {
								let id = i.indexer_rule.id.to_string();

								nodes.push(CacheNode::new(id.clone(), i.indexer_rule));
								Reference::new(id)
							})
							.collect(),
					};

					let id = this.id.to_string();
					nodes.push(CacheNode::new(id.clone(), this));
					Reference::new(id)
				}
			}

			R.with2(library())
				.query(|(_, library), location_id: location::id::Type| async move {
					Ok(library
						.db
						.location()
						.find_unique(location::id::equals(location_id))
						.include(location_with_indexer_rules::include())
						.exec()
						.await?
						.map(|location| {
							let mut nodes = Vec::new();
							NormalisedResult {
								item: LocationWithIndexerRule::from_db(&mut nodes, location),
								nodes,
							}
						}))
				})
		})
		.procedure("create", {
			R.with2(library())
				.mutation(|(node, library), args: LocationCreateArgs| async move {
					if let Some(location) = args.create(&node, &library).await? {
						let id = Some(location.id);
						scan_location(&node, &library, location).await?;
						invalidate_query!(library, "locations.list");
						Ok(id)
					} else {
						Ok(None)
					}
				})
		})
		.procedure("update", {
			R.with2(library())
				.mutation(|(node, library), args: LocationUpdateArgs| async move {
					let ret = args.update(&node, &library).await.map_err(Into::into);
					invalidate_query!(library, "locations.list");
					ret
				})
		})
		.procedure("delete", {
			R.with2(library()).mutation(
				|(node, library), location_id: location::id::Type| async move {
					delete_location(&node, &library, location_id).await?;
					invalidate_query!(library, "locations.list");
					Ok(())
				},
			)
		})
		.procedure("relink", {
			R.with2(library())
				.mutation(|(_, library), location_path: PathBuf| async move {
					relink_location(&library, location_path)
						.await
						.map_err(Into::into)
				})
		})
		.procedure("addLibrary", {
			R.with2(library())
				.mutation(|(node, library), args: LocationCreateArgs| async move {
					if let Some(location) = args.add_library(&node, &library).await? {
						let id = location.id;
						scan_location(&node, &library, location).await?;
						invalidate_query!(library, "locations.list");
						Ok(Some(id))
					} else {
						Ok(None)
					}
				})
		})
		.procedure("fullRescan", {
			#[derive(Type, Deserialize)]
			pub struct FullRescanArgs {
				pub location_id: location::id::Type,
				pub reidentify_objects: bool,
			}

			R.with2(library()).mutation(
				|(node, library),
				 FullRescanArgs {
				     location_id,
				     reidentify_objects,
				 }| async move {
					if reidentify_objects {
						let count = library
							.db
							.file_path()
							.update_many(
								vec![
									file_path::location_id::equals(Some(location_id)),
									file_path::object_id::not(None),
									file_path::cas_id::not(None),
								],
								vec![
									file_path::object::disconnect(),
									file_path::cas_id::set(None),
								],
							)
							.exec()
							.await?;

						debug!("Disconnected {count} file paths from objects");

						// library.orphan_remover.invoke().await;
					}

					// rescan location
					scan_location(
						&node,
						&library,
						find_location(&library, location_id)
							.include(location_with_indexer_rules::include())
							.exec()
							.await?
							.ok_or(LocationError::IdNotFound(location_id))?,
					)
					.await
					.map_err(Into::into)
				},
			)
		})
		.procedure("subPathRescan", {
			#[derive(Clone, Serialize, Deserialize, Type, Debug)]
			pub struct RescanArgs {
				pub location_id: location::id::Type,
				pub sub_path: String,
			}

			R.with2(library()).mutation(
				|(node, library),
				 RescanArgs {
				     location_id,
				     sub_path,
				 }: RescanArgs| async move {
					scan_location_sub_path(
						&node,
						&library,
						find_location(&library, location_id)
							.include(location_with_indexer_rules::include())
							.exec()
							.await?
							.ok_or(LocationError::IdNotFound(location_id))?,
						sub_path,
					)
					.await
					.map_err(Into::into)
				},
			)
		})
		.procedure("quickRescan", {
			#[derive(Clone, Serialize, Deserialize, Type, Debug)]
			pub struct LightScanArgs {
				pub location_id: location::id::Type,
				pub sub_path: String,
			}

			R.with2(library()).subscription(
				|(node, library),
				 LightScanArgs {
				     location_id,
				     sub_path,
				 }: LightScanArgs| async move {
					if node
						.jobs
						.has_job_running(|job_identity| {
							job_identity.target_location == location_id
								&& (job_identity.name == <IndexerJobInit as StatefulJob>::NAME
									|| job_identity.name
										== <FileIdentifierJobInit as StatefulJob>::NAME)
						})
						.await
					{
						return Err(rspc::Error::new(
							ErrorCode::Conflict,
							"We're still indexing this location, pleases wait a bit...".to_string(),
						));
					}

					let location = find_location(&library, location_id)
						.include(location_with_indexer_rules::include())
						.exec()
						.await?
						.ok_or(LocationError::IdNotFound(location_id))?;

					let handle = tokio::spawn(async move {
						if let Err(e) = light_scan_location(node, library, location, sub_path).await
						{
							error!("light scan error: {e:#?}");
						}
					});

					Ok(AbortOnDrop(handle))
				},
			)
		})
		.procedure(
			"online",
			R.subscription(|node, _: ()| async move {
				let mut rx = node.locations.online_rx();

				async_stream::stream! {
					let online = node.locations.get_online().await;

					yield online;

					while let Ok(locations) = rx.recv().await {
						yield locations;
					}
				}
			}),
		)
		.procedure("systemLocations", {
			R.query(|_, _: ()| async move {
				UserDirs::new().map(SystemLocations::from).ok_or_else(|| {
					rspc::Error::new(
						ErrorCode::NotFound,
						"Didn't find any system locations".to_string(),
					)
				})
			})
		})
		.merge("indexer_rules.", mount_indexer_rule_routes())
}

fn mount_indexer_rule_routes() -> AlphaRouter<Ctx> {
	R.router()
		.procedure("create", {
			R.with2(library())
				.mutation(|(_, library), args: IndexerRuleCreateArgs| async move {
					if args.create(&library).await?.is_some() {
						invalidate_query!(library, "locations.indexer_rules.list");
					}

					Ok(())
				})
		})
		.procedure("delete", {
			R.with2(library())
				.mutation(|(_, library), indexer_rule_id: i32| async move {
					let indexer_rule_db = library.db.indexer_rule();

					if let Some(indexer_rule) = indexer_rule_db
						.to_owned()
						.find_unique(indexer_rule::id::equals(indexer_rule_id))
						.exec()
						.await?
					{
						if indexer_rule.default.unwrap_or_default() {
							return Err(rspc::Error::new(
								ErrorCode::Forbidden,
								format!("Indexer rule <id={indexer_rule_id}> can't be deleted"),
							));
						}
					} else {
						return Err(rspc::Error::new(
							ErrorCode::NotFound,
							format!("Indexer rule <id={indexer_rule_id}> not found"),
						));
					}

					library
						.db
						.indexer_rules_in_location()
						.delete_many(vec![indexer_rules_in_location::indexer_rule_id::equals(
							indexer_rule_id,
						)])
						.exec()
						.await?;

					indexer_rule_db
						.delete(indexer_rule::id::equals(indexer_rule_id))
						.exec()
						.await?;

					invalidate_query!(library, "locations.indexer_rules.list");

					Ok(())
				})
		})
		.procedure("get", {
			R.with2(library())
				.query(|(_, library), indexer_rule_id: i32| async move {
					library
						.db
						.indexer_rule()
						.find_unique(indexer_rule::id::equals(indexer_rule_id))
						.exec()
						.await?
						.ok_or_else(|| {
							rspc::Error::new(
								ErrorCode::NotFound,
								format!("Indexer rule <id={indexer_rule_id}> not found"),
							)
						})
						.map(|i| NormalisedResult::from(i, |i| i.id.to_string()))
				})
		})
		.procedure("list", {
			R.with2(library()).query(|(_, library), _: ()| async move {
				let rules = library.db.indexer_rule().find_many(vec![]).exec().await?;

				let (nodes, items) = rules.normalise(|i| i.id.to_string());

				Ok(NormalisedResults { items, nodes })
			})
		})
		// list indexer rules for location, returning the indexer rule
		.procedure("listForLocation", {
			R.with2(library())
				.query(|(_, library), location_id: location::id::Type| async move {
					let rules = library
						.db
						.indexer_rule()
						.find_many(vec![indexer_rule::locations::some(vec![
							indexer_rules_in_location::location_id::equals(location_id),
						])])
						.exec()
						.await?;

					let (nodes, items) = rules.normalise(|i| i.id.to_string());

					Ok(NormalisedResults { items, nodes })
				})
		})
}
