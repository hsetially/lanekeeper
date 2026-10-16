//! The Cursor plugin package (plan 07, T4; D50, D52, A9).
//!
//! `plugin/package.sh` builds one package per authentication mode: the manifest, one `mcp.json`, and a copy of the skill of
//! prompt 11. These tests run the real builder and check **our own invariants**:
//!
//! - the `mcp.json` of a package has exactly one of `auth` or `headers`, never both (D52), and the personal-token package carries
//!   no sign-in or discovery metadata at all;
//! - the skill of prompt 11 is inside, byte for byte, and no token is;
//! - the builder refuses what it cannot build safely, and leaves nothing behind when it does.
//!
//! What these tests do **not** prove: that Cursor 3.22.12 accepts the manifest, the `mcp.json` shape, the `${env:...}`
//! interpolation or the marketplace layout. Nothing here could check that (cursor.com is not reachable from the sandbox that wrote
//! it, and T0 of D91 is NOT RUN), so `plugin/README.md` and `docs/cursor-admin.md` mark each such detail UNVERIFIED (A9).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const ORIGIN: &str = "https://lanekeeper.example.test";
const CLIENT_ID: &str = "00000000-0000-4000-8000-0000000000c1";
const AUDIENCE: &str = "api://00000000-0000-4000-8000-0000000000a1";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn plugin_dir() -> PathBuf {
    manifest_dir().join("plugin")
}

fn skill_dir() -> PathBuf {
    manifest_dir().join("../../skills/lanekeeper")
}

/// A fresh, empty scratch path that does not exist yet (the builder creates it). One per test, under the target directory.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("plugin_package")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    dir
}

/// The settings the builder needs for a mode. Synthetic values: no real tenant, client or host (AGENTS.md rule 12).
fn settings(mode: &str) -> Vec<(&'static str, &'static str)> {
    match mode {
        "oauth" => vec![
            ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
            ("LK_PLUGIN_CLIENT_ID", CLIENT_ID),
            ("LK_PLUGIN_API_AUDIENCE", AUDIENCE),
        ],
        _ => vec![("LK_PLUGIN_HUB_ORIGIN", ORIGIN)],
    }
}

/// Runs the real builder with an empty environment but `PATH`, so that nothing leaks in from the machine.
fn run_builder(mode: &str, out: &Path, env: &[(&str, &str)]) -> Output {
    Command::new("bash")
        .arg(plugin_dir().join("package.sh"))
        .arg(mode)
        .arg(out)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .envs(env.iter().copied())
        .output()
        .expect("bash must be available to build the package")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Builds a package and returns its directory; a failed build is a failed test with the builder's own message.
fn build_ok(mode: &str, name: &str) -> PathBuf {
    let out = scratch(name);
    let result = run_builder(mode, &out, &settings(mode));
    assert!(
        result.status.success(),
        "package.sh {mode} failed\nstdout: {}\nstderr: {}",
        text(&result.stdout),
        text(&result.stderr)
    );
    out
}

fn read_json(path: &Path) -> Value {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// Every file under `dir`, as `/`-separated paths relative to it.
fn files_under(dir: &Path) -> BTreeSet<String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink(), "{} is a symlink", path.display());
            if kind.is_dir() {
                walk(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap();
                out.insert(
                    rel.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(dir, dir, &mut out);
    out
}

/// The entries of `mcpServers` of an `mcp.json`.
fn servers(config: &Value) -> Vec<(&String, &Value)> {
    config
        .get("mcpServers")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("no mcpServers object in {config}"))
        .iter()
        .collect()
}

#[test]
fn package_has_exactly_one_of_auth_or_headers() {
    let oauth = build_ok("oauth", "one_mode_oauth");
    let config = read_json(&oauth.join("mcp.json"));
    let entries = servers(&config);
    assert_eq!(entries.len(), 1, "one MCP server per package: {config}");
    for (name, entry) in &entries {
        assert!(
            entry.get("auth").is_some(),
            "{name}: the OAuth package has no auth"
        );
        assert!(
            entry.get("headers").is_none(),
            "{name}: the OAuth package carries headers"
        );
    }

    let token = build_ok("personal_token", "one_mode_token");
    let config = read_json(&token.join("mcp.json"));
    let entries = servers(&config);
    assert_eq!(entries.len(), 1, "one MCP server per package: {config}");
    for (name, entry) in &entries {
        assert!(
            entry.get("headers").is_some(),
            "{name}: the personal-token package has no headers"
        );
        assert!(
            entry.get("auth").is_none(),
            "{name}: the personal-token package carries auth"
        );
    }
}

#[test]
fn the_two_source_templates_are_json_and_each_holds_one_mode_only() {
    for (file, has, lacks) in [
        ("mcp.oauth.json", "auth", "headers"),
        ("mcp.pat.json", "headers", "auth"),
    ] {
        let config = read_json(&plugin_dir().join(file));
        let entries = servers(&config);
        assert_eq!(entries.len(), 1, "{file}: one server");
        for (name, entry) in entries {
            assert!(entry.get(has).is_some(), "{file}/{name}: no {has}");
            assert!(entry.get(lacks).is_none(), "{file}/{name}: has {lacks} too (D52)");
        }
    }
}

#[test]
fn personal_token_package_publishes_no_discovery_or_sign_in_metadata() {
    let package = build_ok("personal_token", "no_discovery");
    // Everything that configures the connection: the MCP config and the manifest. (The skill is prose and may explain both modes.)
    for file in ["mcp.json", ".cursor-plugin/plugin.json"] {
        let content = fs::read_to_string(package.join(file)).unwrap().to_lowercase();
        for needle in [
            "client_id",
            "scopes",
            "offline_access",
            ".well-known",
            "api://",
            "login.microsoftonline",
            "oauth",
            "mcp.access",
            "tenant",
        ] {
            assert!(
                !content.contains(needle),
                "{file} of the personal-token package mentions {needle}"
            );
        }
    }
}

#[test]
fn oauth_package_carries_the_url_client_id_and_scopes_and_no_placeholder() {
    let package = build_ok("oauth", "oauth_values");
    let config = read_json(&package.join("mcp.json"));
    let (_, entry) = servers(&config).remove(0);
    assert_eq!(entry["url"], format!("{ORIGIN}/mcp"));
    assert_eq!(entry["auth"]["CLIENT_ID"], CLIENT_ID);
    assert_eq!(
        entry["auth"]["scopes"],
        serde_json::json!([format!("{AUDIENCE}/mcp.access"), "offline_access"])
    );
    assert_eq!(
        entry["auth"].as_object().unwrap().len(),
        2,
        "no other auth field is invented: {entry}"
    );
    for file in files_under(&package) {
        if file.starts_with("skills/") {
            continue;
        }
        let content = fs::read_to_string(package.join(&file)).unwrap();
        assert!(!content.contains('@'), "{file} still holds a build placeholder");
    }
}

#[test]
fn personal_token_package_reads_the_token_from_the_environment_and_holds_none() {
    let package = build_ok("personal_token", "token_reference");
    let config = read_json(&package.join("mcp.json"));
    let (_, entry) = servers(&config).remove(0);
    assert_eq!(entry["url"], format!("{ORIGIN}/mcp"));
    assert_eq!(
        entry["headers"],
        serde_json::json!({"Authorization": "Bearer ${env:LANEKEEPER_TOKEN}"}),
        "the token is read from the environment of the user's machine, never written into the package"
    );
    // No file of either package holds anything shaped like a personal token (`lkpat_` plus 256 bits is 43 characters).
    let oauth = build_ok("oauth", "token_scan_oauth");
    for (mode, dir) in [("personal_token", &package), ("oauth", &oauth)] {
        for file in files_under(dir) {
            let content = fs::read(dir.join(&file)).unwrap();
            let content = String::from_utf8_lossy(&content);
            for (at, _) in content.match_indices("lkpat_") {
                let run = content[at + "lkpat_".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                    .count();
                assert!(
                    run < 32,
                    "{mode}: {file} holds something shaped like a personal token"
                );
            }
        }
    }
}

#[test]
fn package_contains_the_skill() {
    let package = build_ok("oauth", "skill");
    let source = files_under(&skill_dir());
    assert!(
        source.contains("SKILL.md"),
        "prompt 11's skill must have a SKILL.md"
    );
    let packaged = files_under(&package.join("skills/lanekeeper"));
    assert_eq!(
        packaged, source,
        "the package holds exactly the files of skills/lanekeeper/"
    );
    for file in &source {
        assert_eq!(
            fs::read(package.join("skills/lanekeeper").join(file)).unwrap(),
            fs::read(skill_dir().join(file)).unwrap(),
            "{file} differs from the source"
        );
    }
    let skill = fs::read_to_string(package.join("skills/lanekeeper/SKILL.md")).unwrap();
    assert!(
        skill.starts_with("---\n"),
        "SKILL.md must open with its front matter"
    );
    assert!(
        skill.lines().take(20).any(|l| l.trim() == "name: lanekeeper"),
        "the front matter must name the skill `lanekeeper`"
    );
}

#[test]
fn manifest_is_valid_json_with_the_required_fields() {
    // "Required" here is what this package relies on (a name, a version, a description, an author). Cursor's own required set
    // could not be checked (A9, UNVERIFIED).
    let package = build_ok("oauth", "manifest");
    let manifest = read_json(&package.join(".cursor-plugin/plugin.json"));
    let object = manifest.as_object().expect("the manifest is an object");
    let name = object["name"].as_str().expect("name");
    assert!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !name.starts_with('-')
            && !name.ends_with('-'),
        "name must be a short kebab-case word: {name}"
    );
    assert_eq!(name, "lanekeeper");
    let version = object["version"].as_str().expect("version");
    let parts: Vec<&str> = version.split('.').collect();
    assert!(
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
        "version must be MAJOR.MINOR.PATCH: {version}"
    );
    let description = object["description"].as_str().expect("description");
    assert!(
        (20..=300).contains(&description.len()),
        "description should be a sentence: {description}"
    );
    assert!(
        object["author"].is_object() || object["author"].is_string(),
        "author is a string or an object"
    );
    // The manifest says nothing about how to authenticate: that is the `mcp.json`'s job, and one mode only lives there.
    for key in ["auth", "headers", "mcpServers"] {
        assert!(
            !object.contains_key(key),
            "the manifest must not carry {key}; the MCP entry lives in mcp.json"
        );
    }
}

#[test]
fn package_has_the_three_parts_and_nothing_else_at_the_top() {
    let package = build_ok("oauth", "layout");
    let top: BTreeSet<String> = fs::read_dir(&package)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let expected: BTreeSet<String> = [".cursor-plugin", "mcp.json", "skills"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(top, expected);
}

/// The builder must refuse these settings by naming `must_name`, and leave nothing at the target.
fn assert_refused(case: &str, mode: &str, env: &[(&str, &str)], must_name: &str) {
    let out = scratch(&format!("refuse_{case}"));
    let result = run_builder(mode, &out, env);
    let stderr = text(&result.stderr);
    assert!(!result.status.success(), "{case}: the builder accepted it");
    assert!(
        stderr.contains(must_name),
        "{case}: the message should name {must_name}, got: {stderr}"
    );
    assert!(!out.exists(), "{case}: the builder left {} behind", out.display());
}

#[test]
fn builder_refuses_a_missing_or_unknown_mode() {
    assert_refused("no_mode", "", &settings("oauth"), "mode");
    assert_refused("unknown_mode", "both", &settings("oauth"), "mode");
    // The settings of the server say `personal_token`; the builder takes the same word, not a second spelling.
    assert_refused(
        "hyphenated_mode",
        "personal-token",
        &settings("personal_token"),
        "mode",
    );
}

#[test]
fn builder_refuses_a_missing_setting_and_names_it() {
    assert_refused(
        "oauth_without_origin",
        "oauth",
        &[
            ("LK_PLUGIN_CLIENT_ID", CLIENT_ID),
            ("LK_PLUGIN_API_AUDIENCE", AUDIENCE),
        ],
        "LK_PLUGIN_HUB_ORIGIN",
    );
    assert_refused(
        "oauth_without_client_id",
        "oauth",
        &[
            ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
            ("LK_PLUGIN_API_AUDIENCE", AUDIENCE),
        ],
        "LK_PLUGIN_CLIENT_ID",
    );
    assert_refused(
        "oauth_without_audience",
        "oauth",
        &[
            ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
            ("LK_PLUGIN_CLIENT_ID", CLIENT_ID),
        ],
        "LK_PLUGIN_API_AUDIENCE",
    );
    assert_refused(
        "token_without_origin",
        "personal_token",
        &[],
        "LK_PLUGIN_HUB_ORIGIN",
    );
}

#[test]
fn builder_refuses_an_origin_that_is_not_a_plain_https_origin() {
    for (case, origin) in [
        ("plain_http", "http://lanekeeper.example.test"),
        ("a_path", "https://lanekeeper.example.test/mcp"),
        ("a_query", "https://lanekeeper.example.test?x=1"),
        ("user_info", "https://user:pw@lanekeeper.example.test"),
        ("upper_case", "https://Lanekeeper.example.test"),
        ("a_quote", "https://lanekeeper.example.test\",\"x\":\""),
        ("a_newline", "https://lanekeeper.example.test\nx"),
        ("an_ampersand", "https://a&b.example.test"),
        ("a_backslash", "https://a\\b.example.test"),
        ("a_space", "https://a b.example.test"),
        ("an_empty_label", "https://.example.test"),
        ("a_port_too_long", "https://lanekeeper.example.test:123456"),
    ] {
        assert_refused(
            &format!("origin_{case}"),
            "personal_token",
            &[("LK_PLUGIN_HUB_ORIGIN", origin)],
            "LK_PLUGIN_HUB_ORIGIN",
        );
    }
    // Plain http is for a machine's own address only, as the server's own public-origin rule has it.
    for (case, origin) in [
        ("localhost", "http://localhost:8080"),
        ("loopback", "http://127.0.0.1"),
    ] {
        let out = scratch(&format!("loopback_{case}"));
        let result = run_builder("personal_token", &out, &[("LK_PLUGIN_HUB_ORIGIN", origin)]);
        assert!(result.status.success(), "{case}: {}", text(&result.stderr));
    }
}

#[test]
fn builder_refuses_a_client_id_or_audience_that_is_not_valid() {
    assert_refused(
        "client_id_not_a_guid",
        "oauth",
        &[
            ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
            ("LK_PLUGIN_CLIENT_ID", "not-a-guid"),
            ("LK_PLUGIN_API_AUDIENCE", AUDIENCE),
        ],
        "LK_PLUGIN_CLIENT_ID",
    );
    for (case, audience) in [
        ("another_scheme", "https://example.test/api"),
        ("a_quote", "api://x\"y"),
        ("empty_after_scheme", "api://"),
        ("a_double_slash", "api://a//b"),
        ("a_space", "api://a b"),
    ] {
        assert_refused(
            &format!("audience_{case}"),
            "oauth",
            &[
                ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
                ("LK_PLUGIN_CLIENT_ID", CLIENT_ID),
                ("LK_PLUGIN_API_AUDIENCE", audience),
            ],
            "LK_PLUGIN_API_AUDIENCE",
        );
    }
}

#[test]
fn builder_refuses_sign_in_settings_in_a_personal_token_build() {
    // The two modes are never combined (D52): OAuth settings in a personal-token build are a mistake, not a hint.
    assert_refused(
        "client_id_in_a_token_build",
        "personal_token",
        &[
            ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
            ("LK_PLUGIN_CLIENT_ID", CLIENT_ID),
        ],
        "LK_PLUGIN_CLIENT_ID",
    );
    assert_refused(
        "audience_in_a_token_build",
        "personal_token",
        &[
            ("LK_PLUGIN_HUB_ORIGIN", ORIGIN),
            ("LK_PLUGIN_API_AUDIENCE", AUDIENCE),
        ],
        "LK_PLUGIN_API_AUDIENCE",
    );
}

#[test]
fn builder_never_echoes_a_value_it_refused() {
    let out = scratch("no_echo");
    let secret_looking = "https://leaky.example.test/path-with-secret-xyzzy";
    let result = run_builder(
        "personal_token",
        &out,
        &[("LK_PLUGIN_HUB_ORIGIN", secret_looking)],
    );
    assert!(!result.status.success());
    let all = format!("{}{}", text(&result.stdout), text(&result.stderr));
    assert!(
        all.contains("LK_PLUGIN_HUB_ORIGIN"),
        "the builder must refuse it by name: {all}"
    );
    assert!(!all.contains("xyzzy"), "a refused value was printed: {all}");
}

#[test]
fn builder_refuses_a_non_empty_target_and_does_not_touch_it() {
    let out = scratch("non_empty_target");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("keep.txt"), "mine").unwrap();
    let result = run_builder("oauth", &out, &settings("oauth"));
    assert!(!result.status.success(), "{}", text(&result.stderr));
    assert!(
        text(&result.stderr).contains("not empty"),
        "the builder must say why: {}",
        text(&result.stderr)
    );
    assert_eq!(fs::read_to_string(out.join("keep.txt")).unwrap(), "mine");
    assert_eq!(files_under(&out), BTreeSet::from(["keep.txt".to_owned()]));
}

#[test]
fn builder_builds_into_an_existing_empty_directory() {
    let out = scratch("empty_target");
    fs::create_dir_all(&out).unwrap();
    let result = run_builder("personal_token", &out, &settings("personal_token"));
    assert!(result.status.success(), "{}", text(&result.stderr));
    assert!(out.join("mcp.json").is_file());
}

#[test]
fn builder_refuses_a_missing_skill() {
    let skill = scratch("missing_skill_src");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("NOTES.md"), "no SKILL.md here").unwrap();
    let out = scratch("missing_skill_out");
    let mut env = settings("oauth");
    let skill_path = skill.to_string_lossy().into_owned();
    env.push(("LK_PLUGIN_SKILL_DIR", &skill_path));
    let result = run_builder("oauth", &out, &env);
    assert!(!result.status.success());
    assert!(
        text(&result.stderr).contains("SKILL.md"),
        "{}",
        text(&result.stderr)
    );
    assert!(!out.exists());

    // A skill folder that is not there at all is the same refusal.
    let gone = scratch("missing_skill_dir");
    let gone_path = gone.to_string_lossy().into_owned();
    let mut env = settings("oauth");
    env.push(("LK_PLUGIN_SKILL_DIR", &gone_path));
    let result = run_builder("oauth", &scratch("missing_skill_out2"), &env);
    assert!(!result.status.success());
    assert!(
        text(&result.stderr).contains("SKILL.md"),
        "{}",
        text(&result.stderr)
    );
}

#[cfg(unix)]
#[test]
fn builder_refuses_a_symlink_inside_the_skill() {
    // A link would put a file from outside the repository into a package that is handed to the whole organisation.
    let skill = scratch("symlink_skill_src");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("SKILL.md"), "---\nname: lanekeeper\n---\n").unwrap();
    std::os::unix::fs::symlink("/etc/hostname", skill.join("leak.md")).unwrap();
    let out = scratch("symlink_skill_out");
    let mut env = settings("oauth");
    let skill_path = skill.to_string_lossy().into_owned();
    env.push(("LK_PLUGIN_SKILL_DIR", &skill_path));
    let result = run_builder("oauth", &out, &env);
    assert!(!result.status.success());
    assert!(
        text(&result.stderr).contains("symlink"),
        "{}",
        text(&result.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn the_justfile_builds_the_package_with_the_mcp_plugin_recipe() {
    let justfile = fs::read_to_string(manifest_dir().join("../../Justfile")).unwrap();
    let mut lines = justfile.lines();
    let header = lines
        .by_ref()
        .find(|l| l.starts_with("mcp-plugin"))
        .expect("the Justfile has no mcp-plugin recipe");
    let mut recipe = String::from(header);
    for line in lines.take_while(|l| l.starts_with(' ') || l.starts_with('\t')) {
        recipe.push('\n');
        recipe.push_str(line);
    }
    assert!(recipe.contains("crates/hub-mcp/plugin/package.sh"), "{recipe}");
    assert!(recipe.contains("target/plugin"), "{recipe}");
    // The mode is a parameter, so one run builds one mode: a package never holds both.
    assert!(header.contains("mode"), "{header}");
}
