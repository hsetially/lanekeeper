//! The effective config of a resource file: the whole file the config-server would choose, rendered (T6, D82, D83).
//!
//! The file is chosen by the engine's rule (`resolve_resource`: the channel folder's copy first, the tenant's copy before the base
//! copy), the application's property view is built from the property sources it reads (`property_view`), and `${KEY}` is replaced
//! where the view has the key (`render_file`). `hash` is the hash of the rendered file and `unresolved_placeholders` are the keys that
//! were left as written, for the service to fill from its own environment. Nothing here reads an environment value.
//!
//! See the module documentation of [`super::effective`] for what is approximated: which application renders the file, and that
//! `channels.yml` is not read here.

use std::collections::BTreeSet;

use domain::{
    AppName, ChannelName, ContentHash, EffectiveConfig, EffectiveSource, FileClass, FileRole, LogicalFile,
    NfsPath, ShortText, SwimlaneId, TenantId,
};
use engine::{
    FlatSetting, PropertyFile, PropertyView, Syntax, property_view, render_file, resolve_resource,
    search_locations,
};
use ports::ReadError;
use tracing::warn;

use super::api::Tagged;
use super::effective::{Group, LayerRow};
use super::etag;
use super::rows;
use super::service::Inner;
use crate::error::RegistryError;

/// Rows of files by exact path, in the shape the layers use.
pub const ROWS_BY_PATH_SQL: &str = "\
SELECT swimlane_id, path, kind, class, nfs_hash::bytea AS blob, nfs_drift_hash::bytea AS norm, COALESCE(size, 0) AS size, denied, false AS in_group \
FROM file_state WHERE swimlane_id = $1 AND path = ANY($2::text[]) AND nfs_hash IS NOT NULL";

/// The property sources of an application and tenant, by file name, in any folder.
pub const PROPERTY_ROWS_SQL: &str = "\
SELECT swimlane_id, path, kind, class, nfs_hash::bytea AS blob, nfs_drift_hash::bytea AS norm, COALESCE(size, 0) AS size, denied, false AS in_group \
FROM file_state \
WHERE swimlane_id = $1 AND class = 'structured' AND NOT denied AND nfs_hash IS NOT NULL \
  AND regexp_replace(path, '^.*/', '') = ANY($2::text[])";

pub const ALL_PATHS_SQL: &str =
    "SELECT path FROM file_state WHERE swimlane_id = $1 AND nfs_hash IS NOT NULL LIMIT 200001";

/// The file names a property view of `(application, tenant)` reads.
pub(crate) fn property_names(app: &AppName, tenant: &TenantId) -> Vec<String> {
    let stems = [
        "application".to_owned(),
        format!("application-{tenant}"),
        app.as_str().to_owned(),
        format!("{app}-{tenant}"),
    ];
    let mut names = Vec::with_capacity(stems.len() * 3);
    for stem in &stems {
        for ext in ["properties", "yml", "yaml"] {
            names.push(format!("{stem}.{ext}"));
        }
    }
    names
}

/// The application whose view renders a file: the folder it is in when that is one of the swimlane's applications, else `application`
/// (the root files alone are the view). `None` only if no valid name exists, which `application` rules out.
pub(crate) fn application_of(file: &LogicalFile, apps: &[AppName]) -> Option<AppName> {
    let path = file.as_str();
    let first = path.split_once('/').map(|(folder, _)| folder);
    first
        .and_then(|folder| apps.iter().find(|a| a.as_str() == folder).cloned())
        .or_else(|| AppName::parse("application").ok())
}

/// The two paths of a file in a channel's folder: the tenant's copy and the base copy.
fn channel_candidates(file: &LogicalFile, tenant: &TenantId, channel: &ChannelName) -> [String; 2] {
    let folder = file.as_path().parent();
    let name = file.as_path().file_name();
    let dir = if folder.is_root() {
        channel.to_string()
    } else {
        format!("{folder}/{channel}")
    };
    let (stem, ext) = name.rsplit_once('.').map_or((name, None), |(s, e)| (s, Some(e)));
    let tenant_name = ext.map_or_else(|| format!("{stem}-{tenant}"), |e| format!("{stem}-{tenant}.{e}"));
    [format!("{dir}/{tenant_name}"), format!("{dir}/{name}")]
}

impl Inner {
    /// Rows of files of a swimlane by exact path.
    pub(crate) async fn layer_rows_by_path(
        &self,
        swimlane: &SwimlaneId,
        paths: &[String],
    ) -> Result<Vec<LayerRow>, ReadError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let fetched = self
            .db
            .run(
                "read_rows_by_path",
                sqlx::query(ROWS_BY_PATH_SQL)
                    .bind(swimlane.as_str())
                    .bind(paths)
                    .fetch_all(self.db.pool()),
            )
            .await?;
        fetched
            .iter()
            .map(|r| super::effective::layer_row(r, &self.deny).map_err(ReadError::from))
            .collect()
    }

    /// The property view of `application` for `tenant` in a swimlane, from the property sources it has.
    async fn view_of(
        &self,
        group: &Group,
        app: &AppName,
        tenant: &TenantId,
        channel: Option<&ChannelName>,
    ) -> Result<PropertyView, ReadError> {
        let all_paths = self.all_paths(&group.swimlane).await?;
        let channels: &[ChannelName] = channel.map_or(&[], std::slice::from_ref);
        let locations = search_locations(app, Some(tenant), &all_paths, channels);
        let fetched = self
            .db
            .run(
                "read_property_rows",
                sqlx::query(PROPERTY_ROWS_SQL)
                    .bind(group.swimlane.as_str())
                    .bind(property_names(app, tenant))
                    .fetch_all(self.db.pool()),
            )
            .await?;
        // A denied file is not a source of the view: its values would be rendered into another file's text (`${KEY}`).
        let property_rows = fetched
            .iter()
            .map(|r| super::effective::layer_row(r, &self.deny).map_err(ReadError::from))
            .filter(|r| r.as_ref().map_or(true, |r| !r.denied))
            .collect::<Result<Vec<_>, _>>()?;
        let wanted: Vec<(ContentHash, Syntax)> = property_rows
            .iter()
            .filter_map(|r| Some((r.blob?, Syntax::of_path(&r.path)?)))
            .collect();
        let blob_rows = self.rows_of(&wanted).await?;
        let empty: Vec<FlatSetting> = Vec::new();
        let files: Vec<PropertyFile<'_>> = property_rows
            .iter()
            .map(|r| PropertyFile {
                path: &r.path,
                settings: r
                    .blob
                    .and_then(|b| blob_rows.get(&b))
                    .filter(|rows| !rows.unparseable)
                    .map_or(empty.as_slice(), |rows| rows.settings.as_slice()),
            })
            .collect();
        Ok(
            property_view(app, Some(tenant), &locations, &files, &self.limits).unwrap_or_else(|_| {
                warn!(
                    "a property view is over its limits: the file is shown with its placeholders as written"
                );
                PropertyView::default()
            }),
        )
    }

    pub(crate) async fn effective_resource(
        &self,
        group: &Group,
        file: &LogicalFile,
        tenant: &TenantId,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<EffectiveConfig>, ReadError> {
        // The candidates the rule looks at: the group (the base file and the tenants' copies) and the channel folder's two.
        let mut candidates: Vec<LayerRow> = group.files.clone();
        if let Some(channel) = channel {
            let extra = channel_candidates(file, tenant, channel);
            candidates.extend(self.layer_rows_by_path(&group.swimlane, &extra).await?);
        }
        let present: BTreeSet<NfsPath> = candidates.iter().map(|r| r.path.clone()).collect();
        let choice = resolve_resource(file, Some(tenant), channel, &present).ok_or(ReadError::NotFound)?;
        let chosen = candidates
            .iter()
            .find(|r| r.path == choice.path)
            .ok_or(ReadError::NotFound)?;
        if chosen.denied || chosen.class == FileClass::Denied {
            return Err(ReadError::NotFound);
        }
        let blob = chosen.blob.ok_or(ReadError::NotFound)?;
        let bytes = self.get_blob(&blob).await?.ok_or(ReadError::NotFound)?;

        let app = application_of(file, &group.apps).ok_or(ReadError::Unavailable)?;
        let view = self.view_of(group, &app, tenant, channel).await?;
        let rendered = render_file(&bytes, chosen.class, &view, &self.limits).map_err(|_| {
            warn!("a resource could not be rendered");
            ReadError::Unavailable
        })?;
        let tag = etag::tag(
            "effective",
            &[
                group.swimlane.as_str(),
                file.as_str(),
                tenant.as_str(),
                channel.map_or("", ChannelName::as_str),
                &rendered.hash.to_string(),
            ],
        );
        if etag::matches(if_none_match, &tag) {
            return Ok(Tagged::NotModified { etag: tag });
        }
        let unresolved = rendered
            .unresolved
            .iter()
            .map(|p| rows::short(&p.key))
            .collect::<Result<Vec<ShortText>, RegistryError>>()?;
        let value = EffectiveConfig {
            swimlane: group.swimlane.clone(),
            file: file.clone(),
            tenant: tenant.clone(),
            channel: channel.cloned(),
            role: FileRole::Resource,
            hash: rendered.hash,
            sources: vec![EffectiveSource {
                path: choice.path,
                rank: 1,
            }],
            settings: Vec::new(),
            unresolved_placeholders: unresolved,
        };
        Ok(Tagged::Fresh { etag: tag, value })
    }

    async fn all_paths(&self, swimlane: &SwimlaneId) -> Result<Vec<NfsPath>, ReadError> {
        let paths: Vec<String> = self
            .db
            .run(
                "read_all_paths",
                sqlx::query_scalar(ALL_PATHS_SQL)
                    .bind(swimlane.as_str())
                    .fetch_all(self.db.pool()),
            )
            .await?;
        Ok(paths.iter().filter_map(|p| NfsPath::parse(p).ok()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_application_is_the_folder_when_it_is_one_the_swimlane_runs_and_otherwise_the_root_files() {
        let apps = vec![AppName::parse("core").unwrap()];
        let file = |p: &str| LogicalFile::parse(p).unwrap();
        assert_eq!(
            application_of(&file("core/msg.txt"), &apps).unwrap().as_str(),
            "core"
        );
        assert_eq!(
            application_of(&file("other/msg.txt"), &apps).unwrap().as_str(),
            "application"
        );
        assert_eq!(
            application_of(&file("core"), &apps).unwrap().as_str(),
            "application",
            "a file in the root is not in a folder"
        );
    }

    #[test]
    fn the_application_depends_only_on_the_candidates_among_the_applications() {
        use proptest::prelude::*;
        let word = prop::sample::select(vec!["core", "pay", "application", "ledger", "x"]);
        let path = (
            prop::collection::vec(word.clone(), 0..4),
            prop::collection::vec(word.clone(), 1..3),
        )
            .prop_map(|(folders, name)| {
                folders
                    .iter()
                    .map(|f| (*f).to_owned())
                    .chain(std::iter::once(format!("{}.yml", name.join("-"))))
                    .collect::<Vec<_>>()
                    .join("/")
            });
        let apps = prop::collection::vec(word.prop_map(str::to_owned), 0..5);
        let mut runner = proptest::test_runner::TestRunner::default();
        runner
            .run(&(path, apps), |(path, apps)| {
                let file = LogicalFile::parse(&path).unwrap();
                let all: Vec<AppName> = apps.iter().filter_map(|a| AppName::parse(a).ok()).collect();
                let candidates = super::super::effective::app_candidates(&file);
                let some: Vec<AppName> = all
                    .iter()
                    .filter(|a| candidates.iter().any(|c| c == a.as_str()))
                    .cloned()
                    .collect();
                prop_assert_eq!(
                    application_of(&file, &all),
                    application_of(&file, &some),
                    "{}",
                    path
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_property_view_reads_twelve_names() {
        let names = property_names(
            &AppName::parse("core").unwrap(),
            &TenantId::parse("sit1").unwrap(),
        );
        assert_eq!(names.len(), 12);
        assert!(names.contains(&"core-sit1.yml".to_owned()));
        assert!(names.contains(&"application-sit1.properties".to_owned()));
    }

    #[test]
    fn the_channel_candidates_are_the_tenant_copy_then_the_base_copy_in_the_channel_folder() {
        let c = channel_candidates(
            &LogicalFile::parse("core/msg.txt").unwrap(),
            &TenantId::parse("sit1").unwrap(),
            &ChannelName::parse("chan").unwrap(),
        );
        assert_eq!(
            c,
            [
                "core/chan/msg-sit1.txt".to_owned(),
                "core/chan/msg.txt".to_owned()
            ]
        );
        let root = channel_candidates(
            &LogicalFile::parse("mappings").unwrap(),
            &TenantId::parse("sit1").unwrap(),
            &ChannelName::parse("chan").unwrap(),
        );
        assert_eq!(
            root,
            ["chan/mappings-sit1".to_owned(), "chan/mappings".to_owned()]
        );
    }
}
