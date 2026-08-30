//! PostgreSQL-backed Project Portfolio Plugin with caller-supplied project snapshots.

mod operator;
#[cfg(all(test, feature = "postgres-acceptance"))]
mod postgres_tests;
mod schema;
mod storage;

use std::{cell::RefCell, collections::BTreeSet, fmt, rc::Rc, time::Duration};

use lenso::prelude::*;
use lenso_auth_sdk::{
    ActorAssertion, ActorAssertionVerifier, ActorProjectionError, AssertionClock, TypedActor,
};
use lenso_capability_access_control as access;
use lenso_capability_access_control::{
    AccessControlInvocationError, CheckPermissionRequest, CheckPermissionRequestScope,
};
use lenso_capability_organization_membership as membership;
use lenso_capability_organization_membership::{
    CheckMembershipRequest, OrganizationMembershipInvocationError,
};
use lenso_capability_project_portfolio as portfolio;
use lenso_capability_project_portfolio_admin as admin;
use lenso_capability_secrets as secrets;
use lenso_capability_secrets::{ResolveRequest, SecretsClient, SecretsInvocationError};
use lenso_kernel::{PluginDependencies, RuntimeFailure};
use lenso_postgres_kit::OwnedPostgres;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use time::{Date, OffsetDateTime, format_description};
use zeroize::Zeroizing;

use crate::storage::{DomainFailure, StorageError};

pub use operator::{ProjectPortfolioOperator, ProjectPortfolioOperatorError};

const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CALLERS: usize = 64;
const MAX_ID_BYTES: usize = 512;
const MAX_TEXT_BYTES: usize = 4_000;

const PORTFOLIO_READ: &str = "portfolio.initiatives.read";
const PORTFOLIO_WRITE: &str = "portfolio.initiatives.write";
const PORTFOLIO_ADMIN: &str = "portfolio.initiatives.admin";

/// Immutable configuration for one Project Portfolio Plugin Instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPortfolioConfig {
    schema: String,
    database_url_secret: String,
    auth_issuer: String,
    auth_assertion_public_key: String,
    portfolio_callers: Vec<String>,
    admin_callers: Vec<String>,
}

impl ProjectPortfolioConfig {
    pub fn new(
        schema: impl Into<String>,
        database_url_secret: impl Into<String>,
        auth_issuer: impl Into<String>,
        auth_assertion_public_key: impl Into<String>,
        portfolio_callers: Vec<String>,
        admin_callers: Vec<String>,
    ) -> Result<Self, ProjectPortfolioConfigError> {
        let value = Self {
            schema: schema.into(),
            database_url_secret: database_url_secret.into(),
            auth_issuer: auth_issuer.into(),
            auth_assertion_public_key: auth_assertion_public_key.into(),
            portfolio_callers,
            admin_callers,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), ProjectPortfolioConfigError> {
        schema::schema_plan(self.schema.clone())
            .map_err(|_| ProjectPortfolioConfigError::InvalidSchema)?;
        if !valid_secret_reference(&self.database_url_secret) {
            return Err(ProjectPortfolioConfigError::InvalidSecretReference);
        }
        if !valid_id(&self.auth_issuer) {
            return Err(ProjectPortfolioConfigError::InvalidAuthIssuer);
        }
        ActorAssertionVerifier::from_public_key_base64(
            self.auth_issuer.clone(),
            &self.auth_assertion_public_key,
        )
        .map_err(|_| ProjectPortfolioConfigError::InvalidAuthPublicKey)?;
        if !valid_callers(&self.portfolio_callers) {
            return Err(ProjectPortfolioConfigError::InvalidPortfolioCallers);
        }
        if !valid_callers(&self.admin_callers) {
            return Err(ProjectPortfolioConfigError::InvalidAdminCallers);
        }
        Ok(())
    }

    fn verifier(&self) -> Result<ActorAssertionVerifier, RuntimeFailure> {
        ActorAssertionVerifier::from_public_key_base64(
            self.auth_issuer.clone(),
            &self.auth_assertion_public_key,
        )
        .map_err(|_| RuntimeFailure::InvalidResolvedPlan {
            detail: "Project Portfolio Auth verification key is invalid".to_owned(),
        })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProjectPortfolioConfigError {
    #[error("invalid owned PostgreSQL schema")]
    InvalidSchema,
    #[error("invalid database URL secret reference")]
    InvalidSecretReference,
    #[error("invalid Auth issuer")]
    InvalidAuthIssuer,
    #[error("invalid Auth assertion public key")]
    InvalidAuthPublicKey,
    #[error("portfolio_callers must contain unique exact Instance keys")]
    InvalidPortfolioCallers,
    #[error("admin_callers must contain unique exact Instance keys")]
    InvalidAdminCallers,
}

fn validate_config(config: &ProjectPortfolioConfig) -> Result<(), RuntimeFailure> {
    config
        .validate()
        .map_err(|error| RuntimeFailure::InvalidResolvedPlan {
            detail: format!("Project Portfolio configuration is invalid: {error}"),
        })
}

#[derive(Clone, Debug)]
struct PreparedPortfolio {
    postgres: OwnedPostgres,
}

#[lenso::plugin(
    lifecycle,
    configuration_schema = "configuration.schema.json",
    validate = validate_config
)]
#[derive(Clone)]
struct ProjectPortfolioPlugin {
    #[config]
    config: ProjectPortfolioConfig,
    secrets: Port<secrets::SecretsClient>,
    membership: Port<membership::OrganizationMembershipClient>,
    access: Port<access::AccessControlClient>,
    prepared: Rc<RefCell<Option<PreparedPortfolio>>>,
}

impl fmt::Debug for ProjectPortfolioPlugin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectPortfolioPlugin")
            .field("schema", &self.config.schema)
            .field("prepared", &self.prepared.borrow().is_some())
            .field(
                "portfolio_caller_count",
                &self.config.portfolio_callers.len(),
            )
            .field("admin_caller_count", &self.config.admin_callers.len())
            .finish_non_exhaustive()
    }
}

#[lenso::provides(portfolio::ProjectPortfolio, admin::ProjectPortfolioAdmin)]
impl ProjectPortfolioPlugin {}

#[derive(Clone, Debug)]
struct Authorized {
    caller: String,
    actor: String,
}

#[derive(Debug)]
enum AuthorizationFailure {
    Unauthenticated,
    Forbidden,
    Runtime(RuntimeFailure),
}

macro_rules! auth_portfolio {
    ($result:expr,$kind:ident) => {
        match $result {
            Ok(value) => value,
            Err(AuthorizationFailure::Unauthenticated) => {
                return Err(PluginError::domain(portfolio::$kind::Unauthenticated))
            }
            Err(AuthorizationFailure::Forbidden) => {
                return Err(PluginError::domain(portfolio::$kind::Forbidden))
            }
            Err(AuthorizationFailure::Runtime(error)) => return Err(PluginError::runtime(error)),
        }
    };
}

macro_rules! auth_admin {
    ($result:expr,$kind:ident) => {
        match $result {
            Ok(value) => value,
            Err(AuthorizationFailure::Unauthenticated) => {
                return Err(PluginError::domain(admin::$kind::Unauthenticated))
            }
            Err(AuthorizationFailure::Forbidden) => {
                return Err(PluginError::domain(admin::$kind::Forbidden))
            }
            Err(AuthorizationFailure::Runtime(error)) => return Err(PluginError::runtime(error)),
        }
    };
}

macro_rules! portfolio_error {
    ($failure:expr,$kind:ident) => {
        match $failure {
            DomainFailure::NotFound => portfolio::$kind::NotFound,
            DomainFailure::Archived => portfolio::$kind::Archived,
            DomainFailure::RevisionConflict => portfolio::$kind::RevisionConflict,
            DomainFailure::IdempotencyConflict => portfolio::$kind::IdempotencyConflict,
            DomainFailure::OperationInProgress => portfolio::$kind::OperationInProgress,
            DomainFailure::AlreadyExists => portfolio::$kind::AlreadyExists,
            _ => portfolio::$kind::InvalidRequest,
        }
    };
}

macro_rules! read_error {
    ($failure:expr,$kind:ident) => {
        match $failure {
            DomainFailure::NotFound => portfolio::$kind::NotFound,
            _ => portfolio::$kind::InvalidRequest,
        }
    };
}

macro_rules! admin_error {
    ($failure:expr,$kind:ident) => {
        match $failure {
            DomainFailure::NotFound => admin::$kind::NotFound,
            DomainFailure::Archived => admin::$kind::Archived,
            DomainFailure::RevisionConflict => admin::$kind::RevisionConflict,
            DomainFailure::IdempotencyConflict => admin::$kind::IdempotencyConflict,
            DomainFailure::OperationInProgress => admin::$kind::OperationInProgress,
            DomainFailure::AlreadyAttached => admin::$kind::AlreadyAttached,
            DomainFailure::NotAttached => admin::$kind::NotAttached,
            DomainFailure::PositionConflict => admin::$kind::PositionConflict,
            _ => admin::$kind::InvalidRequest,
        }
    };
}

impl ProjectPortfolioPlugin {
    fn prepared(&self) -> Result<PreparedPortfolio, RuntimeFailure> {
        self.prepared
            .borrow()
            .clone()
            .ok_or_else(|| RuntimeFailure::PluginFailure {
                detail: "Project Portfolio Plugin is not prepared".to_owned(),
            })
    }

    async fn authorize(
        &self,
        context: &Ctx,
        callers: &[String],
        capability: &str,
        operation: &str,
        organization_id: &str,
        permission: &str,
    ) -> Result<Authorized, AuthorizationFailure> {
        let caller = context
            .caller_instance()
            .filter(|caller| callers.iter().any(|allowed| allowed == *caller))
            .map(ToOwned::to_owned)
            .ok_or(AuthorizationFailure::Forbidden)?;
        let actor = self
            .config
            .verifier()
            .map_err(AuthorizationFailure::Runtime)?
            .project_context::<PortfolioActor>(context, capability, operation, &UtcClock)
            .map_err(|_| AuthorizationFailure::Unauthenticated)?
            .subject;
        if !valid_opaque_id(organization_id) || !valid_opaque_id(&actor) {
            return Err(AuthorizationFailure::Forbidden);
        }
        let membership = self
            .membership
            .check_membership_with_context(
                context.clone(),
                CheckMembershipRequest {
                    organization_id: organization_id.to_owned(),
                    subject: actor.clone(),
                },
            )
            .await
            .map_err(|error| match error {
                OrganizationMembershipInvocationError::Runtime(error) => {
                    AuthorizationFailure::Runtime(error)
                }
                OrganizationMembershipInvocationError::Domain(_) => {
                    AuthorizationFailure::Runtime(RuntimeFailure::ProtocolViolation {
                        capability: membership::CAPABILITY_ID,
                    })
                }
            })?;
        if !membership.active {
            return Err(AuthorizationFailure::Forbidden);
        }
        let decision = self
            .access
            .check_permission_with_context(
                context.clone(),
                CheckPermissionRequest {
                    subject: actor.clone(),
                    scope: CheckPermissionRequestScope {
                        kind: "organization".to_owned(),
                        id: organization_id.to_owned(),
                    },
                    permission: permission.to_owned(),
                },
            )
            .await
            .map_err(|error| match error {
                AccessControlInvocationError::Runtime(error) => {
                    AuthorizationFailure::Runtime(error)
                }
                AccessControlInvocationError::Domain(_) => {
                    AuthorizationFailure::Runtime(RuntimeFailure::ProtocolViolation {
                        capability: access::CAPABILITY_ID,
                    })
                }
            })?;
        if !decision.allowed {
            return Err(AuthorizationFailure::Forbidden);
        }
        Ok(Authorized { caller, actor })
    }

    async fn create_initiative(
        &self,
        context: Ctx,
        request: portfolio::CreateInitiativeRequest,
    ) -> PluginResult<portfolio::CreateInitiativeResponse, portfolio::CreateInitiativeError> {
        let auth = auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::CREATE_INITIATIVE_OPERATION,
                &request.organization_id,
                PORTFOLIO_WRITE,
            )
            .await,
            CreateInitiativeError
        );
        let dates = parse_date_window(
            request.target_start.as_deref(),
            request.target_date.as_deref(),
        )
        .ok_or_else(|| PluginError::domain(portfolio::CreateInitiativeError::InvalidRequest))?;
        if !valid_idempotency_key(&request.idempotency_key)
            || !valid_opaque_id(&request.initiative_id)
            || !valid_text(&request.name, 240)
            || !valid_optional_text(request.summary.as_deref(), MAX_TEXT_BYTES)
            || request
                .owner_subject
                .as_ref()
                .is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(PluginError::domain(
                portfolio::CreateInitiativeError::InvalidRequest,
            ));
        }
        let hash = request_hash(&request)?;
        let value = storage::InitiativeCreate {
            organization_id: &request.organization_id,
            initiative_id: &request.initiative_id,
            name: &request.name,
            summary: request.summary.as_deref(),
            owner_subject: request.owner_subject.as_deref(),
            target_start: dates.0,
            target_date: dates.1,
        };
        let record = map_portfolio_storage(
            storage::create_initiative(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &auth.caller,
                &auth.actor,
                &request.idempotency_key,
                &hash,
                &value,
            )
            .await,
            |failure| portfolio_error!(failure, CreateInitiativeError),
        )?;
        wire_cast(&record)
    }

    async fn get_initiative(
        &self,
        context: Ctx,
        request: portfolio::GetInitiativeRequest,
    ) -> PluginResult<portfolio::GetInitiativeResponse, portfolio::GetInitiativeError> {
        auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::GET_INITIATIVE_OPERATION,
                &request.organization_id,
                PORTFOLIO_READ,
            )
            .await,
            GetInitiativeError
        );
        if !valid_opaque_id(&request.initiative_id) {
            return Err(PluginError::domain(
                portfolio::GetInitiativeError::InvalidRequest,
            ));
        }
        let record = map_portfolio_storage(
            storage::get_initiative(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &request.organization_id,
                &request.initiative_id,
            )
            .await,
            |failure| read_error!(failure, GetInitiativeError),
        )?;
        wire_cast(&record)
    }

    async fn list_initiatives(
        &self,
        context: Ctx,
        request: portfolio::ListInitiativesRequest,
    ) -> PluginResult<portfolio::ListInitiativesResponse, portfolio::ListInitiativesError> {
        auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::LIST_INITIATIVES_OPERATION,
                &request.organization_id,
                PORTFOLIO_READ,
            )
            .await,
            ListInitiativesError
        );
        let after = parse_cursor(request.after.as_deref())
            .map_err(|()| PluginError::domain(portfolio::ListInitiativesError::InvalidRequest))?;
        if !(1..=200).contains(&request.limit) {
            return Err(PluginError::domain(
                portfolio::ListInitiativesError::InvalidRequest,
            ));
        }
        let mut records = map_portfolio_storage(
            storage::list_initiatives(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &request.organization_id,
                request.include_archived,
                after,
                request.limit + 1,
            )
            .await,
            |failure| read_error!(failure, ListInitiativesError),
        )?;
        let has_more = records.len() > usize::try_from(request.limit).unwrap_or(0);
        if has_more {
            records.pop();
        }
        let next_cursor = has_more
            .then(|| records.last().map(|record| record.row_seq.to_string()))
            .flatten();
        let initiatives = records
            .iter()
            .map(wire_cast)
            .collect::<PluginResult<Vec<portfolio::ListInitiativesResponseInitiativesItem>, _>>()?;
        Ok(portfolio::ListInitiativesResponse {
            initiatives,
            next_cursor,
        })
    }

    async fn update_initiative(
        &self,
        context: Ctx,
        request: portfolio::UpdateInitiativeRequest,
    ) -> PluginResult<portfolio::UpdateInitiativeResponse, portfolio::UpdateInitiativeError> {
        let auth = auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::UPDATE_INITIATIVE_OPERATION,
                &request.organization_id,
                PORTFOLIO_WRITE,
            )
            .await,
            UpdateInitiativeError
        );
        let expected_revision = parse_revision(&request.expected_revision)
            .ok_or_else(|| PluginError::domain(portfolio::UpdateInitiativeError::InvalidRequest))?;
        let dates = parse_date_window(
            request.target_start.as_deref(),
            request.target_date.as_deref(),
        )
        .ok_or_else(|| PluginError::domain(portfolio::UpdateInitiativeError::InvalidRequest))?;
        if !valid_idempotency_key(&request.idempotency_key)
            || !valid_opaque_id(&request.initiative_id)
            || request
                .name
                .as_ref()
                .is_some_and(|value| !valid_text(value, 240))
            || !valid_optional_text(request.summary.as_deref(), MAX_TEXT_BYTES)
            || request
                .owner_subject
                .as_ref()
                .is_some_and(|value| !valid_opaque_id(value))
        {
            return Err(PluginError::domain(
                portfolio::UpdateInitiativeError::InvalidRequest,
            ));
        }
        let hash = request_hash(&request)?;
        let patch = storage::InitiativePatch {
            name: request.name.as_deref(),
            summary: request.summary.as_deref(),
            owner_subject: request.owner_subject.as_deref(),
            target_start: dates.0,
            target_date: dates.1,
        };
        let record = map_portfolio_storage(
            storage::update_initiative(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &auth.caller,
                &auth.actor,
                &request.idempotency_key,
                &hash,
                &request.organization_id,
                &request.initiative_id,
                expected_revision,
                &patch,
            )
            .await,
            |failure| portfolio_error!(failure, UpdateInitiativeError),
        )?;
        wire_cast(&record)
    }

    async fn list_initiative_projects(
        &self,
        context: Ctx,
        request: portfolio::ListInitiativeProjectsRequest,
    ) -> PluginResult<
        portfolio::ListInitiativeProjectsResponse,
        portfolio::ListInitiativeProjectsError,
    > {
        auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::LIST_INITIATIVE_PROJECTS_OPERATION,
                &request.organization_id,
                PORTFOLIO_READ,
            )
            .await,
            ListInitiativeProjectsError
        );
        if !valid_opaque_id(&request.initiative_id) || !(1..=200).contains(&request.limit) {
            return Err(PluginError::domain(
                portfolio::ListInitiativeProjectsError::InvalidRequest,
            ));
        }
        let mut records = map_portfolio_storage(
            storage::list_projects(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &request.organization_id,
                &request.initiative_id,
                request.after_position,
                request.limit + 1,
            )
            .await,
            |failure| read_error!(failure, ListInitiativeProjectsError),
        )?;
        let has_more = records.len() > usize::try_from(request.limit).unwrap_or(0);
        if has_more {
            records.pop();
        }
        let next_position = has_more
            .then(|| records.last().map(|record| record.position))
            .flatten();
        let projects = records
            .iter()
            .map(wire_cast)
            .collect::<PluginResult<Vec<portfolio::ListInitiativeProjectsResponseProjectsItem>, _>>(
            )?;
        Ok(portfolio::ListInitiativeProjectsResponse {
            projects,
            next_position,
        })
    }

    async fn add_initiative_update(
        &self,
        context: Ctx,
        request: portfolio::AddInitiativeUpdateRequest,
    ) -> PluginResult<portfolio::AddInitiativeUpdateResponse, portfolio::AddInitiativeUpdateError>
    {
        let auth = auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::ADD_INITIATIVE_UPDATE_OPERATION,
                &request.organization_id,
                PORTFOLIO_WRITE,
            )
            .await,
            AddInitiativeUpdateError
        );
        let expected_revision = parse_revision(&request.expected_revision).ok_or_else(|| {
            PluginError::domain(portfolio::AddInitiativeUpdateError::InvalidRequest)
        })?;
        if !valid_idempotency_key(&request.idempotency_key)
            || !valid_opaque_id(&request.initiative_id)
            || !valid_opaque_id(&request.update_id)
            || !valid_text(&request.summary, MAX_TEXT_BYTES)
            || !(0..=100).contains(&request.progress)
        {
            return Err(PluginError::domain(
                portfolio::AddInitiativeUpdateError::InvalidRequest,
            ));
        }
        let hash = request_hash(&request)?;
        let health = enum_string(&request.health)?;
        let record = map_portfolio_storage(
            storage::add_update(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &auth.caller,
                &auth.actor,
                &request.idempotency_key,
                &hash,
                &request.organization_id,
                &request.initiative_id,
                &request.update_id,
                expected_revision,
                &health,
                &request.summary,
                request.progress,
            )
            .await,
            |failure| portfolio_error!(failure, AddInitiativeUpdateError),
        )?;
        wire_cast(&record)
    }

    async fn list_initiative_updates(
        &self,
        context: Ctx,
        request: portfolio::ListInitiativeUpdatesRequest,
    ) -> PluginResult<portfolio::ListInitiativeUpdatesResponse, portfolio::ListInitiativeUpdatesError>
    {
        auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::LIST_INITIATIVE_UPDATES_OPERATION,
                &request.organization_id,
                PORTFOLIO_READ,
            )
            .await,
            ListInitiativeUpdatesError
        );
        let after = parse_cursor(request.after.as_deref()).map_err(|()| {
            PluginError::domain(portfolio::ListInitiativeUpdatesError::InvalidRequest)
        })?;
        if !valid_opaque_id(&request.initiative_id) || !(1..=200).contains(&request.limit) {
            return Err(PluginError::domain(
                portfolio::ListInitiativeUpdatesError::InvalidRequest,
            ));
        }
        let mut records = map_portfolio_storage(
            storage::list_updates(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &request.organization_id,
                &request.initiative_id,
                after,
                request.limit + 1,
            )
            .await,
            |failure| read_error!(failure, ListInitiativeUpdatesError),
        )?;
        let has_more = records.len() > usize::try_from(request.limit).unwrap_or(0);
        if has_more {
            records.pop();
        }
        let next_cursor = has_more
            .then(|| records.last().map(|record| record.row_seq.to_string()))
            .flatten();
        let updates = records
            .iter()
            .map(wire_cast)
            .collect::<PluginResult<Vec<portfolio::ListInitiativeUpdatesResponseUpdatesItem>, _>>(
            )?;
        Ok(portfolio::ListInitiativeUpdatesResponse {
            updates,
            next_cursor,
        })
    }

    async fn read_rollup(
        &self,
        context: Ctx,
        request: portfolio::ReadRollupRequest,
    ) -> PluginResult<portfolio::ReadRollupResponse, portfolio::ReadRollupError> {
        auth_portfolio!(
            self.authorize(
                &context,
                &self.config.portfolio_callers,
                portfolio::CAPABILITY_ID,
                portfolio::READ_ROLLUP_OPERATION,
                &request.organization_id,
                PORTFOLIO_READ,
            )
            .await,
            ReadRollupError
        );
        if !valid_opaque_id(&request.initiative_id) {
            return Err(PluginError::domain(
                portfolio::ReadRollupError::InvalidRequest,
            ));
        }
        let record = map_portfolio_storage(
            storage::read_rollup(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                &request.organization_id,
                &request.initiative_id,
            )
            .await,
            |failure| read_error!(failure, ReadRollupError),
        )?;
        wire_cast(&record)
    }

    async fn archive_initiative(
        &self,
        context: Ctx,
        request: admin::ArchiveInitiativeRequest,
    ) -> PluginResult<admin::ArchiveInitiativeResponse, admin::ArchiveInitiativeError> {
        let auth = auth_admin!(
            self.authorize(
                &context,
                &self.config.admin_callers,
                admin::CAPABILITY_ID,
                admin::ARCHIVE_INITIATIVE_OPERATION,
                &request.organization_id,
                PORTFOLIO_ADMIN,
            )
            .await,
            ArchiveInitiativeError
        );
        let expected_revision = valid_admin_mutation(
            &request.idempotency_key,
            &request.initiative_id,
            &request.expected_revision,
        )
        .ok_or_else(|| PluginError::domain(admin::ArchiveInitiativeError::InvalidRequest))?;
        let hash = request_hash(&request)?;
        let record = map_admin_storage(
            storage::archive_initiative(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                command(&auth, &request.idempotency_key, &hash),
                &request.organization_id,
                &request.initiative_id,
                expected_revision,
            )
            .await,
            |failure| admin_error!(failure, ArchiveInitiativeError),
        )?;
        wire_cast(&record)
    }

    async fn attach_project(
        &self,
        context: Ctx,
        request: admin::AttachProjectRequest,
    ) -> PluginResult<admin::AttachProjectResponse, admin::AttachProjectError> {
        let auth = auth_admin!(
            self.authorize(
                &context,
                &self.config.admin_callers,
                admin::CAPABILITY_ID,
                admin::ATTACH_PROJECT_OPERATION,
                &request.organization_id,
                PORTFOLIO_ADMIN,
            )
            .await,
            AttachProjectError
        );
        let expected_revision = valid_admin_mutation(
            &request.idempotency_key,
            &request.initiative_id,
            &request.expected_initiative_revision,
        )
        .ok_or_else(|| PluginError::domain(admin::AttachProjectError::InvalidRequest))?;
        let snapshot = parse_snapshot(
            &request.project_id,
            &request.name_snapshot,
            &request.status_category,
            &request.health,
            request.progress,
            request.target_start.as_deref(),
            request.target_date.as_deref(),
            &request.snapshot_revision,
            &request.observed_at,
        )
        .map_err(|()| PluginError::domain(admin::AttachProjectError::InvalidRequest))?;
        if !(0..=10_000).contains(&request.position) {
            return Err(PluginError::domain(
                admin::AttachProjectError::InvalidRequest,
            ));
        }
        let hash = request_hash(&request)?;
        let record = map_admin_storage(
            storage::attach_project(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                command(&auth, &request.idempotency_key, &hash),
                &request.organization_id,
                &request.initiative_id,
                expected_revision,
                &snapshot,
                request.position,
            )
            .await,
            |failure| admin_error!(failure, AttachProjectError),
        )?;
        wire_cast(&record)
    }

    async fn update_project_snapshot(
        &self,
        context: Ctx,
        request: admin::UpdateProjectSnapshotRequest,
    ) -> PluginResult<admin::UpdateProjectSnapshotResponse, admin::UpdateProjectSnapshotError> {
        let auth = auth_admin!(
            self.authorize(
                &context,
                &self.config.admin_callers,
                admin::CAPABILITY_ID,
                admin::UPDATE_PROJECT_SNAPSHOT_OPERATION,
                &request.organization_id,
                PORTFOLIO_ADMIN,
            )
            .await,
            UpdateProjectSnapshotError
        );
        let expected_revision = valid_admin_mutation(
            &request.idempotency_key,
            &request.initiative_id,
            &request.expected_membership_revision,
        )
        .ok_or_else(|| PluginError::domain(admin::UpdateProjectSnapshotError::InvalidRequest))?;
        let snapshot = parse_snapshot(
            &request.project_id,
            &request.name_snapshot,
            &request.status_category,
            &request.health,
            request.progress,
            request.target_start.as_deref(),
            request.target_date.as_deref(),
            &request.snapshot_revision,
            &request.observed_at,
        )
        .map_err(|()| PluginError::domain(admin::UpdateProjectSnapshotError::InvalidRequest))?;
        let hash = request_hash(&request)?;
        let record = map_admin_storage(
            storage::update_project_snapshot(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                command(&auth, &request.idempotency_key, &hash),
                &request.organization_id,
                &request.initiative_id,
                expected_revision,
                &snapshot,
            )
            .await,
            |failure| admin_error!(failure, UpdateProjectSnapshotError),
        )?;
        wire_cast(&record)
    }

    async fn detach_project(
        &self,
        context: Ctx,
        request: admin::DetachProjectRequest,
    ) -> PluginResult<admin::DetachProjectResponse, admin::DetachProjectError> {
        let auth = auth_admin!(
            self.authorize(
                &context,
                &self.config.admin_callers,
                admin::CAPABILITY_ID,
                admin::DETACH_PROJECT_OPERATION,
                &request.organization_id,
                PORTFOLIO_ADMIN,
            )
            .await,
            DetachProjectError
        );
        let initiative_revision = valid_admin_mutation(
            &request.idempotency_key,
            &request.initiative_id,
            &request.expected_initiative_revision,
        )
        .ok_or_else(|| PluginError::domain(admin::DetachProjectError::InvalidRequest))?;
        let membership_revision = parse_revision(&request.expected_membership_revision)
            .filter(|_| valid_opaque_id(&request.project_id))
            .ok_or_else(|| PluginError::domain(admin::DetachProjectError::InvalidRequest))?;
        let hash = request_hash(&request)?;
        let record = map_admin_storage(
            storage::detach_project(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                command(&auth, &request.idempotency_key, &hash),
                &request.organization_id,
                &request.initiative_id,
                &request.project_id,
                initiative_revision,
                membership_revision,
            )
            .await,
            |failure| admin_error!(failure, DetachProjectError),
        )?;
        wire_cast(&record)
    }

    async fn reorder_projects(
        &self,
        context: Ctx,
        request: admin::ReorderProjectsRequest,
    ) -> PluginResult<admin::ReorderProjectsResponse, admin::ReorderProjectsError> {
        let auth = auth_admin!(
            self.authorize(
                &context,
                &self.config.admin_callers,
                admin::CAPABILITY_ID,
                admin::REORDER_PROJECTS_OPERATION,
                &request.organization_id,
                PORTFOLIO_ADMIN,
            )
            .await,
            ReorderProjectsError
        );
        let initiative_revision = valid_admin_mutation(
            &request.idempotency_key,
            &request.initiative_id,
            &request.expected_initiative_revision,
        )
        .ok_or_else(|| PluginError::domain(admin::ReorderProjectsError::InvalidRequest))?;
        let items = request
            .items
            .iter()
            .map(|item| {
                Some(storage::ReorderInput {
                    project_id: valid_opaque_id(&item.project_id)
                        .then_some(item.project_id.as_str())?,
                    expected_membership_revision: parse_revision(
                        &item.expected_membership_revision,
                    )?,
                    position: (0..=10_000)
                        .contains(&item.position)
                        .then_some(item.position)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| PluginError::domain(admin::ReorderProjectsError::InvalidRequest))?;
        let hash = request_hash(&request)?;
        let record = map_admin_storage(
            storage::reorder_projects(
                &self.prepared().map_err(PluginError::runtime)?.postgres,
                command(&auth, &request.idempotency_key, &hash),
                &request.organization_id,
                &request.initiative_id,
                initiative_revision,
                &items,
            )
            .await,
            |failure| admin_error!(failure, ReorderProjectsError),
        )?;
        wire_cast(&record)
    }
}

impl Lifecycle for ProjectPortfolioPlugin {
    async fn activate(&self, context: ActivateContext) -> Result<(), RuntimeFailure> {
        let database_url = resolve_secret(
            &self.secrets,
            context.dependencies(),
            context.cancellation(),
            &self.config.database_url_secret,
        )
        .await?;
        let postgres = OwnedPostgres::prepare(
            &database_url,
            schema::schema_plan(self.config.schema.clone()).map_err(|error| {
                RuntimeFailure::InvalidResolvedPlan {
                    detail: error.to_string(),
                }
            })?,
        )
        .await
        .map_err(|error| RuntimeFailure::PluginFailure {
            detail: error.to_string(),
        })?;
        self.prepared
            .borrow_mut()
            .replace(PreparedPortfolio { postgres });
        Ok(())
    }

    async fn deactivate(&self, _context: DeactivateContext) -> Result<(), RuntimeFailure> {
        let prepared = self.prepared.borrow_mut().take();
        if let Some(prepared) = prepared {
            prepared.postgres.pool().close().await;
        }
        Ok(())
    }
}

fn map_portfolio_storage<T, E>(
    result: Result<T, StorageError>,
    map: impl FnOnce(DomainFailure) -> E,
) -> PluginResult<T, E> {
    map_storage(result, map)
}

fn map_admin_storage<T, E>(
    result: Result<T, StorageError>,
    map: impl FnOnce(DomainFailure) -> E,
) -> PluginResult<T, E> {
    map_storage(result, map)
}

fn map_storage<T, E>(
    result: Result<T, StorageError>,
    map: impl FnOnce(DomainFailure) -> E,
) -> PluginResult<T, E> {
    match result {
        Ok(value) => Ok(value),
        Err(StorageError::Domain(error)) => Err(PluginError::domain(map(error))),
        Err(error) => Err(PluginError::runtime(RuntimeFailure::PluginFailure {
            detail: error.to_string(),
        })),
    }
}

fn command<'a>(auth: &'a Authorized, key: &'a str, hash: &'a [u8]) -> storage::Command<'a> {
    storage::Command {
        caller: &auth.caller,
        actor: &auth.actor,
        key,
        hash,
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_snapshot<'a>(
    project_id: &'a str,
    name_snapshot: &'a str,
    status: &'a admin::Status,
    health: &'a admin::Health,
    progress: i64,
    target_start: Option<&str>,
    target_date: Option<&str>,
    snapshot_revision: &'a str,
    observed_at: &admin::Timestamp,
) -> Result<storage::ProjectSnapshot<'a>, ()> {
    let dates = parse_date_window(target_start, target_date).ok_or(())?;
    if !valid_opaque_id(project_id)
        || !valid_text(name_snapshot, 240)
        || !valid_bounded_opaque(snapshot_revision, 128)
        || !(0..=100).contains(&progress)
    {
        return Err(());
    }
    Ok(storage::ProjectSnapshot {
        project_id,
        name_snapshot,
        status_category: enum_string_plain(status)?,
        health: enum_string_plain(health)?,
        progress,
        target_start: dates.0,
        target_date: dates.1,
        snapshot_revision,
        observed_at: parse_timestamp(observed_at).ok_or(())?,
    })
}

async fn resolve_secret(
    secrets: &SecretsClient,
    dependencies: &PluginDependencies,
    cancellation: lenso_kernel::CancellationToken,
    reference: &str,
) -> Result<Zeroizing<String>, RuntimeFailure> {
    let context = dependencies.invocation_context_after(DEPENDENCY_TIMEOUT, cancellation)?;
    secrets
        .resolve_with_context(
            context,
            ResolveRequest {
                reference: reference.to_owned(),
            },
        )
        .await
        .map(|value| Zeroizing::new(value.value))
        .map_err(|error| match error {
            SecretsInvocationError::Domain(_) => RuntimeFailure::PluginFailure {
                detail: "Project Portfolio database secret was rejected".to_owned(),
            },
            SecretsInvocationError::Runtime(error) => error,
        })
}

fn request_hash<T: Serialize, E>(request: &T) -> Result<Vec<u8>, PluginError<E>> {
    serde_json::to_vec(request)
        .map(|wire| Sha256::digest(wire).to_vec())
        .map_err(serialization_runtime)
}

fn wire_cast<T: DeserializeOwned, E>(value: &impl Serialize) -> Result<T, PluginError<E>> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(serialization_runtime)
}

fn enum_string<T: Serialize, E>(value: &T) -> Result<String, PluginError<E>> {
    serde_json::to_value(value)
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| serde_json::Error::io(std::io::Error::other("enum is not a string")))
        })
        .map_err(serialization_runtime)
}

fn enum_string_plain(value: &impl Serialize) -> Result<String, ()> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or(())
}

#[allow(clippy::needless_pass_by_value)]
fn serialization_runtime<E>(error: serde_json::Error) -> PluginError<E> {
    PluginError::runtime(RuntimeFailure::Internal {
        detail: format!("Project Portfolio wire serialization failed: {error}"),
    })
}

fn parse_date_window(
    start: Option<&str>,
    end: Option<&str>,
) -> Option<(Option<Date>, Option<Date>)> {
    let description = format_description::parse_borrowed::<2>("[year]-[month]-[day]").ok()?;
    let start = start
        .map(|value| Date::parse(value, &description))
        .transpose()
        .ok()?;
    let end = end
        .map(|value| Date::parse(value, &description))
        .transpose()
        .ok()?;
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return None;
    }
    Some((start, end))
}

fn parse_timestamp(value: &impl Serialize) -> Option<OffsetDateTime> {
    serde_json::to_value(value)
        .ok()?
        .as_str()
        .and_then(|value| {
            OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        })
}

fn parse_revision(value: &str) -> Option<i64> {
    value.parse().ok().filter(|revision| *revision > 0)
}

fn parse_cursor(value: Option<&str>) -> Result<Option<i64>, ()> {
    match value {
        None => Ok(None),
        Some(value) => parse_revision(value).map(Some).ok_or(()),
    }
}

fn valid_admin_mutation(key: &str, initiative_id: &str, revision: &str) -> Option<i64> {
    (valid_idempotency_key(key) && valid_opaque_id(initiative_id))
        .then(|| parse_revision(revision))
        .flatten()
}

fn valid_callers(values: &[String]) -> bool {
    !values.is_empty()
        && values.len() <= MAX_CALLERS
        && values.iter().all(|value| valid_id(value))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn valid_opaque_id(value: &str) -> bool {
    valid_bounded_opaque(value, MAX_ID_BYTES)
}

fn valid_bounded_opaque(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.contains('\0')
        && !value.chars().any(char::is_control)
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.contains('\0')
}

fn valid_optional_text(value: Option<&str>, max: usize) -> bool {
    value.is_none_or(|value| valid_text(value, max))
}

fn valid_idempotency_key(value: &str) -> bool {
    valid_id(value) && value.len() <= 200
}

fn valid_secret_reference(value: &str) -> bool {
    valid_id(value)
        || (!value.is_empty()
            && value.len() <= 256
            && !value.starts_with('/')
            && !value.ends_with('/')
            && !value.contains("//")
            && value.split('/').all(|part| part != "." && part != "..")
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/')
            }))
}

#[derive(Clone, Debug)]
struct PortfolioActor {
    subject: String,
}

impl TypedActor for PortfolioActor {
    fn from_assertion(assertion: &ActorAssertion) -> Result<Self, ActorProjectionError> {
        Ok(Self {
            subject: assertion.subject().to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct UtcClock;

impl AssertionClock for UtcClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lenso_auth_sdk::{ActorAssertionIssuer, Validity, audience};
    use lenso_kernel::{CancellationToken, InvocationContext};
    use lenso_native_adapter::NativePluginRegistry;
    use time::Duration as TimeDuration;

    fn config() -> ProjectPortfolioConfig {
        let issuer = ActorAssertionIssuer::new("auth.users", b"portfolio-test-key");
        ProjectPortfolioConfig::new(
            "project_portfolio",
            "portfolio/database-url",
            "auth.users",
            issuer.public_key_base64(),
            vec!["portfolio-api".to_owned()],
            vec!["portfolio-admin".to_owned()],
        )
        .unwrap()
    }

    fn plugin() -> ProjectPortfolioPlugin {
        ProjectPortfolioPlugin {
            config: config(),
            secrets: Port::default(),
            membership: Port::default(),
            access: Port::default(),
            prepared: Rc::new(RefCell::new(None)),
        }
    }

    fn context(caller: &str) -> InvocationContext {
        InvocationContext::new(1, None, CancellationToken::new()).with_caller_instance(caller)
    }

    #[test]
    fn descriptor_declares_two_roles_and_three_dependencies() {
        let descriptor: serde_json::Value = serde_json::from_str(PLUGIN_DESCRIPTOR_JSON).unwrap();
        let provided = descriptor["provided_capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value["capability_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            provided,
            BTreeSet::from([portfolio::CAPABILITY_ID, admin::CAPABILITY_ID])
        );
        let required = descriptor["required_capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value["capability_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            required,
            BTreeSet::from([
                secrets::CAPABILITY_ID,
                membership::CAPABILITY_ID,
                access::CAPABILITY_ID
            ])
        );
        assert_eq!(
            NativePluginRegistry::new()
                .with_linked_factories()
                .factories()
                .filter(|factory| factory.package_id() == PACKAGE_ID)
                .count(),
            1
        );
    }

    #[test]
    fn configuration_rejects_duplicate_callers() {
        let mut invalid = config();
        invalid.portfolio_callers.push("portfolio-api".to_owned());
        assert_eq!(
            invalid.validate(),
            Err(ProjectPortfolioConfigError::InvalidPortfolioCallers)
        );
    }

    #[test]
    fn caller_is_rejected_before_assertion_or_storage() {
        let result = futures::executor::block_on(plugin().get_initiative(
            context("unknown"),
            portfolio::GetInitiativeRequest {
                organization_id: "org".to_owned(),
                initiative_id: "initiative".to_owned(),
            },
        ));
        assert_eq!(
            result,
            Err(PluginError::Domain(
                portfolio::GetInitiativeError::Forbidden
            ))
        );
    }

    #[test]
    fn actor_assertion_is_bound_to_exact_operation() {
        let issuer = ActorAssertionIssuer::new("auth.users", b"portfolio-test-key");
        let now = OffsetDateTime::now_utc();
        let assertion = issuer.issue(
            "usr_1",
            "user",
            "strong",
            [audience(
                portfolio::CAPABILITY_ID,
                portfolio::GET_INITIATIVE_OPERATION,
            )],
            Validity::new(
                now - TimeDuration::seconds(1),
                now + TimeDuration::minutes(1),
            )
            .unwrap(),
            std::collections::BTreeMap::default(),
        );
        let context = assertion.attach(context("portfolio-api")).unwrap();
        let actor = config()
            .verifier()
            .unwrap()
            .project_context::<PortfolioActor>(
                &context,
                portfolio::CAPABILITY_ID,
                portfolio::GET_INITIATIVE_OPERATION,
                &UtcClock,
            )
            .unwrap();
        assert_eq!(actor.subject, "usr_1");
        assert!(
            config()
                .verifier()
                .unwrap()
                .project_context::<PortfolioActor>(
                    &context,
                    portfolio::CAPABILITY_ID,
                    portfolio::UPDATE_INITIATIVE_OPERATION,
                    &UtcClock,
                )
                .is_err()
        );
    }

    #[test]
    fn rollup_uses_only_owned_project_snapshots() {
        let initiative = storage::InitiativeRecord {
            initiative_id: "ini".to_owned(),
            organization_id: "org".to_owned(),
            name: "Launch".to_owned(),
            summary: None,
            owner_subject: None,
            target_start: None,
            target_date: None,
            health: "at_risk".to_owned(),
            progress: 55,
            project_count: 2,
            archived: false,
            revision: "3".to_owned(),
            created_at: "2026-08-30T00:00:00Z".to_owned(),
            updated_at: "2026-08-30T00:00:00Z".to_owned(),
            archived_at: None,
            row_seq: 1,
        };
        let project = |id: &str, status: &str, health: &str, progress| storage::ProjectRecord {
            project_id: id.to_owned(),
            name_snapshot: id.to_owned(),
            status_category: status.to_owned(),
            health: health.to_owned(),
            progress,
            target_start: Some("2026-08-01".to_owned()),
            target_date: Some("2026-09-01".to_owned()),
            snapshot_revision: "p1".to_owned(),
            observed_at: "2026-08-30T00:00:00Z".to_owned(),
            position: 0,
            membership_revision: "1".to_owned(),
            initiative_revision: None,
        };
        let rollup = storage::compute_rollup(
            &initiative,
            &[
                project("a", "completed", "on_track", 100),
                project("b", "started", "off_track", 20),
            ],
            OffsetDateTime::UNIX_EPOCH,
        )
        .unwrap();
        assert_eq!(rollup.project_count, 2);
        assert_eq!(rollup.completed_count, 1);
        assert_eq!(rollup.off_track_count, 1);
        assert_eq!(rollup.average_project_progress, Some(60.0));
        assert_eq!(rollup.source, "owned_project_snapshots");
    }
}
