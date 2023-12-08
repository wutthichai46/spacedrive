use crate::{
	api::locations::ExplorerItem,
	library::Library,
	object::{
		cas::generate_cas_id,
		media::thumbnail::{get_ephemeral_thumb_key, BatchToProcess, GenerateThumbnailArgs},
	},
	prisma::location,
	util::error::FileIOError,
	Node,
};

use std::{
	collections::HashMap,
	path::{Path, PathBuf},
	sync::Arc,
	time::{Duration, Instant},
};

use sd_file_ext::{extensions::Extension, kind::ObjectKind};

use chrono::{DateTime, Utc};
use rspc::ErrorCode;
use sd_utils::chain_optional_iter;
use serde::Serialize;
use specta::Type;
use thiserror::Error;
use tokio::{fs, io};
use tracing::{error, warn};

use super::{
	file_path_helper::{path_is_hidden, MetadataExt},
	indexer::rules::{
		seed::{no_hidden, no_os_protected},
		IndexerRule, RuleKind,
	},
	normalize_path,
};

#[derive(Debug, Error)]
pub enum NonIndexedLocationError {
	#[error("path not found: {}", .0.display())]
	NotFound(PathBuf),

	#[error(transparent)]
	FileIO(#[from] FileIOError),

	#[error("database error: {0}")]
	Database(#[from] prisma_client_rust::QueryError),
}

impl From<NonIndexedLocationError> for rspc::Error {
	fn from(err: NonIndexedLocationError) -> Self {
		match err {
			NonIndexedLocationError::NotFound(_) => {
				rspc::Error::with_cause(ErrorCode::NotFound, err.to_string(), err)
			}
			_ => rspc::Error::with_cause(ErrorCode::InternalServerError, err.to_string(), err),
		}
	}
}

impl<P: AsRef<Path>> From<(P, io::Error)> for NonIndexedLocationError {
	fn from((path, source): (P, io::Error)) -> Self {
		if source.kind() == io::ErrorKind::NotFound {
			Self::NotFound(path.as_ref().into())
		} else {
			Self::FileIO(FileIOError::from((path, source)))
		}
	}
}

#[derive(Serialize, Type, Debug)]
pub struct NonIndexedFileSystemEntries {
	pub entries: Vec<ExplorerItem>,
	pub errors: Vec<rspc::Error>,
}

#[derive(Serialize, Type, Debug)]
pub struct NonIndexedPathItem {
	pub path: String,
	pub name: String,
	pub extension: String,
	pub kind: i32,
	pub is_dir: bool,
	pub date_created: DateTime<Utc>,
	pub date_modified: DateTime<Utc>,
	pub size_in_bytes_bytes: Vec<u8>,
	pub hidden: bool,
}

pub async fn walk(
	full_path: impl AsRef<Path>,
	with_hidden_files: bool,
	node: Arc<Node>,
	library: Arc<Library>,
) -> Result<NonIndexedFileSystemEntries, NonIndexedLocationError> {
	println!("\n\n-- START WALK --");
	let time = Instant::now();

	let path = full_path.as_ref();
	let mut read_dir = fs::read_dir(path).await.map_err(|e| (path, e))?;

	println!("READ DIR: {:?}", time.elapsed());

	let mut directories = vec![];
	let mut errors = vec![];
	let mut entries = vec![];

	let rules = chain_optional_iter(
		[IndexerRule::from(no_os_protected())],
		[(!with_hidden_files).then(|| IndexerRule::from(no_hidden()))],
	);

	println!("RULES: {:?}", time.elapsed());

	let mut thumbnails_to_generate = vec![];
	// Generating thumbnails for PDFs is kinda slow, so we're leaving them for last in the batch
	let mut document_thumbnails_to_generate = vec![];

	let mut times = Vec::new();
	let mut indexer_rule_apply = Vec::new();
	let mut resolve_extension_conflict = Vec::new();
	let mut cas_id_time = Vec::new();

	while let Some(entry) = read_dir.next_entry().await.map_err(|e| (path, e))? {
		let now = Instant::now();

		let Ok((entry_path, name)) = normalize_path(entry.path())
			.map_err(|e| errors.push(NonIndexedLocationError::from((path, e)).into()))
		else {
			continue;
		};

		let a = Instant::now();
		if let Ok(rule_results) = IndexerRule::apply_all(&rules, &entry_path)
			.await
			.map_err(|e| errors.push(e.into()))
		{
			// No OS Protected and No Hidden rules, must always be from this kind, should panic otherwise
			if rule_results[&RuleKind::RejectFilesByGlob]
				.iter()
				.any(|reject| !reject)
			{
				continue;
			}
		} else {
			continue;
		}
		indexer_rule_apply.push(a.elapsed());

		let Ok(metadata) = entry
			.metadata()
			.await
			.map_err(|e| errors.push(NonIndexedLocationError::from((path, e)).into()))
		else {
			continue;
		};

		if metadata.is_dir() {
			directories.push((entry_path, name, metadata));
		} else {
			let path = Path::new(&entry_path);

			let Some(name) = path
				.file_stem()
				.and_then(|s| s.to_str().map(str::to_string))
			else {
				warn!("Failed to extract name from path: {}", &entry_path);
				continue;
			};

			let extension = path
				.extension()
				.and_then(|s| s.to_str().map(str::to_string))
				.unwrap_or_default();

			let b = Instant::now();
			let kind = Extension::resolve_conflicting(&path, false)
				.await
				.map(Into::into)
				.unwrap_or(ObjectKind::Unknown);
			resolve_extension_conflict.push(b.elapsed());

			let should_generate_thumbnail = {
				#[cfg(feature = "ffmpeg")]
				{
					matches!(
						kind,
						ObjectKind::Image | ObjectKind::Video | ObjectKind::Document
					)
				}

				#[cfg(not(feature = "ffmpeg"))]
				{
					matches!(kind, ObjectKind::Image | ObjectKind::Document)
				}
			};

			let thumbnail_key = if should_generate_thumbnail {
				let b = Instant::now();

				let result = if let Ok(cas_id) = generate_cas_id(&path, metadata.len())
					.await
					.map_err(|e| errors.push(NonIndexedLocationError::from((path, e)).into()))
				{
					if kind == ObjectKind::Document {
						document_thumbnails_to_generate.push(GenerateThumbnailArgs::new(
							extension.clone(),
							cas_id.clone(),
							path.to_path_buf(),
						));
					} else {
						thumbnails_to_generate.push(GenerateThumbnailArgs::new(
							extension.clone(),
							cas_id.clone(),
							path.to_path_buf(),
						));
					}

					Some(get_ephemeral_thumb_key(&cas_id))
				} else {
					None
				};

				cas_id_time.push(b.elapsed());

				result
			} else {
				None
			};

			entries.push(ExplorerItem::NonIndexedPath {
				has_local_thumbnail: thumbnail_key.is_some(),
				thumbnail_key,
				item: NonIndexedPathItem {
					hidden: path_is_hidden(Path::new(&entry_path), &metadata),
					path: entry_path,
					name,
					extension,
					kind: kind as i32,
					is_dir: false,
					date_created: metadata.created_or_now().into(),
					date_modified: metadata.modified_or_now().into(),
					size_in_bytes_bytes: metadata.len().to_be_bytes().to_vec(),
				},
			});
		}

		// println!("{:?} {:?}", now.elapsed(), now.elapsed().as_millis());
		times.push(now.elapsed());
	}

	println!("ITERATOR TOTAL: {:?}", time.elapsed());

	// if times.len() != 0 {
	// 	let sum = times.iter().map(|d| d.as_nanos()).sum::<u128>();
	// 	println!(
	// 		"\tAVERAGE TIME PER ITER: {:?} {:?} {:?}/{}",
	// 		sum,
	// 		sum / (times.len() as u128),
	// 		sum as f64 / times.len() as f64,
	// 		times.len()
	// 	);
	// }
	// if indexer_rule_apply.len() != 0 {
	// 	let sum = indexer_rule_apply
	// 		.iter()
	// 		.map(|d| d.as_nanos())
	// 		.sum::<u128>();
	// 	let time_per_iter = sum / indexer_rule_apply.len() as u128;
	// 	let percentage_total = sum as f64 / time.elapsed().as_nanos() as f64 * 100.0;

	// 	println!(
	// 		"\tINDEXER RULE APPLY: {:?} {:?}/{} {:?}",
	// 		Duration::from_nanos(sum as u64),
	// 		time_per_iter,
	// 		indexer_rule_apply.len(),
	// 		percentage_total
	// 	);
	// }
	// if resolve_extension_conflict.len() != 0 {
	// 	let sum = resolve_extension_conflict
	// 		.iter()
	// 		.map(|d: &Duration| d.as_nanos())
	// 		.sum::<u128>();
	// 	let time_per_iter = sum / resolve_extension_conflict.len() as u128;
	// 	let percentage_total = sum as f64 / time.elapsed().as_nanos() as f64 * 100.0;

	// 	println!(
	// 		"\tRESOLVER EXT CONFLICT: {:?} {:?}/{} {:?}",
	// 		Duration::from_nanos(sum as u64),
	// 		time_per_iter,
	// 		resolve_extension_conflict.len(),
	// 		percentage_total
	// 	);
	// }
	// if cas_id_time.len() != 0 {
	// 	let sum = cas_id_time.iter().map(|d| d.as_nanos()).sum::<u128>();
	// 	let time_per_iter = sum / cas_id_time.len() as u128;
	// 	let percentage_total = sum as f64 / time.elapsed().as_nanos() as f64 * 100.0;

	// 	println!(
	// 		"\tCAS ID TIME: {:?} {:?}/{} {:?}",
	// 		Duration::from_nanos(sum as u64),
	// 		time_per_iter,
	// 		cas_id_time.len(),
	// 		percentage_total
	// 	);
	// }

	thumbnails_to_generate.extend(document_thumbnails_to_generate);

	node.thumbnailer
		.new_ephemeral_thumbnails_batch(BatchToProcess::new(thumbnails_to_generate, false, false))
		.await;

	println!("NEW EPHEMERAL THUMBNAILS BATCH: {:?}", time.elapsed());

	let mut locations = library
		.db
		.location()
		.find_many(vec![location::path::in_vec(
			directories
				.iter()
				.map(|(path, _, _)| path.clone())
				.collect(),
		)])
		.exec()
		.await?
		.into_iter()
		.flat_map(|location| {
			location
				.path
				.clone()
				.map(|location_path| (location_path, location))
		})
		.collect::<HashMap<_, _>>();

	println!("PRISMA LOCATIONS GET: {:?}", time.elapsed());

	for (directory, name, metadata) in directories {
		if let Some(location) = locations.remove(&directory) {
			entries.push(ExplorerItem::Location {
				has_local_thumbnail: false,
				thumbnail_key: None,
				item: location,
			});
		} else {
			entries.push(ExplorerItem::NonIndexedPath {
				has_local_thumbnail: false,
				thumbnail_key: None,
				item: NonIndexedPathItem {
					hidden: path_is_hidden(Path::new(&directory), &metadata),
					path: directory,
					name,
					extension: String::new(),
					kind: ObjectKind::Folder as i32,
					is_dir: true,
					date_created: metadata.created_or_now().into(),
					date_modified: metadata.modified_or_now().into(),
					size_in_bytes_bytes: metadata.len().to_be_bytes().to_vec(),
				},
			});
		}
	}

	println!("-- END WALK -- {:?}\n\n", time.elapsed());

	Ok(NonIndexedFileSystemEntries { entries, errors })
}
