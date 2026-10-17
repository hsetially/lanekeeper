//! S10 for the read API (T6): logs hold ids, hashes and counts, never file contents, setting values, patterns, query strings, cursors,
//! what the config-server said, or secrets. Secret-looking values are planted where a read meets them (a file, a setting, a search
//! pattern, a query parameter, a path, a cursor, a header, the config-server's body and its error text) and every read is made, with
//! successes and failures, in the text search, the compare, the effective config, the grid, the served file and the plain lists; then
//! the database is taken away. None of the planted values may reach a log line, from any thread: the compare and the search run on
//! the blocking pool and the rayon pool, so the capture here is the process-wide one, and this binary holds one scenario.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::sync::Arc;

use bytes::Bytes;
use domain::{
    AgentReply, AppName, ChannelName, CompareQuery, CompareRef, Cursor, DriftQuery, DriftState,
    EffectiveQuery, HubCommand, LogicalFile, NfsPath, Page, ServeRequest, SettingPath, SwimlaneId, TenantId,
};
use ports::RegistryRead as _;
use support::http::{Http, SECRET};
use support::log_capture::LogCapture;

const PLANTED: [&str; 9] = [
    "PLANTED-PLAIN-VALUE-7a7a",
    "PLANTED-PATTERN-3c3c",
    "PLANTED-PARAM-5e5e",
    "PLANTED-CURSOR-9b9b",
    "PLANTED-ETAG-1d1d",
    "PLANTED-SERVED-BODY-2f2f",
    "PLANTED-SERVED-ERROR-6a6a",
    "PLANTED-PATH-8c8c",
    "planted-lane-4d4d",
];

#[tokio::test]
async fn planted_values_never_reach_a_log_line_of_a_read() {
    let capture = LogCapture::install_everywhere();
    let Some(mut h) = Http::seeded().await else { return };
    // A file with a plain planted value, and a setting in it.
    h.reads
        .world
        .nfs_change(&[(
            "planted.yml",
            &format!("name: {}\nnote: {}\n", PLANTED[0], PLANTED[0]),
        )])
        .await;
    h.reads.index_all().await;
    h.reads.service.clear_caches();
    let sit1 = SwimlaneId::parse("sit1").unwrap();
    let served_body = format!("name: {}\ntoken: {SECRET}\n", PLANTED[5]);
    h.reads.gateway.attach(
        &sit1,
        Arc::new(move |cmd: &HubCommand| match cmd {
            HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
                request_id: request_id.clone(),
                status: 200,
                bytes: Bytes::from(served_body.clone()),
            }),
            _ => None,
        }),
    );

    let enc = |s: &str| s.replace('-', "%2D");
    let uris: Vec<String> = vec![
        // Searches with the planted values as patterns, needles, globs and parameters.
        format!("/api/v1/search/text?pattern={}", PLANTED[0]),
        format!("/api/v1/search/text?pattern={}", PLANTED[1]),
        format!("/api/v1/search/text?pattern=%28{}&regex=true", PLANTED[1]),
        format!(
            "/api/v1/search/text?pattern={}&path_glob=%5B{}",
            PLANTED[1], PLANTED[1]
        ),
        format!("/api/v1/search/text?pattern=name&{}=1", PLANTED[2]),
        format!("/api/v1/search/settings?q={}", PLANTED[0]),
        format!("/api/v1/search/settings?q=note&swimlanes={}", PLANTED[8]),
        "/api/v1/search/settings?q=token".to_owned(),
        // Cursors that are forged, cut and swapped.
        format!("/api/v1/swimlanes/sit1/tree?cursor={}", PLANTED[3]),
        format!("/api/v1/findings?cursor={}", enc(PLANTED[3])),
        // Paths and ids that are hostile.
        format!("/api/v1/swimlanes/sit1/files/content?path={}", PLANTED[7]),
        format!("/api/v1/swimlanes/sit1/files/content?path=..%2F{}", PLANTED[7]),
        format!("/api/v1/swimlanes/{}/tree", PLANTED[8]),
        format!("/api/v1/swimlanes/{}/files/history?path=core.yml", PLANTED[8]),
        // Content, history, effective, compare, grid and the lists of what is really there.
        "/api/v1/swimlanes/sit1/files/content?path=planted.yml".to_owned(),
        "/api/v1/swimlanes/sit1/files/content?path=core.yml".to_owned(),
        "/api/v1/swimlanes/sit1/files/history?path=planted.yml".to_owned(),
        "/api/v1/swimlanes/sit1/effective?file=planted.yml&tenant=sit1".to_owned(),
        "/api/v1/swimlanes/sit1/effective?file=core.yml&tenant=sit1".to_owned(),
        "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml".to_owned(),
        "/api/v1/compare?left=nfs%3Asit1&right=effective%3Asit2&file=core.yml".to_owned(),
        "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=keys%2Fserver.pem".to_owned(),
        "/api/v1/compare/tree?left=nfs%3Asit1&right=nfs%3Asit2".to_owned(),
        "/api/v1/compare/tree?left=effective%3Asit1&right=effective%3Asit2".to_owned(),
        "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2,sit3".to_owned(),
        "/api/v1/grid?file=planted.yml&swimlanes=sit1,sit2".to_owned(),
        "/api/v1/swimlanes".to_owned(),
        "/api/v1/swimlanes/sit1".to_owned(),
        "/api/v1/swimlanes/sit1/tree".to_owned(),
        "/api/v1/swimlanes/sit1/drift".to_owned(),
        "/api/v1/findings".to_owned(),
        "/api/v1/swimlanes/sit1/pending-restarts".to_owned(),
        // The served file: the config-server's body, then its error.
        "/api/v1/swimlanes/sit1/served?application=core&tenant=sit1&file=core.yml".to_owned(),
    ];
    for uri in &uris {
        let reply = h.view(uri).await;
        assert!(reply.status.as_u16() < 600, "{uri}");
    }
    // The six methods of the port that MCP calls (`contract/07-mcp`): the same planted values in the places they take them (a setting
    // prefix, a file, a folder, a cursor, a request, a swimlane id), with answers and refusals.
    let v = support::reads::Reads::viewer();
    let service = &h.reads.service;
    let lane = |s: &str| SwimlaneId::parse(s).unwrap();
    let planted_file = LogicalFile::parse(PLANTED[7]).unwrap();
    let effective = |file: &LogicalFile, tenant: &str, prefix: &str| EffectiveQuery {
        file: file.clone(),
        tenant: TenantId::parse(tenant).unwrap(),
        channel: Some(ChannelName::parse(PLANTED[1]).unwrap()),
        setting_prefix: Some(SettingPath::parse(prefix).unwrap()),
    };
    let _ = service
        .effective_for(
            &v,
            &sit1,
            &effective(&LogicalFile::parse("planted.yml").unwrap(), "sit1", PLANTED[0]),
        )
        .await;
    let _ = service
        .effective_for(&v, &sit1, &effective(&planted_file, "sit1", PLANTED[1]))
        .await;
    let _ = service
        .effective_for(
            &v,
            &lane(PLANTED[8]),
            &effective(&planted_file, PLANTED[8], PLANTED[1]),
        )
        .await;
    let sides = |l: &str, r: &str| (l.parse::<CompareRef>().unwrap(), r.parse::<CompareRef>().unwrap());
    let (left, right) = sides("nfs:sit1", "nfs:sit2");
    for file in [LogicalFile::parse("core.yml").unwrap(), planted_file.clone()] {
        let _ = service
            .compare_in(
                &v,
                &CompareQuery {
                    left: left.clone(),
                    right: right.clone(),
                    file: Some(file),
                    channel: Some(ChannelName::parse(PLANTED[1]).unwrap()),
                },
            )
            .await;
    }
    let planted_folder = NfsPath::parse(PLANTED[7]).unwrap();
    let forged = Page::new(10, Some(Cursor::parse(PLANTED[3]).unwrap())).unwrap();
    let _ = service
        .compare_tree_under(
            &v,
            left.clone(),
            right.clone(),
            &planted_folder,
            Page::new(10, None).unwrap(),
        )
        .await;
    let _ = service
        .compare_tree_under(&v, left.clone(), right.clone(), &NfsPath::root(), forged.clone())
        .await;
    for (swimlane, state, prefix, page) in [
        (&sit1, None, Some(&planted_folder), Page::new(10, None).unwrap()),
        (&sit1, Some(DriftState::Conflict), None, forged.clone()),
        (&sit1, None, None, Page::new(10, None).unwrap()),
        (&lane(PLANTED[8]), None, None, Page::new(10, None).unwrap()),
    ] {
        let _ = service
            .drift(
                &v,
                swimlane,
                &DriftQuery {
                    state,
                    prefix: prefix.cloned(),
                },
                page,
            )
            .await;
    }
    let _ = service.auto_notify(&v, &sit1).await;
    let _ = service.auto_notify(&v, &lane(PLANTED[8])).await;
    for file in ["core.yml", PLANTED[7]] {
        let request = ServeRequest {
            application: AppName::parse("core").unwrap(),
            tenant: TenantId::parse("sit1").unwrap(),
            channel: Some(ChannelName::parse(PLANTED[1]).unwrap()),
            file: NfsPath::parse(file).unwrap(),
        };
        let _ = service.served(&v, &sit1, &request).await;
    }
    // A header is not a log line either.
    let tagged = h
        .get_with(
            Some("viewer"),
            "/api/v1/swimlanes/sit1/files/content?path=core.yml",
            &[("if-none-match", PLANTED[4])],
        )
        .await;
    assert_eq!(tagged.status, 200);
    // The config-server's error text.
    let error = PLANTED[6];
    h.reads.gateway.attach(
        &sit1,
        Arc::new(move |cmd: &HubCommand| match cmd {
            HubCommand::FetchServed { request_id, .. } => Some(AgentReply::Served {
                request_id: request_id.clone(),
                status: 500,
                bytes: Bytes::from(error.to_owned()),
            }),
            _ => None,
        }),
    );
    let broken = h
        .view("/api/v1/swimlanes/sit1/served?application=core&tenant=sit1&file=core.yml")
        .await;
    assert_eq!(broken.status, 503);
    let failing = ServeRequest {
        application: AppName::parse("core").unwrap(),
        tenant: TenantId::parse("sit1").unwrap(),
        channel: None,
        file: NfsPath::parse("core.yml").unwrap(),
    };
    assert_eq!(
        h.reads.service.served(&v, &sit1, &failing).await.unwrap_err(),
        ports::ReadError::Unavailable
    );

    // The database goes away: reads fail, and the lines they log are the registry's own.
    h.reads.world.rig.pool.close().await;
    h.reads.service.clear_caches();
    for uri in [
        "/api/v1/swimlanes/sit1/files/content?path=core.yml",
        "/api/v1/search/text?pattern=name",
        "/api/v1/search/settings?q=name",
        "/api/v1/grid?file=core.yml&swimlanes=sit1,sit2",
        "/api/v1/compare?left=nfs%3Asit1&right=nfs%3Asit2&file=core.yml",
    ] {
        let reply = h.view(uri).await;
        assert_eq!(reply.status, 503, "{uri}: {}", reply.text());
    }

    assert!(h.reads.service.auto_notify(&v, &sit1).await.is_err());
    assert!(
        h.reads
            .service
            .drift(
                &v,
                &sit1,
                &DriftQuery {
                    state: None,
                    prefix: None
                },
                Page::new(10, None).unwrap()
            )
            .await
            .is_err()
    );
    assert!(
        h.reads
            .service
            .effective_for(
                &v,
                &sit1,
                &effective(&LogicalFile::parse("core.yml").unwrap(), "sit1", "name")
            )
            .await
            .is_err()
    );

    let text = capture.text();
    let ours: Vec<&str> = text.lines().filter(|l| l.contains(" hub_registry")).collect();
    assert!(
        ours.iter()
            .any(|l| l.contains("database call failed") && l.contains("why=")),
        "the capture works and the failing database was logged: {ours:?}"
    );
    assert!(
        ours.iter()
            .any(|l| l.contains("the config-server did not serve the file")),
        "the failing config-server was logged, without its words: {ours:?}"
    );
    for planted in PLANTED.iter().copied().chain([SECRET, "s3cret"]) {
        assert!(!text.contains(planted), "{planted} reached the log: {text}");
    }
    for line in &ours {
        for sql in ["SELECT", "INSERT", "UPDATE", "unnest", "FROM "] {
            assert!(
                !line.contains(sql),
                "SQL text reached a log line of the registry: {line}"
            );
        }
    }
    // The lines name an operation and a reason in fixed words; they hold no part of a request.
    for line in ours {
        assert!(
            !line.contains("core.yml") && !line.contains("planted"),
            "a request detail is in: {line}"
        );
    }

    // Negative control: a planted value logged on the async thread, and one on the blocking pool, are both seen by the capture, so the
    // assertions above would have caught the same line from either.
    tracing::warn!("control: {}", PLANTED[0]);
    tokio::task::spawn_blocking(|| tracing::warn!("control on the blocking pool: {}", PLANTED[1]))
        .await
        .unwrap();
    let after = capture.text();
    assert!(after.contains(&format!("control: {}", PLANTED[0])));
    assert!(after.contains(&format!("control on the blocking pool: {}", PLANTED[1])));
}
