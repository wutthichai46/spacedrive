use crate::{invalidate_query, library::Library, object::media::thumbnail::get_indexed_thumb_key};

use sd_prisma::prisma::{label, label_on_object, object, SortOrder};

use std::collections::BTreeMap;

use rspc::alpha::AlphaRouter;

use super::{locations::ExplorerItem, utils::library, Ctx, R};

label::include!((take: i64) => label_with_objects {
	label_objects(vec![]).take(take): select {
		object: select {
			id
			file_paths(vec![]).take(1)
		}
	}
});

pub(crate) fn mount() -> AlphaRouter<Ctx> {
	R.router()
		.procedure("list", {
			R.with2(library()).query(|(_, library), _: ()| async move {
				Ok(library.db.label().find_many(vec![]).exec().await?)
			})
		})
		.procedure("listWithThumbnails", {
			R.with2(library())
				.query(|(_, library), cursor: label::name::Type| async move {
					Ok(library
						.db
						.label()
						.find_many(vec![label::name::gt(cursor)])
						.order_by(label::name::order(SortOrder::Asc))
						.include(label_with_objects::include(4))
						.exec()
						.await?
						.into_iter()
						.map(|label| ExplorerItem::Label {
							item: label.clone(),
							// map the first 4 objects to thumbnails
							thumbnails: label
								.label_objects
								.into_iter()
								.take(10)
								.filter_map(|label_object| {
									label_object.object.file_paths.into_iter().next()
								})
								.filter_map(|file_path_data| {
									file_path_data
										.cas_id
										.as_ref()
										.map(|cas_id| get_indexed_thumb_key(cas_id, library.id))
								}) // Filter out None values and transform each element to Vec<Vec<String>>
								.collect::<Vec<_>>(), // Collect into Vec<Vec<Vec<String>>>
						})
						.collect::<Vec<_>>())
				})
		})
		.procedure("count", {
			R.with2(library()).query(|(_, library), _: ()| async move {
				Ok(library.db.label().count(vec![]).exec().await? as i32)
			})
		})
		.procedure("getForObject", {
			R.with2(library())
				.query(|(_, library), object_id: i32| async move {
					Ok(library
						.db
						.label()
						.find_many(vec![label::label_objects::some(vec![
							label_on_object::object_id::equals(object_id),
						])])
						.exec()
						.await?)
				})
		})
		.procedure("getWithObjects", {
			R.with2(library()).query(
				|(_, library), object_ids: Vec<object::id::Type>| async move {
					let Library { db, .. } = library.as_ref();
					let labels_with_objects = db
						.label()
						.find_many(vec![label::label_objects::some(vec![
							label_on_object::object_id::in_vec(object_ids.clone()),
						])])
						.select(label::select!({
							id
							label_objects(vec![label_on_object::object_id::in_vec(object_ids.clone())]): select {
								date_created
								object: select {
									id
								}
							}
						}))
						.exec()
						.await?;
					Ok(labels_with_objects
						.into_iter()
						.map(|label| (label.id, label.label_objects))
						.collect::<BTreeMap<_, _>>())
				},
			)
		})
		.procedure("get", {
			R.with2(library())
				.query(|(_, library), label_id: i32| async move {
					Ok(library
						.db
						.label()
						.find_unique(label::id::equals(label_id))
						.exec()
						.await?)
				})
		})
		.procedure(
			"delete",
			R.with2(library())
				.mutation(|(_, library), label_id: i32| async move {
					library
						.db
						.label()
						.delete(label::id::equals(label_id))
						.exec()
						.await?;

					invalidate_query!(library, "labels.list");

					Ok(())
				}),
		)
}
