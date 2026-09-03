//! Administrative Agent Tools over an explicitly bound Project Portfolio Admin capability.

use lenso::prelude::*;
use lenso_capability_agent_tool_provider::{
    self as tool_contract, CatalogRequest, CatalogResponse, ContentType, ExecuteError,
    ExecuteRequest, ExecuteResponse, ExecutionFailedPayload, ToolDefinition, ToolExecutionClass,
};
use lenso_capability_project_portfolio_admin::{
    self as admin, ArchiveInitiativeRequest, AttachProjectRequest, DetachProjectRequest,
    ReorderProjectsRequest, UpdateProjectSnapshotRequest,
};
use lenso_kernel::RuntimeFailure;
use serde::{Serialize, de::DeserializeOwned};

pub const ARCHIVE_INITIATIVE_TOOL: &str = "project_portfolio_admin_archive_initiative";
pub const ATTACH_PROJECT_TOOL: &str = "project_portfolio_admin_attach_project";
pub const UPDATE_PROJECT_SNAPSHOT_TOOL: &str = "project_portfolio_admin_update_project_snapshot";
pub const DETACH_PROJECT_TOOL: &str = "project_portfolio_admin_detach_project";
pub const REORDER_PROJECTS_TOOL: &str = "project_portfolio_admin_reorder_projects";

#[lenso::plugin]
#[derive(Clone, Debug)]
struct ProjectPortfolioAdminAgentToolsPlugin {
    admin: Port<admin::ProjectPortfolioAdminClient>,
}

#[lenso::provides(tool_contract::ToolProvider)]
impl ProjectPortfolioAdminAgentToolsPlugin {
    fn catalog(
        &self,
        _context: Ctx,
        _request: CatalogRequest,
    ) -> impl std::future::Future<Output = PluginResult<CatalogResponse, tool_contract::CatalogError>>
    {
        let _ = self;
        futures::future::ready(Ok(CatalogResponse {
            tools: tool_definitions(),
        }))
    }

    async fn execute(
        &self,
        context: Ctx,
        request: ExecuteRequest,
    ) -> PluginResult<ExecuteResponse, ExecuteError> {
        macro_rules! invoke {
            ($future:expr, $tool:expr, $domain:path, $runtime:path) => {
                match $future.await {
                    Ok(response) => success($tool, &response),
                    Err($domain(error)) => Err(PluginError::domain(map_domain_error(&error))),
                    Err($runtime(error)) => Err(PluginError::runtime(error)),
                }
            };
        }

        match request.name.as_str() {
            ARCHIVE_INITIATIVE_TOOL => {
                let arguments = decode::<ArchiveInitiativeRequest>(&request)?;
                invoke!(
                    self.admin
                        .archive_initiative_with_context(context, arguments),
                    ARCHIVE_INITIATIVE_TOOL,
                    admin::ProjectPortfolioAdminArchiveInitiativeInvocationError::Domain,
                    admin::ProjectPortfolioAdminArchiveInitiativeInvocationError::Runtime
                )
            }
            ATTACH_PROJECT_TOOL => {
                let arguments = decode::<AttachProjectRequest>(&request)?;
                invoke!(
                    self.admin.attach_project_with_context(context, arguments),
                    ATTACH_PROJECT_TOOL,
                    admin::ProjectPortfolioAdminAttachProjectInvocationError::Domain,
                    admin::ProjectPortfolioAdminAttachProjectInvocationError::Runtime
                )
            }
            UPDATE_PROJECT_SNAPSHOT_TOOL => {
                let arguments = decode::<UpdateProjectSnapshotRequest>(&request)?;
                invoke!(
                    self.admin
                        .update_project_snapshot_with_context(context, arguments),
                    UPDATE_PROJECT_SNAPSHOT_TOOL,
                    admin::ProjectPortfolioAdminUpdateProjectSnapshotInvocationError::Domain,
                    admin::ProjectPortfolioAdminUpdateProjectSnapshotInvocationError::Runtime
                )
            }
            DETACH_PROJECT_TOOL => {
                let arguments = decode::<DetachProjectRequest>(&request)?;
                invoke!(
                    self.admin.detach_project_with_context(context, arguments),
                    DETACH_PROJECT_TOOL,
                    admin::ProjectPortfolioAdminDetachProjectInvocationError::Domain,
                    admin::ProjectPortfolioAdminDetachProjectInvocationError::Runtime
                )
            }
            REORDER_PROJECTS_TOOL => {
                let arguments = decode::<ReorderProjectsRequest>(&request)?;
                invoke!(
                    self.admin.reorder_projects_with_context(context, arguments),
                    REORDER_PROJECTS_TOOL,
                    admin::ProjectPortfolioAdminReorderProjectsInvocationError::Domain,
                    admin::ProjectPortfolioAdminReorderProjectsInvocationError::Runtime
                )
            }
            _ => Err(PluginError::domain(ExecuteError::NotFound)),
        }
    }
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        tool(
            ARCHIVE_INITIATIVE_TOOL,
            "Archive one initiative using its current revision. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio-admin/schemas/archive-initiative-request.schema.json"
            ),
        ),
        tool(
            ATTACH_PROJECT_TOOL,
            "Attach one opaque Project snapshot at an exact position using the initiative revision. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio-admin/schemas/attach-project-request.schema.json"
            ),
        ),
        tool(
            UPDATE_PROJECT_SNAPSHOT_TOOL,
            "Refresh one attached Project snapshot using its membership revision. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio-admin/schemas/update-project-snapshot-request.schema.json"
            ),
        ),
        tool(
            DETACH_PROJECT_TOOL,
            "Detach one Project using current initiative and membership revisions. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio-admin/schemas/detach-project-request.schema.json"
            ),
        ),
        tool(
            REORDER_PROJECTS_TOOL,
            "Atomically replace the complete Project order using current initiative and membership revisions. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio-admin/schemas/reorder-projects-request.schema.json"
            ),
        ),
    ]
}

fn tool(name: &str, description: &str, schema: &str) -> ToolDefinition {
    let schema: serde_json::Value = serde_json::from_str(schema)
        .expect("Project Portfolio Admin Tool schema must be valid JSON");
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema_json: schema
            .to_string()
            .try_into()
            .expect("Project Portfolio Admin Tool schema must remain valid JSON"),
        execution: ToolExecutionClass::Exclusive,
    }
}

fn decode<T: DeserializeOwned>(request: &ExecuteRequest) -> PluginResult<T, ExecuteError> {
    serde_json::from_str(request.arguments_json.as_str())
        .map_err(|_| PluginError::domain(ExecuteError::InvalidArguments))
}

fn success<T: Serialize>(
    tool_name: &str,
    response: &T,
) -> PluginResult<ExecuteResponse, ExecuteError> {
    let content = serde_json::to_string_pretty(response).map_err(|error| {
        PluginError::runtime(RuntimeFailure::PluginFailure {
            detail: format!(
                "Project Portfolio Admin Tool could not serialize its typed response: {error}"
            ),
        })
    })?;
    Ok(ExecuteResponse {
        content_blocks: None,
        content,
        content_type: ContentType::Text,
        metadata_json: serde_json::json!({ "tool": tool_name })
            .to_string()
            .try_into()
            .expect("Project Portfolio Admin Tool metadata must be valid JSON"),
    })
}

trait DomainToolError {
    fn to_tool_error(&self) -> ExecuteError;
}
fn map_domain_error(error: &impl DomainToolError) -> ExecuteError {
    error.to_tool_error()
}

fn rejected(reason_code: &str) -> ExecuteError {
    ExecuteError::ExecutionFailed {
        payload: ExecutionFailedPayload {
            reason_code: reason_code.to_owned(),
            message: "Project Portfolio administration rejected the requested operation."
                .to_owned(),
            details_json: serde_json::json!({ "domain_error": reason_code })
                .to_string()
                .try_into()
                .expect("Project Portfolio Admin Tool error metadata must be valid JSON"),
        },
    }
}

macro_rules! impl_admin_error {
    ($($error:ty),+ $(,)?) => {$ (
        impl DomainToolError for $error {
            fn to_tool_error(&self) -> ExecuteError { match self {
                Self::InvalidRequest => ExecuteError::InvalidArguments,
                Self::NotFound => ExecuteError::NotFound,
                Self::Forbidden | Self::Unauthenticated => ExecuteError::PermissionDenied,
                Self::AlreadyAttached => rejected("already_attached"),
                Self::Archived => rejected("archived"),
                Self::IdempotencyConflict => rejected("idempotency_conflict"),
                Self::NotAttached => rejected("not_attached"),
                Self::OperationInProgress => rejected("operation_in_progress"),
                Self::PositionConflict => rejected("position_conflict"),
                Self::RevisionConflict => rejected("revision_conflict"),
                Self::Unknown(_) => rejected("unknown_domain_error"),
            }}
        }
    )+};
}

impl_admin_error!(
    admin::ArchiveInitiativeError,
    admin::AttachProjectError,
    admin::UpdateProjectSnapshotError,
    admin::DetachProjectError,
    admin::ReorderProjectsError,
);

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str, arguments: &str) -> ExecuteRequest {
        ExecuteRequest {
            name: name.to_owned(),
            arguments_json: arguments.try_into().unwrap(),
        }
    }

    #[test]
    fn descriptor_requires_only_the_admin_portfolio_capability() {
        let descriptor: serde_json::Value = serde_json::from_str(PLUGIN_DESCRIPTOR_JSON).unwrap();
        assert_eq!(
            descriptor["plugin_id"],
            "lenso.project-portfolio-admin.agent-tools"
        );
        assert_eq!(
            descriptor["provided_capabilities"][0]["capability_id"],
            "lenso.agent.tool-provider@2"
        );
        assert_eq!(
            descriptor["required_capabilities"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            descriptor["required_capabilities"][0]["capability_id"],
            "lenso.project-portfolio-admin@1"
        );
    }

    #[test]
    fn catalog_has_five_exclusive_admin_mutations() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 5);
        assert!(
            tools
                .iter()
                .all(|tool| tool.execution == ToolExecutionClass::Exclusive)
        );
        assert!(
            tools
                .iter()
                .all(|tool| serde_json::from_str::<serde_json::Value>(
                    tool.input_schema_json.as_str()
                )
                .unwrap()["additionalProperties"]
                    == false)
        );
    }

    #[test]
    fn exact_capability_requests_decode_without_adapter_owned_business_fields() {
        let archive = decode::<ArchiveInitiativeRequest>(&request(ARCHIVE_INITIATIVE_TOOL, r#"{"idempotency_key":"cmd-1","organization_id":"org-1","initiative_id":"init-1","expected_revision":"2"}"#)).unwrap();
        assert_eq!(archive.initiative_id, "init-1");
        assert!(
            decode::<ArchiveInitiativeRequest>(&request(
                ARCHIVE_INITIATIVE_TOOL,
                r#"{"initiative_id":42}"#
            ))
            .is_err()
        );
    }

    #[test]
    fn authorization_not_found_and_revision_failures_remain_distinct() {
        assert_eq!(
            map_domain_error(&admin::AttachProjectError::Forbidden),
            ExecuteError::PermissionDenied
        );
        assert_eq!(
            map_domain_error(&admin::AttachProjectError::NotFound),
            ExecuteError::NotFound
        );
        let ExecuteError::ExecutionFailed { payload } =
            map_domain_error(&admin::ReorderProjectsError::RevisionConflict)
        else {
            panic!("revision conflict must remain an execution failure");
        };
        assert_eq!(payload.reason_code, "revision_conflict");
    }
}
