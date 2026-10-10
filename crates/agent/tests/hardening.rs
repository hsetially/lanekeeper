//! The agent's container and pod contract (T7, S16, S17, S18): the `Dockerfile` and `HARDENING.md`.
//!
//! Prompt 09's chart renders the pod from `HARDENING.md`, and its own policy test checks the rendered chart. These tests
//! check what this prompt owns: that the image is distroless, non-root and pinned by digest, and that the contract the
//! chart is told to implement is complete (S16), consistent with what the code writes (S17), and closed to everything but
//! the five places the agent talks to (S17, S18). Each linter is shown to reject a planted violation, so a pass means
//! something.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

/// Why a Dockerfile does not meet S16. Empty when it does.
fn dockerfile_problems(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let froms: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_ascii_uppercase().starts_with("FROM "))
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
    let word = |l: &&str| l.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
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
    for line in final_stage {
        match word(line).as_str() {
            "RUN" | "ADD" => problems.push(format!("the final stage runs or fetches something: {line}")),
            "COPY" if !line.contains("--from=") => {
                problems.push(format!("the final stage copies from the build context: {line}"));
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
    match final_stage.iter().find(|l| word(l) == "ENTRYPOINT") {
        None => problems.push("no ENTRYPOINT".to_owned()),
        Some(entry) => {
            let args = entry.trim_start_matches(|c: char| c.is_ascii_alphabetic()).trim();
            if !args.starts_with('[') || args.contains("sh") && args.contains("-c") {
                problems.push(format!(
                    "ENTRYPOINT must be the exec form of the binary alone: {entry}"
                ));
            }
        }
    }
    let build_runs: Vec<&&str> = lines[..last].iter().filter(|l| word(l) == "RUN").collect();
    if !build_runs.iter().any(|l| {
        l.contains("cargo build")
            && l.contains("--locked")
            && l.contains("--release")
            && l.contains("-p agent")
    }) {
        problems.push("the build must be `cargo build --locked --release -p agent`".to_owned());
    }
    problems
}

#[test]
fn dockerfile_is_distroless_nonroot_pinned() {
    let text = read("Dockerfile");
    let problems = dockerfile_problems(&text);
    assert!(problems.is_empty(), "{problems:#?}");
    // The two images are the ones this prompt chose, and the build and the runtime agree on the Debian release (glibc).
    assert!(
        text.contains("rust:1.88.0-bookworm@sha256:"),
        "the builder is the toolchain in rust-toolchain.toml"
    );
    assert!(text.contains("gcr.io/distroless/cc-debian12:nonroot@sha256:"));
    // The binary is the only thing in the final image.
    let copies = text
        .lines()
        .filter(|l| l.trim_start().to_ascii_uppercase().starts_with("COPY --FROM"))
        .count();
    assert_eq!(copies, 1);
}

#[test]
fn dockerfile_lint_rejects_planted_violations() {
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
            "gcr.io/distroless/cc-debian12:nonroot",
            "gcr.io/distroless/cc-debian12:debug-nonroot",
        ),
        good.replace("gcr.io/distroless/cc-debian12:nonroot", "debian:12-slim"),
        good.lines()
            .filter(|l| !l.starts_with("USER "))
            .collect::<Vec<_>>()
            .join("\n"),
        good.replace("USER 65532:65532", "USER root"),
        good.replace("USER 65532:65532", "USER 0"),
        good.replace("ENTRYPOINT [", "ENTRYPOINT sh -c [")
            .replace("sh -c [\"/usr/local/bin/agent\"]", "sh -c /usr/local/bin/agent"),
        format!("{good}\nRUN apt-get install -y curl\n"),
        format!("{good}\nENV GITHUB_TOKEN=abc\n"),
        good.replace("--locked ", ""),
        format!("{good}\nCOPY . /app\n"),
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

/// Writable mounts: the NFS export (the agent writes config files), the temp directory, and the spool.
const WRITABLE_VOLUMES: [&str; 3] = ["nfs", "tmp", "spool"];
const ALLOWED_VOLUMES: [&str; 4] = ["nfs", "tmp", "spool", "hub-ca"];

/// Why the pod in `HARDENING.md` does not meet S16 and S17. Empty when it does.
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
    if spec["automountServiceAccountToken"] != Value::Bool(true) {
        problems.push(
            "automountServiceAccountToken must be stated, and true: the agent calls the Kubernetes API"
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
    {
        problems.push("the probes must be /healthz (liveness) and /readyz (readiness)".to_owned());
    }
    if !container["ports"]
        .as_array()
        .is_some_and(|p| p.iter().all(|p| p["hostPort"].is_null()))
    {
        problems.push("a hostPort".to_owned());
    }
    problems
}

fn env_value(container: &Value, name: &str) -> Option<String> {
    container["env"]
        .as_array()
        .and_then(|e| e.iter().find(|v| v["name"] == name))
        .and_then(|v| v["value"].as_str().map(str::to_owned))
}

/// Volumes: exactly the four, none of them the host's, only three writable, and the paths the code writes to are
/// exactly those mounts (the source scan in `source_rules.rs` shows the code writes nowhere else).
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
    for (variable, volume) in [
        ("LK_NFS_ROOT", "nfs"),
        ("LK_TMP_DIR", "tmp"),
        ("LK_SPOOL_DIR", "spool"),
    ] {
        match (env_value(container, variable), mount_path(volume)) {
            (Some(value), Some(path)) if value == path => {}
            (value, path) => problems.push(format!(
                "{variable} ({value:?}) is not the {volume} mount ({path:?})"
            )),
        }
    }
    if mount_path("hub-ca")
        .is_none_or(|p| env_value(container, "LK_HUB_CA_FILE").is_none_or(|f| !f.starts_with(&p)))
    {
        problems.push("LK_HUB_CA_FILE is not inside the hub-ca mount".to_owned());
    }
    problems
}

/// Every variable is one the agent reads, and none of those holds a secret: they are names, paths and addresses.
/// (Tokens come from Workload Identity, or from a Secret the agent reads by name through the API.)
fn environment_problems(container: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let known = known_variables();
    for env in container["env"].as_array().into_iter().flatten() {
        let name = env["name"].as_str().unwrap_or("");
        if !known.contains(name) {
            problems.push(format!(
                "{name} is not a variable the agent reads (a typo, or a secret in the environment)"
            ));
        }
        if !env["valueFrom"].is_null() {
            problems.push(format!(
                "{name} takes its value from a Secret or ConfigMap reference"
            ));
        }
    }
    problems
}

/// How a destination is described, so that the list of egress rules can be compared with the five places the agent
/// talks to. IP blocks must be small (a /0, or anything wider than a /24, is "the internet").
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
                let prefix: u32 = cidr.rsplit('/').next().and_then(|p| p.parse().ok()).unwrap_or(0);
                if prefix < 24 || !destination["ipBlock"]["except"].is_null() {
                    return Err(format!(
                        "an IP block wider than a /24, or with exceptions: {cidr}"
                    ));
                }
                if cidr == "169.254.169.254/32" {
                    "metadata".to_owned()
                } else {
                    "https endpoint".to_owned()
                }
            } else if destination["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
                == "kube-system"
                && destination["podSelector"]["matchLabels"]["k8s-app"] == "kube-dns"
            {
                "dns".to_owned()
            } else if destination["namespaceSelector"].is_null()
                && destination["podSelector"]["matchLabels"]["app"] == "csp-configuration-server"
            {
                "config-server".to_owned()
            } else {
                return Err(format!("an unrecognised egress destination: {destination}"));
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
    match egress_destinations(policy) {
        Err(why) => problems.push(why),
        Ok(found) => {
            // The hub and the Kubernetes API (both HTTPS), the config-server (T12), the metadata server (Workload
            // Identity) and DNS. Nothing else (S17).
            let expected: BTreeSet<String> = [
                "https endpoint TCP/443",
                "config-server TCP/8888",
                "metadata TCP/80",
                "dns UDP/53",
                "dns TCP/53",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
            if found != expected {
                problems.push(format!("egress must be exactly {expected:?}, found {found:?}"));
            }
        }
    }
    let https_blocks = policy["spec"]["egress"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|rule| rule["to"].as_array().into_iter().flatten())
        .filter(|d| {
            d["ipBlock"]["cidr"]
                .as_str()
                .is_some_and(|c| c != "169.254.169.254/32")
        })
        .count();
    if https_blocks != 2 {
        problems.push(format!(
            "expected two IP blocks (the hub, the Kubernetes API), found {https_blocks}"
        ));
    }
    // Ingress: only the monitoring namespace, only the health port.
    let ingress = policy["spec"]["ingress"].as_array().cloned().unwrap_or_default();
    for rule in &ingress {
        let ports: Vec<u64> = rule["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["port"].as_u64())
            .collect();
        let from_monitoring = rule["from"].as_array().is_some_and(|f| {
            !f.is_empty()
                && f.iter().all(|d| {
                    d["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"] == "monitoring"
                })
        });
        if ports != [9090] || !from_monitoring {
            problems.push(format!(
                "ingress must be the monitoring namespace on 9090 only: {rule}"
            ));
        }
    }
    problems
}

fn hardening() -> String {
    read("HARDENING.md")
}

#[test]
fn hardening_contract_is_s16_complete() {
    let docs = documents(&hardening());
    let problems = pod_problems(&docs);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn hardening_contract_egress_is_hub_kube_configserver_metadata_dns_only() {
    let docs = documents(&hardening());
    let problems = network_problems(&docs);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn hardening_lint_rejects_planted_violations() {
    let good = hardening();
    let must_replace = |from: &str, to: &str| {
        assert!(
            good.contains(from),
            "the planted edit needs {from:?} in HARDENING.md"
        );
        good.replacen(from, to, 1)
    };
    let pod_cases = [
        must_replace("readOnlyRootFilesystem: true", "readOnlyRootFilesystem: false"),
        must_replace(
            "allowPrivilegeEscalation: false",
            "allowPrivilegeEscalation: true",
        ),
        must_replace("drop: [\"ALL\"]", "drop: [\"NET_RAW\"]"),
        must_replace("type: RuntimeDefault", "type: Unconfined"),
        must_replace("runAsNonRoot: true", "runAsNonRoot: false"),
        must_replace("privileged: false", "privileged: true"),
        must_replace(
            "name: hub-ca\n      readOnly: true",
            "name: hub-ca\n      readOnly: false",
        ),
        must_replace("emptyDir: {sizeLimit: 64Mi}", "hostPath: {path: /var/run}"),
        must_replace("limits:", "unlimited:"),
        must_replace("path: /readyz", "path: /"),
        must_replace("value: /var/lib/lanekeeper/spool", "value: /var/lib/elsewhere"),
        must_replace(
            "- name: LK_CLUSTER",
            "- name: LK_JOIN_TOKEN\n      value: literal-token\n    - name: LK_CLUSTER",
        ),
    ];
    for (n, bad) in pod_cases.iter().enumerate() {
        assert!(
            !pod_problems(&documents(bad)).is_empty(),
            "planted pod violation {n} was accepted"
        );
    }
    let network_cases = [
        // Anywhere on 443.
        must_replace("cidr: 203.0.113.10/32", "cidr: 0.0.0.0/0"),
        // A wider block for the Kubernetes API.
        must_replace("cidr: 198.51.100.2/32", "cidr: 10.0.0.0/8"),
        // Any destination on the config-server port: the `to` is dropped.
        must_replace(
            "- to:\n    - podSelector:\n        matchLabels:\n          app: csp-configuration-server\n    ports:",
            "- ports:",
        ),
        // The config-server on a second port.
        must_replace("port: 8888", "port: 8080"),
        // Ingress from anywhere.
        must_replace(
            "kubernetes.io/metadata.name: monitoring",
            "kubernetes.io/metadata.name: default",
        ),
        must_replace("policyTypes: [Ingress, Egress]", "policyTypes: [Ingress]"),
    ];
    for (n, bad) in network_cases.iter().enumerate() {
        assert!(
            !network_problems(&documents(bad)).is_empty(),
            "planted network violation {n} was accepted"
        );
    }
}
