use lenso_postgres_kit::OwnedPostgres;
use sqlx::{AssertSqlSafe, Executor as _};
use time::{Date, Month, OffsetDateTime};
use uuid::Uuid;

use crate::{ProjectPortfolioOperator, schema, storage};

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn restart_idempotency_cas_snapshot_rollup_and_reorder_are_durable() {
    let Ok(database_url) = std::env::var("LENSO_PROJECT_PORTFOLIO_TEST_DATABASE_URL") else {
        return;
    };
    let database_name = database_url
        .split('?')
        .next()
        .and_then(|value| value.rsplit('/').next())
        .unwrap_or_default();
    assert!(
        database_name.starts_with("lenso_project_portfolio_test"),
        "acceptance requires a dedicated lenso_project_portfolio_test database"
    );
    let schema_name = format!("portfolio_test_{}", Uuid::new_v4().simple());
    ProjectPortfolioOperator::setup(&database_url, &schema_name)
        .await
        .unwrap();
    let postgres = OwnedPostgres::prepare(
        &database_url,
        schema::schema_plan(schema_name.clone()).unwrap(),
    )
    .await
    .unwrap();
    let create = storage::InitiativeCreate {
        organization_id: "org",
        initiative_id: "initiative_launch",
        name: "Launch",
        summary: Some("Ship the product"),
        owner_subject: Some("usr_lead"),
        target_start: Some(Date::from_calendar_date(2026, Month::August, 1).unwrap()),
        target_date: Some(Date::from_calendar_date(2026, Month::September, 30).unwrap()),
    };
    let created = storage::create_initiative(
        &postgres,
        "portfolio-api",
        "usr_lead",
        "create-1",
        &[1],
        &create,
    )
    .await
    .unwrap();
    assert_eq!(created.revision, "1");
    assert_eq!(
        storage::create_initiative(
            &postgres,
            "portfolio-api",
            "usr_lead",
            "create-1",
            &[1],
            &create,
        )
        .await
        .unwrap(),
        created
    );
    assert!(matches!(
        storage::create_initiative(
            &postgres,
            "portfolio-api",
            "usr_lead",
            "create-1",
            &[2],
            &create,
        )
        .await,
        Err(storage::StorageError::Domain(
            storage::DomainFailure::IdempotencyConflict
        ))
    ));
    postgres.pool().close().await;

    let restarted = OwnedPostgres::prepare(
        &database_url,
        schema::schema_plan(schema_name.clone()).unwrap(),
    )
    .await
    .unwrap();
    let snapshot = storage::ProjectSnapshot {
        project_id: "opaque/project/123",
        name_snapshot: "Backend",
        status_category: "started".to_owned(),
        health: "at_risk".to_owned(),
        progress: 40,
        target_start: None,
        target_date: Some(Date::from_calendar_date(2026, Month::September, 15).unwrap()),
        snapshot_revision: "project-rev-17",
        observed_at: OffsetDateTime::now_utc(),
    };
    let attached = storage::attach_project(
        &restarted,
        storage::Command {
            caller: "portfolio-admin",
            actor: "usr_lead",
            key: "attach-1",
            hash: &[3],
        },
        "org",
        "initiative_launch",
        1,
        &snapshot,
        0,
    )
    .await
    .unwrap();
    assert_eq!(attached.project_id, "opaque/project/123");
    assert_eq!(attached.initiative_revision.as_deref(), Some("2"));
    let rollup = storage::read_rollup(&restarted, "org", "initiative_launch")
        .await
        .unwrap();
    assert_eq!(rollup.project_count, 1);
    assert_eq!(rollup.at_risk_count, 1);
    assert_eq!(rollup.average_project_progress, Some(40.0));

    let patch = storage::InitiativePatch {
        name: Some("Launch now"),
        summary: Some("Ship safely"),
        owner_subject: Some("usr_lead"),
        target_start: None,
        target_date: None,
    };
    let first = storage::update_initiative(
        &restarted,
        "portfolio-api",
        "usr_lead",
        "update-a",
        &[4],
        "org",
        "initiative_launch",
        2,
        &patch,
    );
    let second = storage::update_initiative(
        &restarted,
        "portfolio-api",
        "usr_lead",
        "update-b",
        &[5],
        "org",
        "initiative_launch",
        2,
        &patch,
    );
    let (first, second) = tokio::join!(first, second);
    let outcomes = [first, second];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(
                result,
                Err(storage::StorageError::Domain(
                    storage::DomainFailure::RevisionConflict
                ))
            ))
            .count(),
        1
    );

    restarted.pool().close().await;
    let cleanup = sqlx::PgPool::connect(&database_url).await.unwrap();
    cleanup
        .execute(AssertSqlSafe(format!(
            "DROP SCHEMA \"{schema_name}\" CASCADE"
        )))
        .await
        .unwrap();
    cleanup.close().await;
}
