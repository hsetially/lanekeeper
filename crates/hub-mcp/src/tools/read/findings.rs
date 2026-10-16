//! `find_inconsistencies`.

use domain::FindingsQuery;
use serde::Serialize;

use super::to_json;
use crate::tools::args::FindInconsistencies;
use crate::tools::dto::FindingDto;
use crate::tools::paging::{Resume, Start, paged_result};
use crate::tools::{CallCx, ToolResult};

pub async fn find_inconsistencies(cx: &CallCx<'_>, args: FindInconsistencies) -> ToolResult {
    let start = Start::read(
        cx.spec.name(),
        args.cursor.as_ref().map(|c| c.0.as_str()),
        args.limit.map(|l| l.0),
    )?;
    let swimlane = args.swimlane.map(|s| s.0);
    let query = FindingsQuery {
        swimlane: swimlane.clone(),
        kinds: args.kinds.unwrap_or_default(),
        min_severity: args.min_severity,
        page: start.port_page()?,
    };
    let paged = cx.host.registry.findings(cx.user, query).await?;
    let port_next = paged.next.map(|c| c.as_str().to_owned());
    let items: Vec<FindingDto> = paged
        .items
        .iter()
        .skip(start.skip as usize)
        .map(FindingDto::from)
        .collect();

    #[derive(Serialize)]
    struct Data<'a> {
        findings: &'a [FindingDto],
    }
    Ok(paged_result(
        &cx.shape(false),
        cx.spec.name(),
        &start,
        &Resume::Port { port_next },
        &items,
        |shown| to_json(&Data { findings: shown }),
        |s| {
            format!(
                "{} findings shown{}.",
                s.shown,
                if s.more { ", more follow" } else { "" }
            )
        },
        cx.host.links.findings(swimlane.as_ref()),
    ))
}
