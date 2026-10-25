//! The hub's container and pod contract (T1, S6, S16, S18): the `Dockerfile` and `HARDENING.md`.
//!
//! Prompt 09's chart renders the pods from `HARDENING.md`, and its own policy test checks the rendered chart. These tests
//! check what this prompt owns: that the image is distroless, non-root and pinned by digest, that the contract the chart is
//! told to implement is complete (S16) and consistent with what the code reads, and that the network contract allows only
//! the destinations and callers S18 names. Each linter is shown to reject a planted violation, so a pass means something.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::semicolon_if_nothing_returned
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

fn read(name: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap()
}

// ------------------------------------------------------------------------------------------ the Dockerfile

fn is_digest_pinned(reference: &str) -> bool {
    reference.split_once("@sha256:").is_some_and(|(_, digest)| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The instructions of a Dockerfile, with `\` continuation lines joined (comments and blank lines already removed).
fn instructions(lines: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in lines {
        if let Some(head) = line.strip_suffix('\\') {
            current.push_str(head.trim_end());
            current.push(' ');
        } else {
            current.push_str(line);
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The Debian release of a base image, as a number: `rust:1.88.0-trixie` and `gcr.io/distroless/cc-debian13` are both 13.
/// The binary is linked against the builder's glibc and runs on the runtime image's, so the two must be the same release
/// (09/T1; a base that Debian no longer fixes, such as `cc-debian12`, is moved on both sides together).
fn debian_release(reference: &str) -> Option<u32> {
    if let Some((_, rest)) = reference.split_once("/cc-debian") {
        return rest.split(':').next()?.parse().ok();
    }
    let tag = reference.split_once("rust:")?.1.split('@').next()?;
    match tag.rsplit('-').next()? {
        "bookworm" => Some(12),
        "trixie" => Some(13),
        _ => None,
    }
}

/// Why a Dockerfile does not meet S16. Empty when it does.
fn dockerfile_problems(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let word = |l: &&str| l.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    let froms: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| word(l) == "FROM")
        .map(|(i, _)| i)
        .collect();
    let Some(&last) = froms.last() else {
        return vec!["no FROM".to_owned()];
    };
    for &at in &froms {
        let reference = lines[at].split_whitespace().nth(1).unwrap_or("");
        if !is_digest_pinned(reference) {
            problems.push(format!("base image not pinned by digest: {}", lines[at]));
        }
    }
    let release = debian_release(lines[last].split_whitespace().nth(1).unwrap_or(""));
    for &at in &froms {
        let reference = lines[at].split_whitespace().nth(1).unwrap_or("");
        if reference.contains("/rust:") && (release.is_none() || debian_release(reference) != release) {
            problems.push(format!(
                "the builder and the runtime image are not the same Debian release: {}",
                lines[at]
            ));
        }
    }
    let final_base = lines[last].split_whitespace().nth(1).unwrap_or("");
    if !final_base.starts_with("gcr.io/distroless/") {
        problems.push(format!("the final image is not distroless: {final_base}"));
    }
    if final_base.contains("debug") || final_base.contains(":latest") {
        problems.push(format!(
            "the final image is a debug or floating tag: {final_base}"
        ));
    }
    let final_stage = &lines[last + 1..];
    match final_stage.iter().rev().find(|l| word(l) == "USER") {
        None => problems.push("the final stage never sets USER".to_owned()),
        Some(user) => {
            let who = user.split_whitespace().nth(1).unwrap_or("");
            let (uid, gid) = who.split_once(':').unwrap_or((who, ""));
            let numeric_nonroot = |s: &str| s.parse::<u32>().is_ok_and(|n| n != 0);
            if !numeric_nonroot(uid) || (!gid.is_empty() && !numeric_nonroot(gid)) {
                problems.push(format!("USER must be a numeric non-root uid and gid: {user}"));
            }
        }
    }
    // Only three things reach the final image, and each comes from a build stage: the binary, the web app and the
    // embedding model that the model stage checked against deploy/images/hub-model.lock (09/T1).
    let mut copied = BTreeSet::new();
    for line in final_stage {
        match word(line).as_str() {
            "RUN" | "ADD" => problems.push(format!("the final stage runs or fetches something: {line}")),
            "COPY" => {
                if line.contains("--from=") {
                    copied.insert(line.split_whitespace().last().unwrap_or("").to_owned());
                } else {
                    problems.push(format!("the final stage copies from the build context: {line}"));
                }
            }
            "ENV" | "ARG" => {
                let upper = line.to_ascii_uppercase();
                if ["SECRET", "TOKEN", "PASSWORD", "KEY", "CREDENTIAL"]
                    .iter()
                    .any(|m| upper.contains(m))
                {
                    problems.push(format!("a secret-looking variable in the image: {line}"));
                }
            }
            _ => {}
        }
    }
    let expected: BTreeSet<String> = ["/usr/local/bin/hub", "/app/web/dist", "/app/models"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if copied != expected {
        problems.push(format!(
            "the final image must hold exactly {expected:?}, it copies {copied:?}"
        ));
    }
    match final_stage.iter().find(|l| word(l) == "ENTRYPOINT") {
        None => problems.push("no ENTRYPOINT".to_owned()),
        Some(entry) => {
            let args = entry.trim_start_matches(|c: char| c.is_ascii_alphabetic()).trim();
            if !args.starts_with('[') || (args.contains("sh") && args.contains("-c")) {
                problems.push(format!(
                    "ENTRYPOINT must be the exec form of the binary alone: {entry}"
                ));
            }
        }
    }
    let build_runs: Vec<&&str> = lines[..last].iter().filter(|l| word(l) == "RUN").collect();
    if !build_runs.iter().any(|l| {
        l.contains("cargo build") && l.contains("--locked") && l.contains("--release") && l.contains("-p hub")
    }) {
        problems.push("the build must be `cargo build --locked --release -p hub`".to_owned());
    }
    if !build_runs
        .iter()
        .any(|l| l.contains("pnpm install") && l.contains("--frozen-lockfile"))
    {
        problems.push("the web build must install with `pnpm install --frozen-lockfile`".to_owned());
    }
    // The model files come only from the lock: a build stage reads deploy/images/hub-model.lock, and every file it fetches
    // is checked against the lock's sha256 before the final image can copy it (09/T1, S19).
    let model_stage_checks = instructions(&lines[..last])
        .iter()
        .any(|i| i.starts_with("RUN ") && i.contains("hub-model.lock") && i.contains("sha256sum -c"));
    if !model_stage_checks {
        problems
            .push("the model stage must verify every file against deploy/images/hub-model.lock".to_owned());
    }
    problems
}

#[test]
fn hub_dockerfile_is_hardened() {
    let text = read("Dockerfile");
    let problems = dockerfile_problems(&text);
    assert!(problems.is_empty(), "{problems:#?}");
    // Debian 13 throughout: `cc-debian12` is deprecated and carries critical findings that will not be fixed (plan 09 A32).
    assert!(
        text.contains("rust:1.88.0-trixie@sha256:"),
        "the builder is the toolchain in rust-toolchain.toml"
    );
    assert!(text.contains("gcr.io/distroless/cc-debian13:nonroot@sha256:"));
    let code: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    assert!(
        !code
            .iter()
            .any(|l| l.contains("bookworm") || l.contains("cc-debian12")),
        "no Debian 12 image is left"
    );
    assert!(
        text.contains("node:22"),
        "the web stage uses the Node major of web/.nvmrc"
    );
    let node = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/.nvmrc")).unwrap();
    assert!(
        text.contains(&format!("node:{}.", node.trim())),
        "web/.nvmrc and the Dockerfile disagree"
    );
    // The build context file keeps the repository's own history and the development tree out of the image build.
    let ignore = read("Dockerfile.dockerignore");
    for must in [".git", "target", "node_modules"] {
        assert!(ignore.lines().any(|l| l == must), "{must} is not ignored");
    }
    // deploy/ stays out of the build context except the model lock, which the model stage reads (09/T1).
    assert!(ignore.lines().any(|l| l == "deploy"), "deploy is not ignored");
    assert!(
        ignore.lines().any(|l| l == "!deploy/images/hub-model.lock"),
        "the model lock is not in the build context"
    );
    // The model's files land under /app/models/<name>/, the directory the hub's embedding code is pointed at.
    assert!(text.contains("COPY --from=model /models /app/models\n"));
}

#[test]
fn hardening_lint_rejects_planted_dockerfile_violations() {
    let good = read("Dockerfile");
    assert!(dockerfile_problems(&good).is_empty());
    let digest = good
        .lines()
        .find(|l| l.starts_with("FROM gcr.io/distroless"))
        .and_then(|l| l.split("@sha256:").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_owned();
    let planted = [
        good.replace(&format!("@sha256:{digest}"), ""),
        good.replace(
            "gcr.io/distroless/cc-debian13:nonroot",
            "gcr.io/distroless/cc-debian13:debug-nonroot",
        ),
        good.replace("gcr.io/distroless/cc-debian13:nonroot", "debian:13-slim"),
        // The runtime moves to another Debian release than the builder, or the builder does.
        good.replace(
            "gcr.io/distroless/cc-debian13:nonroot",
            "gcr.io/distroless/cc-debian12:nonroot",
        ),
        good.replace("rust:1.88.0-trixie@", "rust:1.88.0-bookworm@"),
        good.lines()
            .filter(|l| !l.starts_with("USER "))
            .collect::<Vec<_>>()
            .join("\n"),
        good.replace("USER 65532:65532", "USER root"),
        good.replace("USER 65532:65532", "USER 0"),
        good.replace(
            "ENTRYPOINT [\"/usr/local/bin/hub\"]",
            "ENTRYPOINT sh -c /usr/local/bin/hub",
        ),
        format!("{good}\nRUN apt-get install -y curl\n"),
        format!("{good}\nENV GITHUB_TOKEN=abc\n"),
        good.replace(" --locked -p hub", " -p hub"),
        good.replace("pnpm install --frozen-lockfile", "pnpm install"),
        format!("{good}\nCOPY . /app\n"),
        good.replace("COPY --from=web /src/web/dist /app/web/dist\n", ""),
        format!("{good}\nCOPY --from=build /src/.env /app/.env\n"),
        // 09/T1: the model comes from the checked model stage, never straight from the build context, and never unchecked.
        format!("{good}\nCOPY deploy/images/models /app/models\n"),
        good.replace("COPY --from=model /models /app/models\n", ""),
        good.replace("sha256sum -c", "sha256sum"),
    ];
    for (n, bad) in planted.iter().enumerate() {
        assert!(
            !dockerfile_problems(bad).is_empty(),
            "planted violation {n} was accepted:\n{bad}"
        );
    }
}

// ------------------------------------------------------------------------------------------ HARDENING.md

/// Every YAML document in the fenced `yaml` blocks of a Markdown text.
fn documents(markdown: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut open: Option<String> = None;
    for line in markdown.lines() {
        match (&mut open, line.trim_end()) {
            (None, "```yaml") => open = Some(String::new()),
            (Some(text), "```") => {
                blocks.push(std::mem::take(text));
                open = None;
            }
            (Some(text), l) => {
                text.push_str(l);
                text.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
        .iter()
        .flat_map(|block| block.split("\n---\n").map(str::to_owned).collect::<Vec<_>>())
        .filter(|d| !d.trim().is_empty())
        .map(|d| {
            serde_saphyr::from_str::<Value>(&d).unwrap_or_else(|e| panic!("YAML does not parse: {e}\n{d}"))
        })
        .collect()
}

fn of_kind<'a>(docs: &'a [Value], kind: &str) -> Vec<&'a Value> {
    docs.iter().filter(|d| d["kind"] == kind).collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// The `LK_*` variables the settings read: every string literal of that form in `src/config.rs`.
fn known_variables() -> BTreeSet<String> {
    read("src/config.rs")
        .split('"')
        .filter(|s| s.starts_with("LK_") && s.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'))
        .map(str::to_owned)
        .collect()
}

const WRITABLE_VOLUMES: [&str; 2] = ["tmp", "mirror"];
const ALLOWED_VOLUMES: [&str; 4] = ["tmp", "mirror", "db-credentials", "db-ca"];
const PORTS: [(&str, u64); 4] = [("web", 8443), ("agent", 8444), ("replica", 8445), ("ops", 9090)];

/// Why the pod in `HARDENING.md` does not meet S16. Empty when it does.
fn pod_problems(docs: &[Value]) -> Vec<String> {
    let pods = of_kind(docs, "Pod");
    let [pod] = pods.as_slice() else {
        return vec![format!("expected exactly one Pod, found {}", pods.len())];
    };
    let spec = &pod["spec"];
    let mut problems = pod_level_problems(spec);
    let containers = spec["containers"].as_array().cloned().unwrap_or_default();
    let [container] = containers.as_slice() else {
        problems.push(format!(
            "expected exactly one container, found {}",
            containers.len()
        ));
        return problems;
    };
    problems.extend(container_problems(container));
    problems.extend(volume_problems(spec, container));
    problems.extend(environment_problems(container));
    problems
}

fn pod_level_problems(spec: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    for field in ["hostNetwork", "hostPID", "hostIPC"] {
        if spec[field].as_bool().unwrap_or(false) {
            problems.push(format!("{field} is set"));
        }
    }
    if spec["automountServiceAccountToken"] != Value::Bool(false) {
        problems.push(
            "automountServiceAccountToken must be stated, and false: the hub never calls the Kubernetes API"
                .to_owned(),
        );
    }
    let security = &spec["securityContext"];
    if security["runAsNonRoot"] != Value::Bool(true) {
        problems.push("runAsNonRoot is not true".to_owned());
    }
    for field in ["runAsUser", "runAsGroup"] {
        if security[field].as_u64().is_none_or(|n| n == 0) {
            problems.push(format!("{field} must be a non-zero id"));
        }
    }
    if security["seccompProfile"]["type"] != "RuntimeDefault" {
        problems.push("the seccomp profile is not RuntimeDefault".to_owned());
    }
    problems
}

fn container_problems(container: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let context = &container["securityContext"];
    if context["allowPrivilegeEscalation"] != Value::Bool(false) {
        problems.push("allowPrivilegeEscalation is not false".to_owned());
    }
    if context["privileged"].as_bool().unwrap_or(false) {
        problems.push("privileged".to_owned());
    }
    if context["readOnlyRootFilesystem"] != Value::Bool(true) {
        problems.push("the root filesystem is not read-only".to_owned());
    }
    if strings(&context["capabilities"]["drop"]) != ["ALL"] || !context["capabilities"]["add"].is_null() {
        problems.push("capabilities must drop ALL and add none".to_owned());
    }
    for (section, field) in [
        ("requests", "cpu"),
        ("requests", "memory"),
        ("limits", "cpu"),
        ("limits", "memory"),
    ] {
        if container["resources"][section][field].is_null() {
            problems.push(format!("resources.{section}.{field} is not set"));
        }
    }
    if container["livenessProbe"]["httpGet"]["path"] != "/healthz"
        || container["readinessProbe"]["httpGet"]["path"] != "/readyz"
        || container["livenessProbe"]["httpGet"]["port"] != "ops"
        || container["readinessProbe"]["httpGet"]["port"] != "ops"
    {
        problems.push(
            "the probes must be /healthz (liveness) and /readyz (readiness) on the ops port".to_owned(),
        );
    }
    let ports = container["ports"].as_array().cloned().unwrap_or_default();
    if ports.iter().any(|p| !p["hostPort"].is_null()) {
        problems.push("a hostPort".to_owned());
    }
    let declared: BTreeSet<(String, u64)> = ports
        .iter()
        .filter_map(|p| Some((p["name"].as_str()?.to_owned(), p["containerPort"].as_u64()?)))
        .collect();
    let expected: BTreeSet<(String, u64)> = PORTS.iter().map(|(n, p)| ((*n).to_owned(), *p)).collect();
    if declared != expected {
        problems.push(format!(
            "the ports must be exactly {expected:?}, found {declared:?}"
        ));
    }
    problems
}

fn env_value(container: &Value, name: &str) -> Option<String> {
    container["env"]
        .as_array()
        .and_then(|e| e.iter().find(|v| v["name"] == name))
        .and_then(|v| v["value"].as_str().map(str::to_owned))
}

/// Volumes: exactly the four, none of them the host's, only two writable, and the paths the code writes to are exactly
/// those mounts.
fn volume_problems(spec: &Value, container: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let volumes = spec["volumes"].as_array().cloned().unwrap_or_default();
    let names: BTreeSet<String> = volumes
        .iter()
        .filter_map(|v| v["name"].as_str().map(str::to_owned))
        .collect();
    if names != ALLOWED_VOLUMES.iter().map(|s| (*s).to_owned()).collect() {
        problems.push(format!(
            "volumes must be exactly {ALLOWED_VOLUMES:?}, found {names:?}"
        ));
    }
    if volumes.iter().any(|v| !v["hostPath"].is_null()) {
        problems.push("a hostPath volume".to_owned());
    }
    let mounts = container["volumeMounts"].as_array().cloned().unwrap_or_default();
    let mount_path = |name: &str| {
        mounts
            .iter()
            .find(|m| m["name"] == name)
            .and_then(|m| m["mountPath"].as_str().map(str::to_owned))
    };
    for mount in &mounts {
        let name = mount["name"].as_str().unwrap_or("");
        let writable = mount["readOnly"] != Value::Bool(true);
        if writable && !WRITABLE_VOLUMES.contains(&name) {
            problems.push(format!("{name} is mounted writable"));
        }
    }
    if mount_path("tmp").as_deref() != Some("/tmp") {
        problems.push("the tmp volume is not mounted at /tmp".to_owned());
    }
    match (env_value(container, "LK_MIRROR_DIR"), mount_path("mirror")) {
        (Some(value), Some(path)) if value == path => {}
        (value, path) => problems.push(format!(
            "LK_MIRROR_DIR ({value:?}) is not the mirror mount ({path:?})"
        )),
    }
    for (variable, volume) in [
        ("LK_DATABASE_URL_FILE", "db-credentials"),
        ("LK_DB_CA_FILE", "db-ca"),
    ] {
        let inside = mount_path(volume)
            .zip(env_value(container, variable))
            .is_some_and(|(mount, file)| file.starts_with(&format!("{mount}/")));
        if !inside {
            problems.push(format!("{variable} is not a file inside the {volume} mount"));
        }
    }
    // A Secret that holds a password is readable by the group only (mode 0440).
    let credentials = volumes.iter().find(|v| v["name"] == "db-credentials");
    if credentials.is_none_or(|v| v["secret"]["defaultMode"].as_u64().is_none_or(|m| m & 0o007 != 0)) {
        problems
            .push("the db-credentials Secret must be a Secret volume that is not world-readable".to_owned());
    }
    problems
}

/// Every variable is one the hub reads. None holds a secret (they are names, paths and addresses), and none takes its value
/// from a Secret reference: secrets reach the hub as files and through Secret Manager.
fn environment_problems(container: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let known = known_variables();
    for env in container["env"].as_array().into_iter().flatten() {
        let name = env["name"].as_str().unwrap_or("");
        if !known.contains(name) {
            problems.push(format!(
                "{name} is not a variable the hub reads (a typo, or a secret in the environment)"
            ));
        }
        if !env["valueFrom"].is_null() {
            problems.push(format!(
                "{name} takes its value from a Secret or ConfigMap reference"
            ));
        }
        let upper = name.to_ascii_uppercase();
        if ["PASSWORD", "TOKEN", "SECRET", "CREDENTIAL"]
            .iter()
            .any(|m| upper.contains(m))
            || upper.ends_with("_KEY")
        {
            problems.push(format!("{name} looks like a secret in the environment"));
        }
        if name == "LK_DATABASE_URL" {
            problems.push(
                "LK_DATABASE_URL puts the database password in the environment; use LK_DATABASE_URL_FILE"
                    .to_owned(),
            );
        }
    }
    problems
}

const EXPECTED_HOSTS: [&str; 7] = [
    "login.microsoftonline.com",
    "github.com",
    "api.github.com",
    "secretmanager.googleapis.com",
    "cloudkms.googleapis.com",
    "www.googleapis.com",
    "<teams webhook host>",
];

/// The in-cluster egress destinations of the NetworkPolicy, as words, or why the policy is too open.
fn egress_destinations(policy: &Value) -> Result<BTreeSet<String>, String> {
    let rules = policy["spec"]["egress"].as_array().cloned().unwrap_or_default();
    let mut found = BTreeSet::new();
    for rule in &rules {
        let to = rule["to"].as_array().cloned().unwrap_or_default();
        let ports = rule["ports"].as_array().cloned().unwrap_or_default();
        if to.is_empty() || ports.is_empty() {
            return Err(format!(
                "an egress rule that allows every destination or every port: {rule}"
            ));
        }
        for destination in &to {
            let who = if let Some(cidr) = destination["ipBlock"]["cidr"].as_str() {
                if cidr != "169.254.169.254/32" || !destination["ipBlock"]["except"].is_null() {
                    return Err(format!("an IP block other than the metadata server: {cidr}"));
                }
                "metadata".to_owned()
            } else {
                let namespace =
                    destination["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"].as_str();
                let pod = &destination["podSelector"]["matchLabels"];
                match (
                    namespace,
                    pod["k8s-app"].as_str(),
                    pod["cnpg.io/cluster"].as_str(),
                    pod["app.kubernetes.io/name"].as_str(),
                ) {
                    (Some("kube-system"), Some("kube-dns"), None, None) => "dns".to_owned(),
                    (None, None, Some("lanekeeper-db"), None) => "postgres".to_owned(),
                    (None, None, None, Some("lanekeeper-hub")) => "replicas".to_owned(),
                    (Some("monitoring"), None, None, Some("otel-collector")) => "otel".to_owned(),
                    _ => return Err(format!("an unrecognised egress destination: {destination}")),
                }
            };
            for port in &ports {
                let protocol = port["protocol"].as_str().unwrap_or("TCP");
                let number = port["port"].as_u64().ok_or("a named or missing port")?;
                found.insert(format!("{who} {protocol}/{number}"));
            }
        }
    }
    Ok(found)
}

fn network_problems(docs: &[Value]) -> Vec<String> {
    let mut problems = Vec::new();
    let policies = of_kind(docs, "NetworkPolicy");
    let [policy] = policies.as_slice() else {
        return vec![format!(
            "expected exactly one NetworkPolicy, found {}",
            policies.len()
        )];
    };
    if strings(&policy["spec"]["policyTypes"])
        .iter()
        .collect::<BTreeSet<_>>()
        != ["Egress".to_owned(), "Ingress".to_owned()].iter().collect()
    {
        problems.push(
            "policyTypes must be Ingress and Egress, so that everything not listed is denied".to_owned(),
        );
    }
    if !policy["spec"]["podSelector"]["matchLabels"]["app.kubernetes.io/name"].is_string() {
        problems.push(
            "the policy must select the hub's pods by label (an empty selector is every pod)".to_owned(),
        );
    }
    match egress_destinations(policy) {
        Err(why) => problems.push(why),
        Ok(found) => {
            let expected: BTreeSet<String> = [
                "dns UDP/53",
                "dns TCP/53",
                "postgres TCP/5432",
                "replicas TCP/8445",
                "otel TCP/4317",
                "metadata TCP/80",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
            if found != expected {
                problems.push(format!("egress must be exactly {expected:?}, found {found:?}"));
            }
        }
    }
    // The external hosts: Entra, GitHub, Google APIs, the Teams webhook host. Names, no wildcards, HTTPS only.
    let fqdn = of_kind(docs, "FQDNNetworkPolicy");
    match fqdn.as_slice() {
        [policy] => {
            let rules = policy["spec"]["egress"].as_array().cloned().unwrap_or_default();
            let mut hosts = BTreeSet::new();
            for rule in &rules {
                let ports = rule["ports"].as_array().cloned().unwrap_or_default();
                if ports.is_empty() || ports.iter().any(|p| p["port"] != 443) {
                    problems.push(format!(
                        "an external egress rule on a port other than 443: {rule}"
                    ));
                }
                for m in rule["matches"].as_array().into_iter().flatten() {
                    let host = m["pattern"].as_str().or_else(|| m["name"].as_str()).unwrap_or("");
                    if host.contains('*') || host.is_empty() {
                        problems.push(format!("a wildcard or empty external host: {host:?}"));
                    }
                    hosts.insert(host.to_owned());
                }
            }
            let expected: BTreeSet<String> = EXPECTED_HOSTS.iter().map(|s| (*s).to_owned()).collect();
            if hosts != expected {
                problems.push(format!(
                    "external hosts must be exactly {expected:?}, found {hosts:?}"
                ));
            }
        }
        other => problems.push(format!(
            "expected exactly one FQDNNetworkPolicy, found {}",
            other.len()
        )),
    }
    // Ingress: exactly the four listeners, each from its own callers.
    let ingress = policy["spec"]["ingress"].as_array().cloned().unwrap_or_default();
    let mut by_port = std::collections::BTreeMap::new();
    for rule in &ingress {
        let ports: Vec<u64> = rule["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["port"].as_u64())
            .collect();
        let from = rule["from"].as_array().cloned().unwrap_or_default();
        if from.is_empty() || ports.len() != 1 {
            problems.push(format!(
                "an ingress rule must name its callers and exactly one port: {rule}"
            ));
            continue;
        }
        let caller = if from.iter().all(|d| {
            d["ipBlock"]["cidr"].as_str().is_some_and(|c| {
                c.rsplit('/')
                    .next()
                    .and_then(|p| p.parse::<u32>().ok())
                    .is_some_and(|p| p >= 16)
            })
        }) {
            "block"
        } else if from
            .iter()
            .all(|d| d["podSelector"]["matchLabels"]["app.kubernetes.io/name"] == "lanekeeper-hub")
        {
            "hub"
        } else if from
            .iter()
            .all(|d| d["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"] == "monitoring")
        {
            "monitoring"
        } else {
            "other"
        };
        by_port.insert(ports[0], caller);
    }
    let expected: std::collections::BTreeMap<u64, &str> = [
        (8443, "block"),
        (8444, "block"),
        (8445, "hub"),
        (9090, "monitoring"),
    ]
    .into();
    if by_port != expected {
        problems.push(format!("ingress must be {expected:?}, found {by_port:?}"));
    }
    if ingress.iter().any(|r| {
        r["from"]
            .as_array()
            .is_some_and(|f| f.iter().any(|d| d["ipBlock"]["cidr"] == "0.0.0.0/0"))
    }) {
        problems.push("ingress from 0.0.0.0/0".to_owned());
    }
    problems
}

fn contract() -> Vec<Value> {
    documents(&read("HARDENING.md"))
}

#[test]
fn hardening_md_pod_contract_is_complete() {
    let problems = pod_problems(&contract());
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn hardening_md_network_policy_denies_by_default_and_lists_only_allowed_egress() {
    let problems = network_problems(&contract());
    assert!(problems.is_empty(), "{problems:#?}");
}

/// A copy of the contract with `change` applied to the document of `kind`.
fn mutate(kind: &str, change: impl Fn(&mut Value)) -> Vec<Value> {
    let mut docs = contract();
    for doc in &mut docs {
        if doc["kind"] == kind {
            change(doc);
        }
    }
    docs
}

fn container(doc: &mut Value) -> &mut Value {
    &mut doc["spec"]["containers"][0]
}

#[test]
fn hardening_lint_rejects_planted_violations() {
    assert!(pod_problems(&contract()).is_empty());
    assert!(network_problems(&contract()).is_empty());

    let pod_cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "root",
            mutate("Pod", |d| {
                d["spec"]["securityContext"]["runAsNonRoot"] = Value::Bool(false)
            }),
        ),
        (
            "uid 0",
            mutate("Pod", |d| {
                d["spec"]["securityContext"]["runAsUser"] = Value::from(0)
            }),
        ),
        (
            "no seccomp",
            mutate("Pod", |d| {
                d["spec"]["securityContext"]["seccompProfile"]["type"] = "Unconfined".into()
            }),
        ),
        (
            "service-account token mounted",
            mutate("Pod", |d| {
                d["spec"]["automountServiceAccountToken"] = Value::Bool(true)
            }),
        ),
        (
            "token setting absent",
            mutate("Pod", |d| {
                d["spec"]
                    .as_object_mut()
                    .unwrap()
                    .remove("automountServiceAccountToken");
            }),
        ),
        (
            "host network",
            mutate("Pod", |d| d["spec"]["hostNetwork"] = Value::Bool(true)),
        ),
        (
            "writable root",
            mutate("Pod", |d| {
                container(d)["securityContext"]["readOnlyRootFilesystem"] = Value::Bool(false)
            }),
        ),
        (
            "privilege escalation",
            mutate("Pod", |d| {
                container(d)["securityContext"]["allowPrivilegeEscalation"] = Value::Bool(true)
            }),
        ),
        (
            "privileged",
            mutate("Pod", |d| {
                container(d)["securityContext"]["privileged"] = Value::Bool(true)
            }),
        ),
        (
            "capability added",
            mutate("Pod", |d| {
                container(d)["securityContext"]["capabilities"]["add"] = Value::from(vec!["NET_ADMIN"])
            }),
        ),
        (
            "capabilities kept",
            mutate("Pod", |d| {
                container(d)["securityContext"]["capabilities"]["drop"] = Value::from(vec!["NET_RAW"])
            }),
        ),
        (
            "no memory limit",
            mutate("Pod", |d| {
                container(d)["resources"]["limits"]
                    .as_object_mut()
                    .unwrap()
                    .remove("memory");
            }),
        ),
        (
            "no requests",
            mutate("Pod", |d| container(d)["resources"]["requests"] = Value::Null),
        ),
        (
            "probe path",
            mutate("Pod", |d| {
                container(d)["readinessProbe"]["httpGet"]["path"] = "/".into()
            }),
        ),
        (
            "host port",
            mutate("Pod", |d| container(d)["ports"][0]["hostPort"] = Value::from(443)),
        ),
        (
            "extra port",
            mutate("Pod", |d| {
                container(d)["ports"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": "debug", "containerPort": 6060}))
            }),
        ),
        (
            "host path volume",
            mutate("Pod", |d| {
                d["spec"]["volumes"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": "host", "hostPath": {"path": "/"}}))
            }),
        ),
        (
            "extra volume",
            mutate("Pod", |d| {
                d["spec"]["volumes"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": "scratch", "emptyDir": {}}))
            }),
        ),
        (
            "writable credentials",
            mutate("Pod", |d| {
                container(d)["volumeMounts"][2]["readOnly"] = Value::Bool(false)
            }),
        ),
        (
            "world-readable credentials",
            mutate("Pod", |d| {
                d["spec"]["volumes"][2]["secret"]["defaultMode"] = Value::from(0o644)
            }),
        ),
        (
            "mirror path differs",
            mutate("Pod", |d| container(d)["env"][6]["value"] = "/elsewhere".into()),
        ),
        (
            "a secret from a Secret reference",
            mutate("Pod", |d| {
                container(d)["env"].as_array_mut().unwrap().push(serde_json::json!({"name": "LK_DB_CA_FILE", "valueFrom": {"secretKeyRef": {"name": "s", "key": "k"}}}))
            }),
        ),
        (
            "a variable the hub does not read",
            mutate("Pod", |d| {
                container(d)["env"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": "LK_DEBUG_SHELL", "value": "1"}))
            }),
        ),
        (
            "the password in the environment",
            mutate("Pod", |d| {
                container(d)["env"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"name": "LK_DATABASE_URL", "value": "postgres://u:p@h/d"}))
            }),
        ),
        (
            "a second container",
            mutate("Pod", |d| {
                let c = container(d).clone();
                d["spec"]["containers"].as_array_mut().unwrap().push(c);
            }),
        ),
    ];
    for (label, docs) in &pod_cases {
        assert!(
            !pod_problems(docs).is_empty(),
            "planted pod violation accepted: {label}"
        );
    }

    let policy = |f: fn(&mut Value)| mutate("NetworkPolicy", f);
    let network_cases: Vec<(&str, Vec<Value>)> = vec![
        (
            "egress not denied by default",
            policy(|d| d["spec"]["policyTypes"] = Value::from(vec!["Ingress"])),
        ),
        (
            "empty pod selector",
            policy(|d| d["spec"]["podSelector"] = serde_json::json!({})),
        ),
        (
            "open egress rule",
            policy(|d| {
                d["spec"]["egress"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"to": [], "ports": [{"port": 443}]}))
            }),
        ),
        (
            "egress to the internet",
            policy(|d| {
                d["spec"]["egress"].as_array_mut().unwrap().push(serde_json::json!({"to": [{"ipBlock": {"cidr": "0.0.0.0/0"}}], "ports": [{"protocol": "TCP", "port": 443}]}))
            }),
        ),
        (
            "egress without ports",
            policy(|d| {
                let _ = d["spec"]["egress"][1].as_object_mut().unwrap().remove("ports");
            }),
        ),
        (
            "ingress from everywhere",
            policy(|d| d["spec"]["ingress"][0]["from"][0]["ipBlock"]["cidr"] = "0.0.0.0/0".into()),
        ),
        (
            "ingress on a fifth port",
            policy(|d| {
                d["spec"]["ingress"].as_array_mut().unwrap().push(serde_json::json!({"from": [{"ipBlock": {"cidr": "203.0.113.0/24"}}], "ports": [{"port": 6060}]}))
            }),
        ),
        (
            "replica port open to a block",
            policy(|d| {
                d["spec"]["ingress"][2]["from"] = serde_json::json!([{"ipBlock": {"cidr": "10.0.0.0/8"}}])
            }),
        ),
        (
            "a wildcard host",
            mutate("FQDNNetworkPolicy", |d| {
                d["spec"]["egress"][0]["matches"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"pattern": "*.example.com"}))
            }),
        ),
        (
            "an extra host",
            mutate("FQDNNetworkPolicy", |d| {
                d["spec"]["egress"][0]["matches"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"pattern": "pastebin.example"}))
            }),
        ),
        (
            "a missing host",
            mutate("FQDNNetworkPolicy", |d| {
                d["spec"]["egress"][0]["matches"].as_array_mut().unwrap().pop();
            }),
        ),
        (
            "a non-HTTPS port",
            mutate("FQDNNetworkPolicy", |d| {
                d["spec"]["egress"][0]["ports"][0]["port"] = Value::from(80)
            }),
        ),
    ];
    for (label, docs) in &network_cases {
        assert!(
            !network_problems(docs).is_empty(),
            "planted network violation accepted: {label}"
        );
    }
}
