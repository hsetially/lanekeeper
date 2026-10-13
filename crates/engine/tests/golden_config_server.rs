//! T10 (D82, D83, acceptance 4): the config-server's own fixtures, ported.
//!
//! The inputs and the expected files under `tests/golden/config-server/` are the ones the config-server's tests use, as
//! `worked-examples.md` documents them, reconstructed (plan decision A3: the originals are in another repository). The test
//! reads them by name, so byte-exact originals can replace them without a code change. The comparison is after line endings
//! are normalised, as the acceptance says; a second run feeds CRLF inputs and gets the same files.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use domain::{AppName, ChannelName, FileClass, LogicalFile, NfsPath, TenantId, TenantSet};
use engine::{
    Limits, PropertyFile, PropertyView, Syntax, file_role, flatten, normalise_eol, parse, property_view,
    render_file, resolve_resource, search_locations,
};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/config-server")
}

fn nfs(s: &str) -> NfsPath {
    NfsPath::parse(s).unwrap()
}

/// The files below `config/`, as (path relative to it, bytes). `crlf` turns every line ending of every file into CRLF.
fn config_files(crlf: bool) -> Vec<(NfsPath, Bytes)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(NfsPath, Bytes)>, crlf: bool) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(base, &path, out, crlf);
            } else {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let mut data = fs::read(&path).unwrap();
                if crlf {
                    let text = String::from_utf8(data).unwrap();
                    data = text.replace("\r\n", "\n").replace('\n', "\r\n").into_bytes();
                }
                out.push((nfs(&rel), Bytes::from(data)));
            }
        }
    }
    let base = dir().join("config");
    let mut out = Vec::new();
    walk(&base, &base, &mut out, crlf);
    out
}

fn expected(name: &str) -> Bytes {
    Bytes::from(fs::read(dir().join("expected").join(name)).unwrap())
}

struct Request<'a> {
    app: &'a str,
    tenant: Option<&'a str>,
    channel: Option<&'a str>,
}

/// What the config-server serves for `file`: the property view for the request, the resource chosen, rendered.
fn serve(files: &[(NfsPath, Bytes)], request: &Request<'_>, file: &str) -> Bytes {
    let limits = Limits::default();
    let app = AppName::parse(request.app).unwrap();
    let tenant = request.tenant.map(|t| TenantId::parse(t).unwrap());
    let channels: Vec<ChannelName> = ["remoteteller"]
        .iter()
        .map(|c| ChannelName::parse(c).unwrap())
        .collect();
    let paths: Vec<NfsPath> = files.iter().map(|(p, _)| p.clone()).collect();
    let locations = search_locations(&app, tenant.as_ref(), &paths, &channels);

    // Every property source, parsed as the hub would parse a blob.
    let tenants = TenantSet::new(["bis", "sit1"].iter().map(|t| TenantId::parse(t).unwrap())).unwrap();
    let apps = [app.clone()];
    let mut parsed = Vec::new();
    for (path, data) in files {
        if file_role(path, &tenants, &apps) != domain::FileRole::PropertySource {
            continue;
        }
        let syntax = Syntax::of_path(path).unwrap();
        let text = String::from_utf8(data.to_vec()).unwrap();
        let flat = flatten(&parse(&text, syntax, &limits).unwrap(), &limits).unwrap();
        parsed.push((path.clone(), flat));
    }
    let property_files: Vec<PropertyFile<'_>> = parsed
        .iter()
        .map(|(path, flat)| PropertyFile {
            path,
            settings: &flat.settings,
        })
        .collect();
    let view: PropertyView =
        property_view(&app, tenant.as_ref(), &locations, &property_files, &limits).unwrap();

    let set: BTreeSet<NfsPath> = paths.into_iter().collect();
    let channel = request.channel.map(|c| ChannelName::parse(c).unwrap());
    let choice = resolve_resource(
        &LogicalFile::parse(file).unwrap(),
        tenant.as_ref(),
        channel.as_ref(),
        &set,
    )
    .expect("the requested file exists");
    let data = &files.iter().find(|(p, _)| *p == choice.path).unwrap().1;
    render_file(data, FileClass::Structured, &view, &limits)
        .unwrap()
        .bytes
}

fn same_after_normalising(got: &Bytes, want: &Bytes, what: &str) {
    assert_eq!(
        String::from_utf8(normalise_eol(got).to_vec()).unwrap(),
        String::from_utf8(normalise_eol(want).to_vec()).unwrap(),
        "{what}"
    );
}

const APP: &str = "tx-infinity-api";

#[test]
fn property_view_bis_core_routing_true_log_level_debug() {
    let limits = Limits::default();
    let files = config_files(false);
    let app = AppName::parse(APP).unwrap();
    let tenant = TenantId::parse("bis").unwrap();
    let paths: Vec<NfsPath> = files.iter().map(|(p, _)| p.clone()).collect();
    let locations = search_locations(&app, Some(&tenant), &paths, &[]);
    let parsed: Vec<_> = files
        .iter()
        .filter(|(p, _)| p.file_name().ends_with(".properties"))
        .map(|(p, d)| {
            let text = std::str::from_utf8(d).unwrap();
            (
                p.clone(),
                flatten(&parse(text, Syntax::Properties, &limits).unwrap(), &limits).unwrap(),
            )
        })
        .collect();
    let property_files: Vec<_> = parsed
        .iter()
        .map(|(path, flat)| PropertyFile {
            path,
            settings: &flat.settings,
        })
        .collect();
    let view = property_view(&app, Some(&tenant), &locations, &property_files, &limits).unwrap();
    let got: Vec<(&str, &str)> = view
        .iter()
        .map(|s| (s.key.as_str(), s.value.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("CORE_ROUTING", "true"),
            ("CREATE_VIRTUAL_ITEM", "false"),
            ("LOG_LEVEL", "DEBUG"),
        ],
        "the tenant file wins, key by key"
    );
    assert_eq!(
        view.get("LOG_LEVEL").unwrap().source,
        nfs("application-bis.properties")
    );

    // No tenant: the general file alone.
    let locations = search_locations(&app, None, &paths, &[]);
    let view = property_view(&app, None, &locations, &property_files, &limits).unwrap();
    let got: Vec<(&str, &str)> = view
        .iter()
        .map(|s| (s.key.as_str(), s.value.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("CORE_ROUTING", "false"),
            ("CREATE_VIRTUAL_ITEM", "true"),
            ("LOG_LEVEL", "INFO"),
        ]
    );
}

#[test]
fn tx_infinity_core_resolved_bis() {
    for crlf in [false, true] {
        let files = config_files(crlf);
        let got = serve(
            &files,
            &Request {
                app: APP,
                tenant: Some("bis"),
                channel: None,
            },
            "tx-infinity-core.yml",
        );
        same_after_normalising(&got, &expected("tx-infinity-core-resolved-bis.yml"), "bis");
    }
}

#[test]
fn tx_infinity_core_resolved_no_tenant() {
    for crlf in [false, true] {
        let files = config_files(crlf);
        let got = serve(
            &files,
            &Request {
                app: APP,
                tenant: None,
                channel: None,
            },
            "tx-infinity-core.yml",
        );
        same_after_normalising(&got, &expected("tx-infinity-core-resolved.yml"), "no tenant");
    }
}

#[test]
fn entry_group_config_resolved_bis_channel() {
    for crlf in [false, true] {
        let files = config_files(crlf);
        let got = serve(
            &files,
            &Request {
                app: APP,
                tenant: Some("bis"),
                channel: Some("remoteteller"),
            },
            "entryGroupConfig.yml",
        );
        same_after_normalising(&got, &expected("entryGroupConfig-resolved-bis.yml"), "channel");
    }
}

#[test]
fn a_request_without_the_channel_does_not_read_the_channel_folder() {
    let files = config_files(false);
    let got = serve(
        &files,
        &Request {
            app: APP,
            tenant: Some("bis"),
            channel: None,
        },
        "entryGroupConfig.yml",
    );
    let text = String::from_utf8(got.to_vec()).unwrap();
    assert!(text.contains("createVirtualItem: false"), "{text}");
    assert!(
        !text.contains("atm:") && !text.contains("withdrawals"),
        "the channel copy leaked into a request without a channel: {text}"
    );
}
