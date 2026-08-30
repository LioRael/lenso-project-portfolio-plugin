use std::collections::{BTreeMap, BTreeSet};

use lenso_postgres_kit::OwnedPostgres;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{Postgres, Row, Transaction};
use thiserror::Error;
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_PROJECTS_PER_INITIATIVE: i64 = 500;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct InitiativeRecord {
    pub initiative_id: String,
    pub organization_id: String,
    pub name: String,
    pub summary: Option<String>,
    pub owner_subject: Option<String>,
    pub target_start: Option<String>,
    pub target_date: Option<String>,
    pub health: String,
    pub progress: i64,
    pub project_count: i64,
    pub archived: bool,
    pub revision: String,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
    pub row_seq: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ProjectRecord {
    pub project_id: String,
    pub name_snapshot: String,
    pub status_category: String,
    pub health: String,
    pub progress: i64,
    pub target_start: Option<String>,
    pub target_date: Option<String>,
    pub snapshot_revision: String,
    pub observed_at: String,
    pub position: i64,
    pub membership_revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiative_revision: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct UpdateRecord {
    pub update_id: String,
    pub initiative_id: String,
    pub organization_id: String,
    pub health: String,
    pub summary: String,
    pub progress: i64,
    pub created_by: String,
    pub initiative_revision: String,
    pub created_at: String,
    pub row_seq: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ArchiveRecord {
    pub initiative_id: String,
    pub organization_id: String,
    pub archived: bool,
    pub revision: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct DetachRecord {
    pub detached: bool,
    pub initiative_revision: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ReorderRecord {
    pub initiative_revision: String,
    pub items: Vec<ReorderItemRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ReorderItemRecord {
    pub project_id: String,
    pub position: i64,
    pub membership_revision: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct RollupRecord {
    pub initiative_id: String,
    pub initiative_revision: String,
    pub health: String,
    pub initiative_progress: i64,
    pub project_count: i64,
    pub completed_count: i64,
    pub at_risk_count: i64,
    pub off_track_count: i64,
    pub average_project_progress: Option<f64>,
    pub earliest_target_start: Option<String>,
    pub latest_target_date: Option<String>,
    pub computed_at: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub(crate) struct InitiativeCreate<'a> {
    pub organization_id: &'a str,
    pub initiative_id: &'a str,
    pub name: &'a str,
    pub summary: Option<&'a str>,
    pub owner_subject: Option<&'a str>,
    pub target_start: Option<Date>,
    pub target_date: Option<Date>,
}

#[derive(Clone, Debug)]
pub(crate) struct InitiativePatch<'a> {
    pub name: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub owner_subject: Option<&'a str>,
    pub target_start: Option<Date>,
    pub target_date: Option<Date>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectSnapshot<'a> {
    pub project_id: &'a str,
    pub name_snapshot: &'a str,
    pub status_category: String,
    pub health: String,
    pub progress: i64,
    pub target_start: Option<Date>,
    pub target_date: Option<Date>,
    pub snapshot_revision: &'a str,
    pub observed_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
pub(crate) struct ReorderInput<'a> {
    pub project_id: &'a str,
    pub expected_membership_revision: i64,
    pub position: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DomainFailure {
    InvalidRequest,
    NotFound,
    Archived,
    RevisionConflict,
    IdempotencyConflict,
    OperationInProgress,
    AlreadyExists,
    AlreadyAttached,
    NotAttached,
    PositionConflict,
}

#[derive(Debug, Error)]
pub(crate) enum StorageError {
    #[error("domain failure: {0:?}")]
    Domain(DomainFailure),
    #[error("database failure during {operation}: {source}")]
    Database {
        operation: &'static str,
        #[source]
        source: sqlx::Error,
    },
    #[error("failed to encode or decode command receipt: {0}")]
    Receipt(#[from] serde_json::Error),
    #[error("failed to format a timestamp: {0}")]
    Time(#[from] time::error::Format),
}

impl From<DomainFailure> for StorageError {
    fn from(value: DomainFailure) -> Self {
        Self::Domain(value)
    }
}

pub(crate) async fn create_initiative(
    postgres: &OwnedPostgres,
    caller: &str,
    actor: &str,
    idempotency_key: &str,
    request_hash: &[u8],
    value: &InitiativeCreate<'_>,
) -> Result<InitiativeRecord, StorageError> {
    let mut tx = begin(postgres, "begin create initiative").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        caller,
        actor,
        "create_initiative",
        idempotency_key,
        request_hash,
    )
    .await?
    {
        commit(tx, "commit create replay").await?;
        return Ok(replay);
    }
    let inserted = sqlx::query(
        "INSERT INTO portfolio_initiatives(organization_id,initiative_id,name,summary,owner_subject,target_start,target_date) VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(value.organization_id)
    .bind(value.initiative_id)
    .bind(value.name)
    .bind(value.summary)
    .bind(value.owner_subject)
    .bind(value.target_start)
    .bind(value.target_date)
    .execute(&mut *tx)
    .await;
    if let Err(source) = inserted {
        if unique_violation(&source) {
            return Err(DomainFailure::AlreadyExists.into());
        }
        return Err(database("insert initiative", source));
    }
    let record = read_initiative_tx(&mut tx, value.organization_id, value.initiative_id).await?;
    finish_command(
        &mut tx,
        caller,
        actor,
        "create_initiative",
        idempotency_key,
        &record,
    )
    .await?;
    commit(tx, "commit create initiative").await?;
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn update_initiative(
    postgres: &OwnedPostgres,
    caller: &str,
    actor: &str,
    idempotency_key: &str,
    request_hash: &[u8],
    organization_id: &str,
    initiative_id: &str,
    expected_revision: i64,
    patch: &InitiativePatch<'_>,
) -> Result<InitiativeRecord, StorageError> {
    let mut tx = begin(postgres, "begin update initiative").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        caller,
        actor,
        "update_initiative",
        idempotency_key,
        request_hash,
    )
    .await?
    {
        commit(tx, "commit update replay").await?;
        return Ok(replay);
    }
    let current = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    require_mutable_revision(&current, expected_revision)?;
    sqlx::query(
        "UPDATE portfolio_initiatives SET name=COALESCE($3,name),summary=$4,owner_subject=$5,target_start=$6,target_date=$7,revision=revision+1,updated_at=clock_timestamp() WHERE organization_id=$1 AND initiative_id=$2",
    )
    .bind(organization_id)
    .bind(initiative_id)
    .bind(patch.name)
    .bind(patch.summary)
    .bind(patch.owner_subject)
    .bind(patch.target_start)
    .bind(patch.target_date)
    .execute(&mut *tx)
    .await
    .map_err(|source| database("update initiative", source))?;
    let record = read_initiative_tx(&mut tx, organization_id, initiative_id).await?;
    finish_command(
        &mut tx,
        caller,
        actor,
        "update_initiative",
        idempotency_key,
        &record,
    )
    .await?;
    commit(tx, "commit update initiative").await?;
    Ok(record)
}

pub(crate) async fn get_initiative(
    postgres: &OwnedPostgres,
    organization_id: &str,
    initiative_id: &str,
) -> Result<InitiativeRecord, StorageError> {
    let row = sqlx::query(
        "SELECT i.*,(SELECT COUNT(*) FROM portfolio_projects p WHERE p.organization_id=i.organization_id AND p.initiative_id=i.initiative_id) project_count FROM portfolio_initiatives i WHERE i.organization_id=$1 AND i.initiative_id=$2",
    )
    .bind(organization_id)
    .bind(initiative_id)
    .fetch_optional(postgres.pool())
    .await
    .map_err(|source| database("read initiative", source))?
    .ok_or(DomainFailure::NotFound)?;
    initiative_from_row(&row)
}

pub(crate) async fn list_initiatives(
    postgres: &OwnedPostgres,
    organization_id: &str,
    include_archived: bool,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<InitiativeRecord>, StorageError> {
    let rows = sqlx::query(
        "SELECT i.*,(SELECT COUNT(*) FROM portfolio_projects p WHERE p.organization_id=i.organization_id AND p.initiative_id=i.initiative_id) project_count FROM portfolio_initiatives i WHERE i.organization_id=$1 AND ($2 OR NOT i.archived) AND i.row_seq>$3 ORDER BY i.row_seq LIMIT $4",
    )
    .bind(organization_id)
    .bind(include_archived)
    .bind(after.unwrap_or(0))
    .bind(limit)
    .fetch_all(postgres.pool())
    .await
    .map_err(|source| database("list initiatives", source))?;
    rows.iter().map(initiative_from_row).collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_update(
    postgres: &OwnedPostgres,
    caller: &str,
    actor: &str,
    idempotency_key: &str,
    request_hash: &[u8],
    organization_id: &str,
    initiative_id: &str,
    update_id: &str,
    expected_revision: i64,
    health: &str,
    summary: &str,
    progress: i64,
) -> Result<UpdateRecord, StorageError> {
    let mut tx = begin(postgres, "begin initiative update").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        caller,
        actor,
        "add_initiative_update",
        idempotency_key,
        request_hash,
    )
    .await?
    {
        commit(tx, "commit initiative update replay").await?;
        return Ok(replay);
    }
    let current = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    require_mutable_revision(&current, expected_revision)?;
    let next_revision = current.revision + 1;
    let inserted = sqlx::query(
        "INSERT INTO portfolio_updates(organization_id,initiative_id,update_id,health,summary,progress,created_by,initiative_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(organization_id)
    .bind(initiative_id)
    .bind(update_id)
    .bind(health)
    .bind(summary)
    .bind(progress)
    .bind(actor)
    .bind(next_revision)
    .execute(&mut *tx)
    .await;
    if let Err(source) = inserted {
        if unique_violation(&source) {
            return Err(DomainFailure::AlreadyExists.into());
        }
        return Err(database("insert initiative update", source));
    }
    sqlx::query("UPDATE portfolio_initiatives SET health=$3,progress=$4,revision=$5,updated_at=clock_timestamp() WHERE organization_id=$1 AND initiative_id=$2")
        .bind(organization_id).bind(initiative_id).bind(health).bind(progress).bind(next_revision)
        .execute(&mut *tx).await.map_err(|source| database("advance initiative update", source))?;
    let row =
        sqlx::query("SELECT * FROM portfolio_updates WHERE organization_id=$1 AND update_id=$2")
            .bind(organization_id)
            .bind(update_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|source| database("read initiative update", source))?;
    let record = update_from_row(&row)?;
    finish_command(
        &mut tx,
        caller,
        actor,
        "add_initiative_update",
        idempotency_key,
        &record,
    )
    .await?;
    commit(tx, "commit initiative update").await?;
    Ok(record)
}

pub(crate) async fn list_updates(
    postgres: &OwnedPostgres,
    organization_id: &str,
    initiative_id: &str,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<UpdateRecord>, StorageError> {
    ensure_exists(postgres, organization_id, initiative_id).await?;
    let rows = sqlx::query("SELECT * FROM portfolio_updates WHERE organization_id=$1 AND initiative_id=$2 AND row_seq>$3 ORDER BY row_seq LIMIT $4")
        .bind(organization_id).bind(initiative_id).bind(after.unwrap_or(0)).bind(limit)
        .fetch_all(postgres.pool()).await.map_err(|source| database("list initiative updates", source))?;
    rows.iter().map(update_from_row).collect()
}

pub(crate) async fn list_projects(
    postgres: &OwnedPostgres,
    organization_id: &str,
    initiative_id: &str,
    after_position: Option<i64>,
    limit: i64,
) -> Result<Vec<ProjectRecord>, StorageError> {
    ensure_exists(postgres, organization_id, initiative_id).await?;
    let rows = sqlx::query("SELECT * FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND position>$3 ORDER BY position,project_id LIMIT $4")
        .bind(organization_id).bind(initiative_id).bind(after_position.unwrap_or(-1)).bind(limit)
        .fetch_all(postgres.pool()).await.map_err(|source| database("list initiative projects", source))?;
    rows.iter().map(|row| project_from_row(row, None)).collect()
}

pub(crate) async fn read_rollup(
    postgres: &OwnedPostgres,
    organization_id: &str,
    initiative_id: &str,
) -> Result<RollupRecord, StorageError> {
    let initiative = get_initiative(postgres, organization_id, initiative_id).await?;
    let projects = list_projects(
        postgres,
        organization_id,
        initiative_id,
        None,
        MAX_PROJECTS_PER_INITIATIVE,
    )
    .await?;
    compute_rollup(&initiative, &projects, OffsetDateTime::now_utc())
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn compute_rollup(
    initiative: &InitiativeRecord,
    projects: &[ProjectRecord],
    computed_at: OffsetDateTime,
) -> Result<RollupRecord, StorageError> {
    let project_count = i64::try_from(projects.len()).map_err(|_| DomainFailure::InvalidRequest)?;
    let completed_count = count(projects, |project| project.status_category == "completed")?;
    let at_risk_count = count(projects, |project| project.health == "at_risk")?;
    let off_track_count = count(projects, |project| project.health == "off_track")?;
    let average_project_progress = if projects.is_empty() {
        None
    } else {
        let total: i64 = projects.iter().map(|project| project.progress).sum();
        Some(total as f64 / projects.len() as f64)
    };
    let earliest_target_start = projects
        .iter()
        .filter_map(|project| project.target_start.as_ref())
        .min()
        .cloned();
    let latest_target_date = projects
        .iter()
        .filter_map(|project| project.target_date.as_ref())
        .max()
        .cloned();
    Ok(RollupRecord {
        initiative_id: initiative.initiative_id.clone(),
        initiative_revision: initiative.revision.clone(),
        health: initiative.health.clone(),
        initiative_progress: initiative.progress,
        project_count,
        completed_count,
        at_risk_count,
        off_track_count,
        average_project_progress,
        earliest_target_start,
        latest_target_date,
        computed_at: format_timestamp(computed_at)?,
        source: "owned_project_snapshots".to_owned(),
    })
}

pub(crate) async fn archive_initiative(
    postgres: &OwnedPostgres,
    command: Command<'_>,
    organization_id: &str,
    initiative_id: &str,
    expected_revision: i64,
) -> Result<ArchiveRecord, StorageError> {
    let mut tx = begin(postgres, "begin archive initiative").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        command.caller,
        command.actor,
        "archive_initiative",
        command.key,
        command.hash,
    )
    .await?
    {
        commit(tx, "commit archive replay").await?;
        return Ok(replay);
    }
    let current = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    require_mutable_revision(&current, expected_revision)?;
    let row = sqlx::query("UPDATE portfolio_initiatives SET archived=true,archived_at=clock_timestamp(),updated_at=clock_timestamp(),revision=revision+1 WHERE organization_id=$1 AND initiative_id=$2 RETURNING initiative_id,organization_id,archived,revision,updated_at,archived_at")
        .bind(organization_id).bind(initiative_id).fetch_one(&mut *tx).await
        .map_err(|source| database("archive initiative", source))?;
    let record = ArchiveRecord {
        initiative_id: row
            .try_get("initiative_id")
            .map_err(|source| database("decode archived initiative", source))?,
        organization_id: row
            .try_get("organization_id")
            .map_err(|source| database("decode archived initiative", source))?,
        archived: row
            .try_get("archived")
            .map_err(|source| database("decode archived initiative", source))?,
        revision: row
            .try_get::<i64, _>("revision")
            .map_err(|source| database("decode archived initiative", source))?
            .to_string(),
        updated_at: format_timestamp(
            row.try_get("updated_at")
                .map_err(|source| database("decode archived initiative", source))?,
        )?,
        archived_at: optional_timestamp(
            row.try_get("archived_at")
                .map_err(|source| database("decode archived initiative", source))?,
        )?,
    };
    finish_command(
        &mut tx,
        command.caller,
        command.actor,
        "archive_initiative",
        command.key,
        &record,
    )
    .await?;
    commit(tx, "commit archive initiative").await?;
    Ok(record)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Command<'a> {
    pub caller: &'a str,
    pub actor: &'a str,
    pub key: &'a str,
    pub hash: &'a [u8],
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn attach_project(
    postgres: &OwnedPostgres,
    command: Command<'_>,
    organization_id: &str,
    initiative_id: &str,
    expected_initiative_revision: i64,
    snapshot: &ProjectSnapshot<'_>,
    position: i64,
) -> Result<ProjectRecord, StorageError> {
    let mut tx = begin(postgres, "begin attach project").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        command.caller,
        command.actor,
        "attach_project",
        command.key,
        command.hash,
    )
    .await?
    {
        commit(tx, "commit attach replay").await?;
        return Ok(replay);
    }
    let initiative = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    require_mutable_revision(&initiative, expected_initiative_revision)?;
    let existing = sqlx::query("SELECT position FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3")
        .bind(organization_id).bind(initiative_id).bind(snapshot.project_id).fetch_optional(&mut *tx).await
        .map_err(|source| database("check project attachment", source))?;
    if existing.is_some() {
        return Err(DomainFailure::AlreadyAttached.into());
    }
    let project_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2",
    )
    .bind(organization_id)
    .bind(initiative_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|source| database("count project attachments", source))?;
    if project_count >= MAX_PROJECTS_PER_INITIATIVE {
        return Err(DomainFailure::InvalidRequest.into());
    }
    let inserted = sqlx::query("INSERT INTO portfolio_projects(organization_id,initiative_id,project_id,name_snapshot,status_category,health,progress,target_start,target_date,snapshot_revision,observed_at,position) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(organization_id).bind(initiative_id).bind(snapshot.project_id).bind(snapshot.name_snapshot)
        .bind(&snapshot.status_category).bind(&snapshot.health).bind(snapshot.progress).bind(snapshot.target_start)
        .bind(snapshot.target_date).bind(snapshot.snapshot_revision).bind(snapshot.observed_at).bind(position)
        .execute(&mut *tx).await;
    if let Err(source) = inserted {
        if unique_violation(&source) {
            return Err(DomainFailure::PositionConflict.into());
        }
        return Err(database("attach project", source));
    }
    let initiative_revision = bump_initiative(&mut tx, organization_id, initiative_id).await?;
    let row = sqlx::query("SELECT * FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3")
        .bind(organization_id).bind(initiative_id).bind(snapshot.project_id).fetch_one(&mut *tx).await
        .map_err(|source| database("read attached project", source))?;
    let record = project_from_row(&row, Some(initiative_revision.to_string()))?;
    finish_command(
        &mut tx,
        command.caller,
        command.actor,
        "attach_project",
        command.key,
        &record,
    )
    .await?;
    commit(tx, "commit attach project").await?;
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn update_project_snapshot(
    postgres: &OwnedPostgres,
    command: Command<'_>,
    organization_id: &str,
    initiative_id: &str,
    expected_membership_revision: i64,
    snapshot: &ProjectSnapshot<'_>,
) -> Result<ProjectRecord, StorageError> {
    let mut tx = begin(postgres, "begin project snapshot").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        command.caller,
        command.actor,
        "update_project_snapshot",
        command.key,
        command.hash,
    )
    .await?
    {
        commit(tx, "commit snapshot replay").await?;
        return Ok(replay);
    }
    let initiative = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    if initiative.archived {
        return Err(DomainFailure::Archived.into());
    }
    let row = sqlx::query("SELECT membership_revision FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3 FOR UPDATE")
        .bind(organization_id).bind(initiative_id).bind(snapshot.project_id).fetch_optional(&mut *tx).await
        .map_err(|source| database("lock project snapshot", source))?.ok_or(DomainFailure::NotAttached)?;
    let revision: i64 = row
        .try_get("membership_revision")
        .map_err(|source| database("decode membership revision", source))?;
    if revision != expected_membership_revision {
        return Err(DomainFailure::RevisionConflict.into());
    }
    sqlx::query("UPDATE portfolio_projects SET name_snapshot=$4,status_category=$5,health=$6,progress=$7,target_start=$8,target_date=$9,snapshot_revision=$10,observed_at=$11,membership_revision=membership_revision+1 WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3")
        .bind(organization_id).bind(initiative_id).bind(snapshot.project_id).bind(snapshot.name_snapshot)
        .bind(&snapshot.status_category).bind(&snapshot.health).bind(snapshot.progress).bind(snapshot.target_start)
        .bind(snapshot.target_date).bind(snapshot.snapshot_revision).bind(snapshot.observed_at)
        .execute(&mut *tx).await.map_err(|source| database("update project snapshot", source))?;
    let initiative_revision = bump_initiative(&mut tx, organization_id, initiative_id).await?;
    let row = sqlx::query("SELECT * FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3")
        .bind(organization_id).bind(initiative_id).bind(snapshot.project_id).fetch_one(&mut *tx).await
        .map_err(|source| database("read project snapshot", source))?;
    let record = project_from_row(&row, Some(initiative_revision.to_string()))?;
    finish_command(
        &mut tx,
        command.caller,
        command.actor,
        "update_project_snapshot",
        command.key,
        &record,
    )
    .await?;
    commit(tx, "commit project snapshot").await?;
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn detach_project(
    postgres: &OwnedPostgres,
    command: Command<'_>,
    organization_id: &str,
    initiative_id: &str,
    project_id: &str,
    expected_initiative_revision: i64,
    expected_membership_revision: i64,
) -> Result<DetachRecord, StorageError> {
    let mut tx = begin(postgres, "begin detach project").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        command.caller,
        command.actor,
        "detach_project",
        command.key,
        command.hash,
    )
    .await?
    {
        commit(tx, "commit detach replay").await?;
        return Ok(replay);
    }
    let initiative = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    require_mutable_revision(&initiative, expected_initiative_revision)?;
    let deleted = sqlx::query("DELETE FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3 AND membership_revision=$4")
        .bind(organization_id).bind(initiative_id).bind(project_id).bind(expected_membership_revision)
        .execute(&mut *tx).await.map_err(|source| database("detach project", source))?;
    if deleted.rows_affected() == 0 {
        let exists = sqlx::query("SELECT 1 FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3")
            .bind(organization_id).bind(initiative_id).bind(project_id).fetch_optional(&mut *tx).await
            .map_err(|source| database("check detached project", source))?;
        return Err(if exists.is_some() {
            DomainFailure::RevisionConflict
        } else {
            DomainFailure::NotAttached
        }
        .into());
    }
    let revision = bump_initiative(&mut tx, organization_id, initiative_id).await?;
    let record = DetachRecord {
        detached: true,
        initiative_revision: revision.to_string(),
    };
    finish_command(
        &mut tx,
        command.caller,
        command.actor,
        "detach_project",
        command.key,
        &record,
    )
    .await?;
    commit(tx, "commit detach project").await?;
    Ok(record)
}

pub(crate) async fn reorder_projects(
    postgres: &OwnedPostgres,
    command: Command<'_>,
    organization_id: &str,
    initiative_id: &str,
    expected_initiative_revision: i64,
    items: &[ReorderInput<'_>],
) -> Result<ReorderRecord, StorageError> {
    let mut tx = begin(postgres, "begin reorder projects").await?;
    if let Some(replay) = admit_command(
        &mut tx,
        command.caller,
        command.actor,
        "reorder_projects",
        command.key,
        command.hash,
    )
    .await?
    {
        commit(tx, "commit reorder replay").await?;
        return Ok(replay);
    }
    let initiative = lock_initiative(&mut tx, organization_id, initiative_id).await?;
    require_mutable_revision(&initiative, expected_initiative_revision)?;
    let rows = sqlx::query("SELECT project_id,membership_revision FROM portfolio_projects WHERE organization_id=$1 AND initiative_id=$2 ORDER BY project_id FOR UPDATE")
        .bind(organization_id).bind(initiative_id).fetch_all(&mut *tx).await
        .map_err(|source| database("lock project order", source))?;
    let current = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<String, _>("project_id")
                    .map_err(|source| database("decode project order", source))?,
                row.try_get::<i64, _>("membership_revision")
                    .map_err(|source| database("decode project order", source))?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, StorageError>>()?;
    let requested_ids = items
        .iter()
        .map(|item| item.project_id)
        .collect::<BTreeSet<_>>();
    let requested_positions = items
        .iter()
        .map(|item| item.position)
        .collect::<BTreeSet<_>>();
    if items.is_empty()
        || i64::try_from(items.len()).map_or(true, |count| count > MAX_PROJECTS_PER_INITIATIVE)
        || requested_ids.len() != items.len()
        || requested_positions.len() != items.len()
        || current.len() != items.len()
        || !items
            .iter()
            .all(|item| current.get(item.project_id) == Some(&item.expected_membership_revision))
    {
        return Err(DomainFailure::InvalidRequest.into());
    }
    sqlx::query("UPDATE portfolio_projects SET position=position+1000000 WHERE organization_id=$1 AND initiative_id=$2")
        .bind(organization_id).bind(initiative_id).execute(&mut *tx).await
        .map_err(|source| database("stage project order", source))?;
    let mut output = Vec::with_capacity(items.len());
    for item in items {
        let row = sqlx::query("UPDATE portfolio_projects SET position=$4,membership_revision=membership_revision+1 WHERE organization_id=$1 AND initiative_id=$2 AND project_id=$3 RETURNING membership_revision")
            .bind(organization_id).bind(initiative_id).bind(item.project_id).bind(item.position)
            .fetch_one(&mut *tx).await.map_err(|source| database("write project order", source))?;
        let membership_revision: i64 = row
            .try_get("membership_revision")
            .map_err(|source| database("decode project order", source))?;
        output.push(ReorderItemRecord {
            project_id: item.project_id.to_owned(),
            position: item.position,
            membership_revision: membership_revision.to_string(),
        });
    }
    output.sort_by_key(|item| item.position);
    let revision = bump_initiative(&mut tx, organization_id, initiative_id).await?;
    let record = ReorderRecord {
        initiative_revision: revision.to_string(),
        items: output,
    };
    finish_command(
        &mut tx,
        command.caller,
        command.actor,
        "reorder_projects",
        command.key,
        &record,
    )
    .await?;
    commit(tx, "commit reorder projects").await?;
    Ok(record)
}

#[derive(Clone, Copy, Debug)]
struct LockedInitiative {
    revision: i64,
    archived: bool,
}

async fn lock_initiative(
    tx: &mut Transaction<'_, Postgres>,
    organization_id: &str,
    initiative_id: &str,
) -> Result<LockedInitiative, StorageError> {
    let row = sqlx::query("SELECT revision,archived FROM portfolio_initiatives WHERE organization_id=$1 AND initiative_id=$2 FOR UPDATE")
        .bind(organization_id).bind(initiative_id).fetch_optional(&mut **tx).await
        .map_err(|source| database("lock initiative", source))?.ok_or(DomainFailure::NotFound)?;
    Ok(LockedInitiative {
        revision: row
            .try_get("revision")
            .map_err(|source| database("decode initiative lock", source))?,
        archived: row
            .try_get("archived")
            .map_err(|source| database("decode initiative lock", source))?,
    })
}

fn require_mutable_revision(current: &LockedInitiative, expected: i64) -> Result<(), StorageError> {
    if current.archived {
        return Err(DomainFailure::Archived.into());
    }
    if current.revision != expected {
        return Err(DomainFailure::RevisionConflict.into());
    }
    Ok(())
}

async fn bump_initiative(
    tx: &mut Transaction<'_, Postgres>,
    organization_id: &str,
    initiative_id: &str,
) -> Result<i64, StorageError> {
    let row = sqlx::query("UPDATE portfolio_initiatives SET revision=revision+1,updated_at=clock_timestamp() WHERE organization_id=$1 AND initiative_id=$2 RETURNING revision")
        .bind(organization_id).bind(initiative_id).fetch_one(&mut **tx).await
        .map_err(|source| database("bump initiative revision", source))?;
    row.try_get("revision")
        .map_err(|source| database("decode initiative revision", source))
}

async fn read_initiative_tx(
    tx: &mut Transaction<'_, Postgres>,
    organization_id: &str,
    initiative_id: &str,
) -> Result<InitiativeRecord, StorageError> {
    let row = sqlx::query("SELECT i.*,(SELECT COUNT(*) FROM portfolio_projects p WHERE p.organization_id=i.organization_id AND p.initiative_id=i.initiative_id) project_count FROM portfolio_initiatives i WHERE i.organization_id=$1 AND i.initiative_id=$2")
        .bind(organization_id).bind(initiative_id).fetch_optional(&mut **tx).await
        .map_err(|source| database("read initiative", source))?.ok_or(DomainFailure::NotFound)?;
    initiative_from_row(&row)
}

async fn ensure_exists(
    postgres: &OwnedPostgres,
    organization_id: &str,
    initiative_id: &str,
) -> Result<(), StorageError> {
    sqlx::query(
        "SELECT 1 FROM portfolio_initiatives WHERE organization_id=$1 AND initiative_id=$2",
    )
    .bind(organization_id)
    .bind(initiative_id)
    .fetch_optional(postgres.pool())
    .await
    .map_err(|source| database("check initiative", source))?
    .ok_or(DomainFailure::NotFound)?;
    Ok(())
}

async fn begin<'a>(
    postgres: &'a OwnedPostgres,
    operation: &'static str,
) -> Result<Transaction<'a, Postgres>, StorageError> {
    postgres
        .pool()
        .begin()
        .await
        .map_err(|source| database(operation, source))
}

async fn commit(
    tx: Transaction<'_, Postgres>,
    operation: &'static str,
) -> Result<(), StorageError> {
    tx.commit()
        .await
        .map_err(|source| database(operation, source))
}

async fn admit_command<T: DeserializeOwned>(
    tx: &mut Transaction<'_, Postgres>,
    caller: &str,
    actor: &str,
    operation: &str,
    key: &str,
    hash: &[u8],
) -> Result<Option<T>, StorageError> {
    let existing = sqlx::query("SELECT request_hash,response_json FROM portfolio_commands WHERE caller_instance=$1 AND actor_subject=$2 AND operation=$3 AND idempotency_key=$4 FOR UPDATE")
        .bind(caller).bind(actor).bind(operation).bind(key).fetch_optional(&mut **tx).await
        .map_err(|source| database("read command receipt", source))?;
    if let Some(row) = existing {
        let stored_hash: Vec<u8> = row
            .try_get("request_hash")
            .map_err(|source| database("decode command receipt", source))?;
        if stored_hash != hash {
            return Err(DomainFailure::IdempotencyConflict.into());
        }
        let response: Option<serde_json::Value> = row
            .try_get("response_json")
            .map_err(|source| database("decode command receipt", source))?;
        let Some(response) = response else {
            return Err(DomainFailure::OperationInProgress.into());
        };
        return Ok(Some(serde_json::from_value(response)?));
    }
    sqlx::query("INSERT INTO portfolio_commands(caller_instance,actor_subject,operation,idempotency_key,request_hash) VALUES($1,$2,$3,$4,$5)")
        .bind(caller).bind(actor).bind(operation).bind(key).bind(hash).execute(&mut **tx).await
        .map_err(|source| if unique_violation(&source) { StorageError::Domain(DomainFailure::OperationInProgress) } else { database("insert command receipt", source) })?;
    Ok(None)
}

async fn finish_command<T: Serialize>(
    tx: &mut Transaction<'_, Postgres>,
    caller: &str,
    actor: &str,
    operation: &str,
    key: &str,
    response: &T,
) -> Result<(), StorageError> {
    let json = serde_json::to_value(response)?;
    sqlx::query("UPDATE portfolio_commands SET response_json=$5 WHERE caller_instance=$1 AND actor_subject=$2 AND operation=$3 AND idempotency_key=$4")
        .bind(caller).bind(actor).bind(operation).bind(key).bind(json).execute(&mut **tx).await
        .map_err(|source| database("finish command receipt", source))?;
    Ok(())
}

fn initiative_from_row(row: &sqlx::postgres::PgRow) -> Result<InitiativeRecord, StorageError> {
    Ok(InitiativeRecord {
        initiative_id: decode(row, "initiative_id", "decode initiative")?,
        organization_id: decode(row, "organization_id", "decode initiative")?,
        name: decode(row, "name", "decode initiative")?,
        summary: decode(row, "summary", "decode initiative")?,
        owner_subject: decode(row, "owner_subject", "decode initiative")?,
        target_start: optional_date(decode(row, "target_start", "decode initiative")?),
        target_date: optional_date(decode(row, "target_date", "decode initiative")?),
        health: decode(row, "health", "decode initiative")?,
        progress: decode::<i16>(row, "progress", "decode initiative")?.into(),
        project_count: decode(row, "project_count", "decode initiative")?,
        archived: decode(row, "archived", "decode initiative")?,
        revision: decode::<i64>(row, "revision", "decode initiative")?.to_string(),
        created_at: format_timestamp(decode(row, "created_at", "decode initiative")?)?,
        updated_at: format_timestamp(decode(row, "updated_at", "decode initiative")?)?,
        archived_at: optional_timestamp(decode(row, "archived_at", "decode initiative")?)?,
        row_seq: decode(row, "row_seq", "decode initiative")?,
    })
}

fn project_from_row(
    row: &sqlx::postgres::PgRow,
    initiative_revision: Option<String>,
) -> Result<ProjectRecord, StorageError> {
    Ok(ProjectRecord {
        project_id: decode(row, "project_id", "decode project membership")?,
        name_snapshot: decode(row, "name_snapshot", "decode project membership")?,
        status_category: decode(row, "status_category", "decode project membership")?,
        health: decode(row, "health", "decode project membership")?,
        progress: decode::<i16>(row, "progress", "decode project membership")?.into(),
        target_start: optional_date(decode(row, "target_start", "decode project membership")?),
        target_date: optional_date(decode(row, "target_date", "decode project membership")?),
        snapshot_revision: decode(row, "snapshot_revision", "decode project membership")?,
        observed_at: format_timestamp(decode(row, "observed_at", "decode project membership")?)?,
        position: decode::<i32>(row, "position", "decode project membership")?.into(),
        membership_revision: decode::<i64>(
            row,
            "membership_revision",
            "decode project membership",
        )?
        .to_string(),
        initiative_revision,
    })
}

fn update_from_row(row: &sqlx::postgres::PgRow) -> Result<UpdateRecord, StorageError> {
    Ok(UpdateRecord {
        update_id: decode(row, "update_id", "decode initiative update")?,
        initiative_id: decode(row, "initiative_id", "decode initiative update")?,
        organization_id: decode(row, "organization_id", "decode initiative update")?,
        health: decode(row, "health", "decode initiative update")?,
        summary: decode(row, "summary", "decode initiative update")?,
        progress: decode::<i16>(row, "progress", "decode initiative update")?.into(),
        created_by: decode(row, "created_by", "decode initiative update")?,
        initiative_revision: decode::<i64>(row, "initiative_revision", "decode initiative update")?
            .to_string(),
        created_at: format_timestamp(decode(row, "created_at", "decode initiative update")?)?,
        row_seq: decode(row, "row_seq", "decode initiative update")?,
    })
}

fn decode<T>(
    row: &sqlx::postgres::PgRow,
    column: &'static str,
    operation: &'static str,
) -> Result<T, StorageError>
where
    for<'r> T: sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(column)
        .map_err(|source| database(operation, source))
}

fn optional_date(value: Option<Date>) -> Option<String> {
    value.map(|date| date.to_string())
}

fn optional_timestamp(value: Option<OffsetDateTime>) -> Result<Option<String>, StorageError> {
    value.map(format_timestamp).transpose()
}

fn format_timestamp(value: OffsetDateTime) -> Result<String, StorageError> {
    Ok(value.format(&Rfc3339)?)
}

fn count(
    projects: &[ProjectRecord],
    predicate: impl Fn(&ProjectRecord) -> bool,
) -> Result<i64, StorageError> {
    i64::try_from(projects.iter().filter(|project| predicate(project)).count())
        .map_err(|_| DomainFailure::InvalidRequest.into())
}

fn unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .as_deref()
        == Some("23505")
}

fn database(operation: &'static str, source: sqlx::Error) -> StorageError {
    StorageError::Database { operation, source }
}
