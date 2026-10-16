//! `list_swimlanes`, `get_swimlane`, `get_pending_restarts`.

use domain::{ServiceState, SwimlaneSummary};
use serde::Serialize;

use super::to_json;
use crate::tools::args::{GetPendingRestarts, GetSwimlane, ListSwimlanes};
use crate::tools::dto::{ServiceStateDto, SwimlaneDto};
use crate::tools::error::{Code, ToolError};
use crate::tools::paging::{Resume, Start, capped_result, paged_result};
use crate::tools::{CallCx, ToolResult};

/// The most services `get_swimlane` lists: the contract's `maxItems`.
const MAX_SERVICES: usize = 100;

pub async fn list_swimlanes(cx: &CallCx<'_>, args: ListSwimlanes) -> ToolResult {
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let mut all = cx.host.registry.swimlanes(cx.user).await?;
    // The cursor is a key, so the order is ours and does not depend on the port's.
    all.sort_by(|a, b| a.id.cmp(&b.id));
    let total = all.len();
    let after = start.after_key();
    let remaining: Vec<&SwimlaneSummary> = all
        .iter()
        .filter(|s| after.is_none_or(|key| s.id.as_str() > key))
        .collect();
    let limit = start.limit as usize;
    let page: Vec<&SwimlaneSummary> = remaining.iter().copied().take(limit).collect();
    let more = remaining.len() > page.len();
    let keys: Vec<String> = page.iter().map(|s| s.id.as_str().to_owned()).collect();
    let items: Vec<SwimlaneDto> = page.iter().map(|s| SwimlaneDto::from(*s)).collect();

    #[derive(Serialize)]
    struct Data<'a> {
        swimlanes: &'a [SwimlaneDto],
    }
    Ok(paged_result(
        &cx.shape(false),
        cx.spec.name(),
        &start,
        &Resume::Keys { keys, more },
        &items,
        |shown| to_json(&Data { swimlanes: shown }),
        |s| format!("{} of {total} swimlanes shown.", s.shown),
        None,
    ))
}

pub async fn get_swimlane(cx: &CallCx<'_>, args: GetSwimlane) -> ToolResult {
    let id = args.swimlane.0;
    let (all, services, auto_notify) = tokio::join!(
        cx.host.registry.swimlanes(cx.user),
        cx.host.registry.pending_restarts(cx.user, &id),
        cx.host.registry.auto_notify(cx.user, &id),
    );
    let all = all?;
    let summary = all
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ToolError::new(Code::NotFound))?;
    let mut services = services?;
    // A tool that cannot say whether auto-notify is on says so. It never answers `false` for "I could not tell".
    let auto_notify = auto_notify?;
    services.sort_by(|a, b| a.service.cmp(&b.service));
    let more = services.len() > MAX_SERVICES;
    let items: Vec<ServiceStateDto> = services
        .iter()
        .take(MAX_SERVICES)
        .map(ServiceStateDto::from)
        .collect();
    let dto = SwimlaneDto::from(summary);

    #[derive(Serialize)]
    struct Data<'a> {
        swimlane: &'a SwimlaneDto,
        services: &'a [ServiceStateDto],
        auto_notify: bool,
    }
    let total = services.len();
    Ok(capped_result(
        &cx.shape(false),
        &items,
        more,
        |shown| {
            to_json(&Data {
                swimlane: &dto,
                services: shown,
                auto_notify,
            })
        },
        |s| format!("Swimlane {id}: {} of {total} services shown.", s.shown),
        cx.host.links.swimlane(&id),
    ))
}

pub async fn get_pending_restarts(cx: &CallCx<'_>, args: GetPendingRestarts) -> ToolResult {
    let id = args.swimlane.0;
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let mut all: Vec<ServiceState> = cx.host.registry.pending_restarts(cx.user, &id).await?;
    let key_of = |s: &ServiceState| format!("{}/{}", s.service.namespace(), s.service.name());
    // The cursor is this key, so the order is the key's, not `ServiceRef`'s (`app-x/a` sorts before `app/a` as text).
    all.sort_by_cached_key(key_of);
    let total = all.len();
    let after = start.after_key();
    let remaining: Vec<&ServiceState> = all
        .iter()
        .filter(|s| after.is_none_or(|key| key_of(s).as_str() > key))
        .collect();
    let limit = start.limit as usize;
    let page: Vec<&ServiceState> = remaining.iter().copied().take(limit).collect();
    let more = remaining.len() > page.len();
    let keys: Vec<String> = page.iter().map(|s| key_of(s)).collect();
    let items: Vec<ServiceStateDto> = page.iter().map(|s| ServiceStateDto::from(*s)).collect();

    #[derive(Serialize)]
    struct Data<'a> {
        services: &'a [ServiceStateDto],
    }
    Ok(paged_result(
        &cx.shape(false),
        cx.spec.name(),
        &start,
        &Resume::Keys { keys, more },
        &items,
        |shown| to_json(&Data { services: shown }),
        |s| format!("Swimlane {id}: {} of {total} services shown.", s.shown),
        cx.host.links.restarts(&id),
    ))
}
