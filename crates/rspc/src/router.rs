use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    marker::PhantomData,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use futures::{Stream, StreamExt};
use serde_json::Value;
use specta::{
    ts::{self, datatype, ExportConfig},
    DataType, TypeMap,
};

use crate::{
    internal::{Procedure, ProcedureKind, ProcedureStore, RequestContext},
    Config, ExecError, ExportError,
};

/// TODO
pub struct Router<TCtx = (), TMeta = ()>
where
    TCtx: 'static,
{
    pub(crate) config: Config,
    pub(crate) queries: ProcedureStore<TCtx>,
    pub(crate) mutations: ProcedureStore<TCtx>,
    pub(crate) subscriptions: ProcedureStore<TCtx>,
    pub(crate) typ_store: TypeMap,
    pub(crate) phantom: PhantomData<TMeta>,
}

// TODO: Move this out of this file
// TODO: Rename??
pub enum ExecKind {
    Query,
    Mutation,
}

impl<TCtx, TMeta> Router<TCtx, TMeta>
where
    TCtx: Send + 'static,
{
    // TODO: Deprecate these in 0.1.3 and move into internal package and merge with `execute_jsonrpc`?
    pub async fn exec(
        &self,
        ctx: TCtx,
        kind: ExecKind,
        key: String,
        input: Option<Value>,
    ) -> Result<Value, ExecError> {
        let (operations, kind) = match kind {
            ExecKind::Query => (&self.queries.store, ProcedureKind::Query),
            ExecKind::Mutation => (&self.mutations.store, ProcedureKind::Mutation),
        };

        let mut stream = operations
            .get(&key)
            .ok_or_else(|| ExecError::OperationNotFound(key.clone()))?
            .exec
            .call(
                ctx,
                input.unwrap_or(Value::Null),
                RequestContext {
                    kind,
                    path: key.clone(),
                },
            )
            .await?;

        stream.next().await.unwrap()
    }

    // TODO: Deprecate these in 0.1.3 and move into internal package and merge with `execute_jsonrpc`?
    pub async fn exec_subscription(
        &self,
        ctx: TCtx,
        key: String,
        input: Option<Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Value, ExecError>> + Send + '_>>, ExecError> {
        self.subscriptions
            .store
            .get(&key)
            .ok_or_else(|| ExecError::OperationNotFound(key.clone()))?
            .exec
            .call(
                ctx,
                input.unwrap_or(Value::Null),
                RequestContext {
                    kind: ProcedureKind::Subscription,
                    path: key.clone(),
                },
            )
            .await
    }

    pub fn arced(self) -> Arc<Self> {
        Arc::new(self)
    }

    pub fn typ_store(&self) -> TypeMap {
        self.typ_store.clone()
    }

    #[cfg(feature = "unstable")]
    pub fn typ_store_mut(&mut self) -> &mut TypeMap {
        &mut self.typ_store
    }

    // TODO: Drop this API in v1
    pub fn queries(&self) -> &BTreeMap<String, Procedure<TCtx>> {
        &self.queries.store
    }

    // TODO: Drop this API in v1
    pub fn mutations(&self) -> &BTreeMap<String, Procedure<TCtx>> {
        &self.mutations.store
    }

    // TODO: Drop this API in v1
    pub fn subscriptions(&self) -> &BTreeMap<String, Procedure<TCtx>> {
        &self.subscriptions.store
    }

    #[allow(clippy::unwrap_used)] // TODO
    pub fn export_ts<TPath: AsRef<Path>>(&self, export_path: TPath) -> Result<(), ExportError> {
        let export_path = PathBuf::from(export_path.as_ref());
        if let Some(export_dir) = export_path.parent() {
            fs::create_dir_all(export_dir)?;
        }
        let mut file = File::create(export_path)?;
        if let Some(header) = &self.config.bindings_header {
            writeln!(file, "{}", header)?;
        }
        writeln!(file, "// This file was generated by [rspc](https://github.com/oscartbeaumont/rspc). Do not edit this file manually.")?;

        let config = ExportConfig::new().bigint(
            ts::BigIntExportBehavior::FailWithReason(
                "rspc does not support exporting bigint types (i64, u64, i128, u128) because they are lossily decoded by `JSON.parse` on the frontend. Tracking issue: https://github.com/oscartbeaumont/rspc/issues/93",
            )
        );

        let queries_ts =
            generate_procedures_ts(&config, self.queries.store.iter(), &self.typ_store());
        let mutations_ts =
            generate_procedures_ts(&config, self.mutations.store.iter(), &self.typ_store());
        let subscriptions_ts =
            generate_procedures_ts(&config, self.subscriptions.store.iter(), &self.typ_store());

        // TODO: Specta API
        writeln!(
            file,
            r#"
export type Procedures = {{
    queries: {queries_ts},
    mutations: {mutations_ts},
    subscriptions: {subscriptions_ts}
}};"#
        )?;

        for (name, a, b) in specta::detect_duplicate_type_names(&self.typ_store) {
            return Err(ExportError::TsExportErr(
                ts::ExportError::DuplicateTypeName(name, a, b),
            ));
        }

        for (_sid, dt) in self.typ_store.iter() {
            writeln!(
                file,
                "\n{}",
                ts::export_named_datatype(&config, dt, &self.typ_store)?
            )?;
        }

        Ok(())
    }
}

// TODO: Move this out into a Specta API
fn generate_procedures_ts<'a, Ctx: 'a>(
    config: &ExportConfig,
    procedures: impl ExactSizeIterator<Item = (&'a String, &'a Procedure<Ctx>)>,
    type_store: &TypeMap,
) -> String {
    match procedures.len() {
        0 => "never".to_string(),
        _ => procedures
            .map(|(key, operation)| {
                let input = match &operation.ty.input {
                    DataType::Tuple(def)
                        // This condition is met with an empty enum or `()`.
                        if def.elements().is_empty() =>
                    {
                        "never".into()
                    }
                    #[allow(clippy::unwrap_used)] // TODO
                    ty => datatype(config, ty, type_store).unwrap(),
                };
                #[allow(clippy::unwrap_used)] // TODO
                let result_ts = datatype(config, &operation.ty.result, type_store).unwrap();

                // TODO: Specta API
                format!(
                    r#"
        {{ key: "{key}", input: {input}, result: {result_ts} }}"#
                )
            })
            .collect::<Vec<_>>()
            .join(" | "),
    }
}
