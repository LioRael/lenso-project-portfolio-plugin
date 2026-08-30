use lenso_postgres_kit::{PostgresKitError, SchemaOperator, SetupOutcome, UpgradeOutcome};
use thiserror::Error;

use crate::schema::schema_plan;

/// Explicit schema administration for the Project Portfolio Plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProjectPortfolioOperator;

impl ProjectPortfolioOperator {
    pub async fn setup(
        database_url: &str,
        schema: &str,
    ) -> Result<SetupOutcome, ProjectPortfolioOperatorError> {
        Ok(SchemaOperator::connect(database_url, schema_plan(schema)?)
            .await?
            .setup()
            .await?)
    }

    pub async fn upgrade(
        database_url: &str,
        schema: &str,
    ) -> Result<UpgradeOutcome, ProjectPortfolioOperatorError> {
        Ok(SchemaOperator::connect(database_url, schema_plan(schema)?)
            .await?
            .upgrade()
            .await?)
    }
}

#[derive(Debug, Error)]
pub enum ProjectPortfolioOperatorError {
    #[error(transparent)]
    Plan(#[from] lenso_postgres_kit::PlanError),
    #[error(transparent)]
    Postgres(#[from] PostgresKitError),
}
