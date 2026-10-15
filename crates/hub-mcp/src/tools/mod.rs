//! The tools (T2): typed arguments, output shapes, and the handlers behind them.
//!
//! A call arrives at [`ToolHost::run`] with a user whose role was just re-checked against the tool's floor, and with arguments that
//! were parsed into typed values. Read tools call `RegistryRead` and `DocSearch`; write tools call `WriteService` with `Via::Mcp`.
//! Facts come from those ports and from the engine, never from this crate (rule 7), and every text that came from a file or a doc
//! goes through the secret scan before it is part of a result (S14b).

use std::sync::Arc;
use std::time::Duration;

use domain::{Role, User};
use ports::{DocSearch, RegistryRead, Users, WriteService};
use rmcp::model::{Tool, ToolAnnotations};
use tokio::sync::watch;

use crate::clock::Clock;
use crate::config::{ConfirmMode, ContentMode, OutputSettings, ToolSettings};
use crate::confirm::Asker;
use crate::confirm::replay::Replay;
use crate::limits::{Class, Limits};
use crate::metrics::McpMetrics;
use crate::output::Redactor;
use crate::output::envelope::{Rendered, Shape};
use crate::output::links::WebLinks;
use crate::schemas::{Contract, SchemaError, ToolSpec};

pub mod args;
pub mod cache;
pub mod dto;
pub mod error;
pub mod paging;
pub mod read;
pub mod write;

pub use args::{ArgError, ToolArgs, ToolName, parse_args};
pub use error::{Code, ToolError};

/// The finished answer of a tool, or why there is none.
pub type ToolResult = Result<Rendered, ToolError>;

/// Everything the tools share. One per server; cheap to share by `Arc`.
pub struct ToolHost {
    pub contract: Contract,
    pub registry: Arc<dyn RegistryRead>,
    pub docs: Arc<dyn DocSearch>,
    pub writes: Arc<dyn WriteService>,
    pub users: Arc<dyn Users>,
    pub redactor: Redactor,
    pub links: WebLinks,
    pub limits: Limits,
    pub content_mode: ContentMode,
    pub confirm_mode: ConfirmMode,
    pub output: OutputSettings,
    pub tool_settings: ToolSettings,
    pub files: cache::FileCache,
    pub metrics: McpMetrics,
    pub clock: Arc<dyn Clock>,
    /// Finished writes, so that a retry is answered and not asked twice (D68).
    pub replay: Replay,
    /// The server's own stop signal: `true` once it is stopping. A question that is open is withdrawn when it goes (D91).
    pub stopping: watch::Receiver<bool>,
}

impl std::fmt::Debug for ToolHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolHost").finish_non_exhaustive()
    }
}

/// A tool as `tools/list` shows it, with the lowest role that sees it.
#[derive(Debug, Clone)]
pub struct Listed {
    pub name: ToolName,
    pub role: Role,
    pub tool: Tool,
}

/// Why the tools could not be set up: the contract and the handlers do not match. The server does not start (S4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("the tool contract is not usable")]
    Contract(#[from] SchemaError),
    #[error("a tool of the contract has no handler, or a handler has no tool in the contract")]
    Mismatch,
}

fn annotations(spec: &ToolSpec) -> ToolAnnotations {
    let read_only = !spec.acts_on_the_world() && spec.output_cap_bytes() > crate::schemas::WRITE_CAP_BYTES;
    let proposes = spec.name() == "propose_change";
    ToolAnnotations::new()
        .read_only(read_only)
        // A read tool destroys nothing; the draft of `propose_change` only adds a record; the rest can change or remove things.
        .destructive(!read_only && !proposes)
        .idempotent(read_only)
        .open_world(false)
}

impl ToolHost {
    /// Check that the contract and the handlers are the same set, and prepare what `tools/list` shows.
    ///
    /// # Errors
    ///
    /// [`HostError`] when they differ.
    pub fn listing(contract: &Contract, advertise_output_schemas: bool) -> Result<Vec<Listed>, HostError> {
        let names: std::collections::BTreeSet<&str> = ToolName::ALL.iter().map(|t| t.as_str()).collect();
        let specs: std::collections::BTreeSet<&str> = contract.tools().iter().map(ToolSpec::name).collect();
        if names != specs || ToolName::ALL.len() != contract.tools().len() {
            return Err(HostError::Mismatch);
        }
        contract
            .tools()
            .iter()
            .map(|spec| {
                let name = ToolName::from_name(spec.name()).ok_or(HostError::Mismatch)?;
                let mut tool = Tool::new(
                    spec.name().to_owned(),
                    spec.description().to_owned(),
                    Arc::clone(spec.input()),
                );
                if advertise_output_schemas {
                    tool = tool.with_raw_output_schema(Arc::clone(spec.result()));
                }
                Ok(Listed {
                    name,
                    role: spec.role(),
                    tool: tool.annotate(annotations(spec)),
                })
            })
            .collect()
    }

    /// The tools `user` may see: those whose lowest role the user has, and `notify_services` only while it is switched on (A16).
    #[must_use]
    pub fn visible<'a>(&'a self, listing: &'a [Listed], user: &User) -> Vec<&'a Tool> {
        listing
            .iter()
            .filter(|l| self.allowed(l.name, l.role, user))
            .map(|l| &l.tool)
            .collect()
    }

    /// Whether `user` may call the tool at all (the role of the record the caller has; the handler checks it again on the call).
    #[must_use]
    pub fn allowed(&self, name: ToolName, role: Role, user: &User) -> bool {
        user.allows(role) && (name != ToolName::NotifyServices || self.tool_settings.notify_services_enabled)
    }
}

/// What one call knows about itself.
pub struct CallCx<'a> {
    pub host: &'a ToolHost,
    pub user: &'a User,
    pub spec: &'a ToolSpec,
    /// The id quoted in an error and in the log.
    pub request_id: &'a str,
    /// The MCP session, as the client sent it (for idempotency keys, T3). `None` in a test that has none.
    pub session_id: Option<&'a str>,
    /// The JSON-RPC request id, for idempotency keys (T3).
    pub rpc_id: Option<&'a str>,
    /// Who can put a question to the user of this call. `None` in a test that has no client: such a call is in draft mode.
    pub asker: Option<&'a dyn Asker>,
}

impl std::fmt::Debug for CallCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallCx")
            .field("tool", &self.spec.name())
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

impl CallCx<'_> {
    /// How this tool's results are delivered: the contract's cap, lowered by configuration when it is.
    #[must_use]
    pub fn shape(&self, text_bearing: bool) -> Shape {
        let configured = match self.spec.output_cap_bytes() {
            n if n <= crate::schemas::WRITE_CAP_BYTES => self.host.output.write_cap_bytes,
            _ => self.host.output.read_cap_bytes,
        };
        Shape {
            mode: self.host.content_mode,
            cap: self.spec.output_cap_bytes().min(configured),
            text_bearing,
        }
    }

    /// How long a read may wait for the in-flight byte budget.
    #[must_use]
    pub fn byte_wait(&self) -> Duration {
        self.host.limits.timeout(Class::Read)
    }
}

impl ToolHost {
    /// Run a tool. The caller has already admitted the call (role, rate, permits) and holds the timeout.
    ///
    /// The last check of the cap is here: whatever a tool built, a result over the cap is not sent (D58). A tool that builds one
    /// has a defect, which `output_cap` tests look for at the worst case; this is what keeps the defect from reaching a client.
    pub async fn run(&self, cx: &CallCx<'_>, args: ToolArgs) -> ToolResult {
        let result = self.dispatch(cx, args).await;
        match result {
            Ok(rendered) if rendered.bytes > cx.shape(false).cap => {
                tracing::error!(target: "hub_mcp::call", tool = cx.spec.name(), bytes = rendered.bytes, "a result was over the cap and was not sent");
                Err(ToolError::new(Code::Internal))
            }
            other => other,
        }
    }

    async fn dispatch(&self, cx: &CallCx<'_>, args: ToolArgs) -> ToolResult {
        match args {
            ToolArgs::ListSwimlanes(a) => read::swimlanes::list_swimlanes(cx, a).await,
            ToolArgs::GetSwimlane(a) => read::swimlanes::get_swimlane(cx, a).await,
            ToolArgs::GetPendingRestarts(a) => read::swimlanes::get_pending_restarts(cx, a).await,
            ToolArgs::ListFiles(a) => read::files::list_files(cx, a).await,
            ToolArgs::ReadFile(a) => read::files::read_file(cx, a).await,
            ToolArgs::GetDrift(a) => read::files::get_drift(cx, a).await,
            ToolArgs::GetHistory(a) => read::files::get_history(cx, a).await,
            ToolArgs::GetEffectiveConfig(a) => read::effective::get_effective_config(cx, a).await,
            ToolArgs::GetServedConfig(a) => read::effective::get_served_config(cx, a).await,
            ToolArgs::Compare(a) => read::compare::compare(cx, a).await,
            ToolArgs::CompareTree(a) => read::compare::compare_tree(cx, a).await,
            ToolArgs::CompareGrid(a) => read::compare::compare_grid(cx, a).await,
            ToolArgs::SearchSettings(a) => read::search::search_settings(cx, a).await,
            ToolArgs::SearchText(a) => read::search::search_text(cx, a).await,
            ToolArgs::FindInconsistencies(a) => read::findings::find_inconsistencies(cx, a).await,
            ToolArgs::SearchDocs(a) => read::docs::search_docs(cx, a).await,
            ToolArgs::ReadDoc(a) => read::docs::read_doc(cx, a).await,
            ToolArgs::GrepDocs(a) => read::docs::grep_docs(cx, a).await,
            ToolArgs::GetDocsForFile(a) => read::docs::get_docs_for_file(cx, a).await,
            ToolArgs::GetFeatureStatus(a) => read::docs::get_feature_status(cx, a).await,
            ToolArgs::ProposeChange(a) => write::propose_change(cx, a).await,
            ToolArgs::ApplyChange(a) => write::apply_change(cx, a).await,
            ToolArgs::UploadFile(a) => write::upload_file(cx, a).await,
            ToolArgs::DeleteFile(a) => write::delete_file(cx, a).await,
            ToolArgs::RaisePr(a) => write::raise_pr(cx, a).await,
            ToolArgs::RestartService(a) => write::restart_service(cx, a).await,
            ToolArgs::NotifyServices(a) => write::notify_services(cx, a).await,
        }
    }

    /// The class of budget a tool draws on.
    #[must_use]
    pub fn class_of(spec: &ToolSpec) -> Class {
        if spec.output_cap_bytes() <= crate::schemas::WRITE_CAP_BYTES {
            Class::Write
        } else {
            Class::Read
        }
    }
}
