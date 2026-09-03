//! Agent-facing Tools over an explicitly bound Project Portfolio capability.

use lenso::prelude::*;
use lenso_capability_agent_tool_provider::{
    self as tool_contract, CatalogRequest, CatalogResponse, ContentType, ExecuteError,
    ExecuteRequest, ExecuteResponse, ExecutionFailedPayload, ToolDefinition, ToolExecutionClass,
};
use lenso_capability_project_portfolio::{
    self as portfolio, AddInitiativeUpdateRequest, CreateInitiativeRequest, GetInitiativeRequest,
    ListInitiativeProjectsRequest, ListInitiativeUpdatesRequest, ListInitiativesRequest,
    ReadRollupRequest, UpdateInitiativeRequest,
};
use lenso_kernel::RuntimeFailure;
use serde::{Serialize, de::DeserializeOwned};

pub const CREATE_INITIATIVE_TOOL: &str = "project_portfolio_create_initiative";
pub const GET_INITIATIVE_TOOL: &str = "project_portfolio_get_initiative";
pub const LIST_INITIATIVES_TOOL: &str = "project_portfolio_list_initiatives";
pub const UPDATE_INITIATIVE_TOOL: &str = "project_portfolio_update_initiative";
pub const LIST_INITIATIVE_PROJECTS_TOOL: &str = "project_portfolio_list_initiative_projects";
pub const ADD_INITIATIVE_UPDATE_TOOL: &str = "project_portfolio_add_initiative_update";
pub const LIST_INITIATIVE_UPDATES_TOOL: &str = "project_portfolio_list_initiative_updates";
pub const READ_ROLLUP_TOOL: &str = "project_portfolio_read_rollup";

#[lenso::plugin]
#[derive(Clone, Debug)]
struct ProjectPortfolioAgentToolsPlugin {
    portfolio: Port<portfolio::ProjectPortfolioClient>,
}

#[lenso::provides(tool_contract::ToolProvider)]
impl ProjectPortfolioAgentToolsPlugin {
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
            CREATE_INITIATIVE_TOOL => {
                let arguments = decode::<CreateInitiativeRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .create_initiative_with_context(context, arguments),
                    CREATE_INITIATIVE_TOOL,
                    portfolio::ProjectPortfolioCreateInitiativeInvocationError::Domain,
                    portfolio::ProjectPortfolioCreateInitiativeInvocationError::Runtime
                )
            }
            GET_INITIATIVE_TOOL => {
                let arguments = decode::<GetInitiativeRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .get_initiative_with_context(context, arguments),
                    GET_INITIATIVE_TOOL,
                    portfolio::ProjectPortfolioGetInitiativeInvocationError::Domain,
                    portfolio::ProjectPortfolioGetInitiativeInvocationError::Runtime
                )
            }
            LIST_INITIATIVES_TOOL => {
                let arguments = decode::<ListInitiativesRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .list_initiatives_with_context(context, arguments),
                    LIST_INITIATIVES_TOOL,
                    portfolio::ProjectPortfolioListInitiativesInvocationError::Domain,
                    portfolio::ProjectPortfolioListInitiativesInvocationError::Runtime
                )
            }
            UPDATE_INITIATIVE_TOOL => {
                let arguments = decode::<UpdateInitiativeRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .update_initiative_with_context(context, arguments),
                    UPDATE_INITIATIVE_TOOL,
                    portfolio::ProjectPortfolioUpdateInitiativeInvocationError::Domain,
                    portfolio::ProjectPortfolioUpdateInitiativeInvocationError::Runtime
                )
            }
            LIST_INITIATIVE_PROJECTS_TOOL => {
                let arguments = decode::<ListInitiativeProjectsRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .list_initiative_projects_with_context(context, arguments),
                    LIST_INITIATIVE_PROJECTS_TOOL,
                    portfolio::ProjectPortfolioListInitiativeProjectsInvocationError::Domain,
                    portfolio::ProjectPortfolioListInitiativeProjectsInvocationError::Runtime
                )
            }
            ADD_INITIATIVE_UPDATE_TOOL => {
                let arguments = decode::<AddInitiativeUpdateRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .add_initiative_update_with_context(context, arguments),
                    ADD_INITIATIVE_UPDATE_TOOL,
                    portfolio::ProjectPortfolioAddInitiativeUpdateInvocationError::Domain,
                    portfolio::ProjectPortfolioAddInitiativeUpdateInvocationError::Runtime
                )
            }
            LIST_INITIATIVE_UPDATES_TOOL => {
                let arguments = decode::<ListInitiativeUpdatesRequest>(&request)?;
                invoke!(
                    self.portfolio
                        .list_initiative_updates_with_context(context, arguments),
                    LIST_INITIATIVE_UPDATES_TOOL,
                    portfolio::ProjectPortfolioListInitiativeUpdatesInvocationError::Domain,
                    portfolio::ProjectPortfolioListInitiativeUpdatesInvocationError::Runtime
                )
            }
            READ_ROLLUP_TOOL => {
                let arguments = decode::<ReadRollupRequest>(&request)?;
                invoke!(
                    self.portfolio.read_rollup_with_context(context, arguments),
                    READ_ROLLUP_TOOL,
                    portfolio::ProjectPortfolioReadRollupInvocationError::Domain,
                    portfolio::ProjectPortfolioReadRollupInvocationError::Runtime
                )
            }
            _ => Err(PluginError::domain(ExecuteError::NotFound)),
        }
    }
}

fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        tool(
            GET_INITIATIVE_TOOL,
            "Get one visible initiative and its current revision.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/get-initiative-request.schema.json"
            ),
            ToolExecutionClass::ParallelSafe,
        ),
        tool(
            LIST_INITIATIVES_TOOL,
            "List visible initiatives with bounded stable pagination.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/list-initiatives-request.schema.json"
            ),
            ToolExecutionClass::ParallelSafe,
        ),
        tool(
            LIST_INITIATIVE_PROJECTS_TOOL,
            "List the ordered project snapshots attached to one initiative.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/list-initiative-projects-request.schema.json"
            ),
            ToolExecutionClass::ParallelSafe,
        ),
        tool(
            LIST_INITIATIVE_UPDATES_TOOL,
            "List structured health updates for one initiative with bounded pagination.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/list-initiative-updates-request.schema.json"
            ),
            ToolExecutionClass::ParallelSafe,
        ),
        tool(
            READ_ROLLUP_TOOL,
            "Read the current rollup derived from Portfolio-owned project snapshots.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/read-rollup-request.schema.json"
            ),
            ToolExecutionClass::ParallelSafe,
        ),
        tool(
            CREATE_INITIATIVE_TOOL,
            "Create one initiative with a stable ID. Reuse the same idempotency_key when retrying the same intent.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/create-initiative-request.schema.json"
            ),
            ToolExecutionClass::Exclusive,
        ),
        tool(
            UPDATE_INITIATIVE_TOOL,
            "Update one initiative using its current revision. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/update-initiative-request.schema.json"
            ),
            ToolExecutionClass::Exclusive,
        ),
        tool(
            ADD_INITIATIVE_UPDATE_TOOL,
            "Add one structured health update using the initiative revision. Reuse the same idempotency_key for retries.",
            include_str!(
                "../../lenso-capability-project-portfolio/schemas/add-initiative-update-request.schema.json"
            ),
            ToolExecutionClass::Exclusive,
        ),
    ]
}

fn tool(
    name: &str,
    description: &str,
    schema: &str,
    execution: ToolExecutionClass,
) -> ToolDefinition {
    let schema: serde_json::Value =
        serde_json::from_str(schema).expect("Project Portfolio Tool schema must be valid JSON");
    ToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema_json: schema
            .to_string()
            .try_into()
            .expect("Project Portfolio Tool schema must remain valid JSON"),
        execution,
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
                "Project Portfolio Tool could not serialize its typed response: {error}"
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
            .expect("Project Portfolio Tool metadata must be valid JSON"),
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
            message: "Project Portfolio rejected the requested operation.".to_owned(),
            details_json: serde_json::json!({ "domain_error": reason_code })
                .to_string()
                .try_into()
                .expect("Project Portfolio Tool error metadata must be valid JSON"),
        },
    }
}

macro_rules! impl_read_error {
    ($($error:ty),+ $(,)?) => {$ (
        impl DomainToolError for $error {
            fn to_tool_error(&self) -> ExecuteError { match self {
                Self::InvalidRequest => ExecuteError::InvalidArguments,
                Self::NotFound => ExecuteError::NotFound,
                Self::Forbidden | Self::Unauthenticated => ExecuteError::PermissionDenied,
                Self::Unknown(_) => rejected("unknown_domain_error"),
            }}
        }
    )+};
}

impl_read_error!(
    portfolio::GetInitiativeError,
    portfolio::ListInitiativesError,
    portfolio::ListInitiativeProjectsError,
    portfolio::ListInitiativeUpdatesError,
    portfolio::ReadRollupError,
);

macro_rules! impl_mutation_error {
    ($($error:ty),+ $(,)?) => {$ (
        impl DomainToolError for $error {
            fn to_tool_error(&self) -> ExecuteError { match self {
                Self::InvalidRequest => ExecuteError::InvalidArguments,
                Self::NotFound => ExecuteError::NotFound,
                Self::Forbidden | Self::Unauthenticated => ExecuteError::PermissionDenied,
                Self::AlreadyExists => rejected("already_exists"),
                Self::Archived => rejected("archived"),
                Self::IdempotencyConflict => rejected("idempotency_conflict"),
                Self::OperationInProgress => rejected("operation_in_progress"),
                Self::RevisionConflict => rejected("revision_conflict"),
                Self::Unknown(_) => rejected("unknown_domain_error"),
            }}
        }
    )+};
}

impl_mutation_error!(
    portfolio::CreateInitiativeError,
    portfolio::UpdateInitiativeError,
    portfolio::AddInitiativeUpdateError,
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
    fn descriptor_requires_only_the_ordinary_portfolio_capability() {
        let descriptor: serde_json::Value = serde_json::from_str(PLUGIN_DESCRIPTOR_JSON).unwrap();
        assert_eq!(
            descriptor["plugin_id"],
            "lenso.project-portfolio.agent-tools"
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
            "lenso.project-portfolio@1"
        );
    }

    #[test]
    fn catalog_has_five_parallel_reads_and_three_exclusive_mutations() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 8);
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool.execution == ToolExecutionClass::ParallelSafe)
                .count(),
            5
        );
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool.execution == ToolExecutionClass::Exclusive)
                .count(),
            3
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
        let get = decode::<GetInitiativeRequest>(&request(
            GET_INITIATIVE_TOOL,
            r#"{"organization_id":"org-1","initiative_id":"init-1"}"#,
        ))
        .unwrap();
        assert_eq!(get.initiative_id, "init-1");
        assert!(
            decode::<GetInitiativeRequest>(&request(
                GET_INITIATIVE_TOOL,
                r#"{"initiative_id":42}"#
            ))
            .is_err()
        );
    }

    #[test]
    fn authorization_not_found_and_revision_failures_remain_distinct() {
        assert_eq!(
            map_domain_error(&portfolio::GetInitiativeError::Forbidden),
            ExecuteError::PermissionDenied
        );
        assert_eq!(
            map_domain_error(&portfolio::GetInitiativeError::NotFound),
            ExecuteError::NotFound
        );
        let ExecuteError::ExecutionFailed { payload } =
            map_domain_error(&portfolio::UpdateInitiativeError::RevisionConflict)
        else {
            panic!("revision conflict must remain an execution failure");
        };
        assert_eq!(payload.reason_code, "revision_conflict");
    }
}
