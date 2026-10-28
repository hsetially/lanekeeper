{{/*
Labels. `app.kubernetes.io/name` alone selects the hub's pods in every NetworkPolicy (crates/hub/HARDENING.md); the instance label
tells two releases apart.
*/}}
{{- define "lanekeeper-hub.selectorLabels" -}}
app.kubernetes.io/name: lanekeeper-hub
{{- end }}

{{- define "lanekeeper-hub.podLabels" -}}
app.kubernetes.io/name: lanekeeper-hub
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: lanekeeper
{{- end }}

{{- define "lanekeeper-hub.labels" -}}
{{ include "lanekeeper-hub.podLabels" . }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
{{- end }}

{{/*
The image, by digest. A tag is never deployed (S19), so a digest that is not sha256:<64 hex digits> stops the render, whatever
the schema said.
*/}}
{{- define "lanekeeper-hub.image" -}}
{{- $digest := required "image.digest is required: a tag is never deployed (S19)" .Values.image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "image.digest %q is not sha256:<64 hex digits>: a tag is never deployed (S19)" $digest) -}}
{{- end -}}
{{- printf "%s@%s" .Values.image.repository $digest -}}
{{- end }}

{{/*
A list of source ranges that must not be empty and must not be the internet (S18, Q1). Takes (list "values.path" $ranges).
Prints nothing; stops the render.
*/}}
{{- define "lanekeeper-hub.requireRanges" -}}
{{- $name := index . 0 -}}
{{- $ranges := index . 1 -}}
{{- if not $ranges -}}
{{- fail (printf "%s is empty: an empty allowlist is an open port (S18, Q1)" $name) -}}
{{- end -}}
{{- range $ranges -}}
{{- if or (eq . "0.0.0.0/0") (eq . "::/0") (eq . "*") -}}
{{- fail (printf "%s has %s, which is the whole internet (S18)" $name .) -}}
{{- end -}}
{{- end -}}
{{- end }}

{{/*
The name of the StorageClass of the mirror volumes: the CMEK class this chart renders, or the cluster's own with a marker.
*/}}
{{- define "lanekeeper-hub.storageClassName" -}}
{{- if .Values.storage.cmekKey -}}lanekeeper-hub-cmek{{- else -}}{{ .Values.storage.fallbackClassName }}{{- end -}}
{{- end }}

{{/* The ports of the contract (HARDENING.md). */}}
{{- define "lanekeeper-hub.ports" -}}
- {name: web, containerPort: 8443, protocol: TCP}
- {name: agent, containerPort: 8444, protocol: TCP}
- {name: replica, containerPort: 8445, protocol: TCP}
- {name: ops, containerPort: 9090, protocol: TCP}
{{- end }}

{{/*
The pod's terminationGracePeriodSeconds: crates/hub/HARDENING.md and README.md (item 5) say 45. The hub fails /readyz, waits
LK_SHUTDOWN_GRACE_SECS (5), stops accepting, drains for LK_SHUTDOWN_DRAIN_SECS (25) and exits, and the period must be longer than the two
by 10 s, or the kubelet kills a hub that is still draining (lanekeeper-hub.requireSettings holds the tunables to that).
*/}}
{{- define "lanekeeper-hub.terminationGracePeriodSeconds" -}}45{{- end }}

{{/*
The startup probe's window: it asks /healthz every 2 s and the kubelet restarts the container after 30 failures, so a start that is still going
after 60 s is killed (statefulset.yaml). One place for the two numbers, because requireSettings checks LK_START_TIMEOUT_SECS against their product.
*/}}
{{- define "lanekeeper-hub.startupProbePeriodSeconds" -}}2{{- end }}
{{- define "lanekeeper-hub.startupProbeFailureThreshold" -}}30{{- end }}

{{/* A memory quantity in Gi or Mi as mebibytes (the units resources.rego reads); anything else stops the render. */}}
{{- define "lanekeeper-hub.mebibytes" -}}
{{- $q := toString .quantity -}}
{{- if hasSuffix "Gi" $q -}}{{ mulf (float64 (trimSuffix "Gi" $q)) 1024 | int }}
{{- else if hasSuffix "Mi" $q -}}{{ float64 (trimSuffix "Mi" $q) | int }}
{{- else -}}{{ fail (printf "%s is %q: the chart checks the caches against the memory limit and reads only Mi or Gi" .name $q) }}
{{- end -}}
{{- end }}

{{/*
The origin the users type: https://<gateway.hostname>. It is LK_PUBLIC_ORIGIN (the origin a request may name in Origin, and the host Entra
sends the browser back to) and LK_HUB_AUDIENCE (what the agents ask Google for a token for), so that the two cannot diverge. gateway.hostname
is required even where the Gateway is off (kind): the hub checks Host and Origin against it.
*/}}
{{- define "lanekeeper-hub.origin" -}}
{{- $host := required "gateway.hostname is required: LK_PUBLIC_ORIGIN and LK_HUB_AUDIENCE are https://<gateway.hostname>, and the hub does not start without them" .Values.gateway.hostname -}}
{{- printf "https://%s" $host -}}
{{- end }}

{{/*
A number as the hub reads it. Helm reads numbers as floating point, and Go prints a float of a million or more as 1e+06, which the hub
refuses; a whole number is printed whole.
*/}}
{{- define "lanekeeper-hub.scalar" -}}
{{- if and (kindIs "float64" .) (eq . (floor .)) -}}{{ int64 . }}{{- else -}}{{ . }}{{- end -}}
{{- end }}

{{/*
The settings the hub requires to start (REQUIRED_VARIABLES in crates/hub/tests/hardening.rs; exit 2 without one) that have no usable default.
Each stops the render and names the value, because "the hub exits 2" is otherwise found in a pod's log after the rollout. Prints nothing.
*/}}
{{- define "lanekeeper-hub.requireSettings" -}}
{{- $c := .Values.config -}}
{{- $_ := required "config.gcpProject is required: the Google project of the secrets and keys (LK_GCP_PROJECT)" $c.gcpProject -}}
{{- $_ := required "config.entra.tenantId is required: the Entra tenant, a GUID (LK_ENTRA_TENANT_ID)" $c.entra.tenantId -}}
{{- $_ := required "config.entra.clientId is required: the Entra app registration, a GUID (LK_ENTRA_CLIENT_ID)" $c.entra.clientId -}}
{{- $_ := required "config.kms.agentCa is required: the Cloud KMS key version of the agent CA (LK_AGENT_CA_KMS_REF)" $c.kms.agentCa -}}
{{- $_ := required "config.kms.replicaCa is required: the Cloud KMS key version of the replica CA (LK_REPLICA_CA_KMS_REF)" $c.kms.replicaCa -}}
{{- $_ := required "config.kms.auditCheckpoint is required: the Cloud KMS key version that signs the audit checkpoints (LK_AUDIT_CHECKPOINT_KMS_REF)" $c.kms.auditCheckpoint -}}
{{- $_ := required "config.kms.vault is required: the Cloud KMS crypto key, without a version, that wraps the GitHub tokens (LK_VAULT_KMS_REF)" $c.kms.vault -}}
{{- $_ := required "config.auditCheckpointBucket is required: the Cloud Storage bucket with a retention lock (LK_AUDIT_CHECKPOINT_BUCKET)" $c.auditCheckpointBucket -}}
{{- if not $c.agentEndpointHosts -}}
{{- fail "config.agentEndpointHosts is required: the DNS names the agents reach the agent endpoint by, at least one (LK_AGENT_ENDPOINT_HOSTS)" -}}
{{- end -}}
{{- $_ := required "config.git.baseUrl is required: the base repository, https (LK_GIT_BASE_URL)" $c.git.baseUrl -}}
{{- $_ := required "config.git.tenantUrl is required: the tenant repository, https (LK_GIT_TENANT_URL)" $c.git.tenantUrl -}}
{{- $_ := required "config.git.baseRepo is required: owner/name of the base repository as GitHub reports it (LK_GIT_BASE_REPO)" $c.git.baseRepo -}}
{{- $_ := required "config.git.tenantRepo is required: owner/name of the tenant repository as GitHub reports it (LK_GIT_TENANT_REPO)" $c.git.tenantRepo -}}
{{- /* S5: each listener trusts only its own CA. The hub refuses the same key or the same secret for both (exit 2); so does the chart. */ -}}
{{- if eq $c.kms.agentCa $c.kms.replicaCa -}}
{{- fail "config.kms.replicaCa is the same key version as config.kms.agentCa (LK_REPLICA_CA_KMS_REF): the two CAs are two keys, or an agent's certificate would be accepted as a replica's (S5)" -}}
{{- end -}}
{{- if eq $c.secretManager.agentCaCert $c.secretManager.replicaCaCert -}}
{{- fail "config.secretManager.replicaCaCert is the same secret as config.secretManager.agentCaCert (LK_REPLICA_CA_CERT_REF): the two CAs are two certificates (S5)" -}}
{{- end -}}
{{- /* The migrator's credential is never the application role's (S9, S21): the migration Job reads one Secret, the hub and the seed Job the other. */ -}}
{{- if eq .Values.database.migratorSecretName .Values.database.credentialsSecretName -}}
{{- fail "database.migratorSecretName is the same Secret as database.credentialsSecretName: the migrator's credential would be the application role's, and the hub's credential would change the schema (S9, S21)" -}}
{{- end -}}
{{- /* The OAuth mode of MCP needs an application ID URI (api://...) among the audiences; the hub refuses a list without one. */ -}}
{{- $tuning := $c.tuning -}}
{{- if and (hasKey $tuning "LK_ENTRA_API_AUDIENCES") (ne (index $tuning "LK_MCP_AUTH" | default "oauth") "personal_token") (not (regexMatch "(^|,)api://" (index $tuning "LK_ENTRA_API_AUDIENCES"))) -}}
{{- fail "config.tuning.LK_ENTRA_API_AUDIENCES has no api:// entry, which MCP in OAuth mode needs for its scope (LK_MCP_AUTH is oauth unless it is personal_token)" -}}
{{- end -}}
{{- /* T1: the hub fails /readyz, waits, drains and exits; the kubelet kills it when the period runs out first. The hub's defaults are 5 and 25. */ -}}
{{- $wait := 5 -}}
{{- if hasKey $tuning "LK_SHUTDOWN_GRACE_SECS" -}}{{- $wait = int (index $tuning "LK_SHUTDOWN_GRACE_SECS") -}}{{- end -}}
{{- $drain := 25 -}}
{{- if hasKey $tuning "LK_SHUTDOWN_DRAIN_SECS" -}}{{- $drain = int (index $tuning "LK_SHUTDOWN_DRAIN_SECS") -}}{{- end -}}
{{- $period := int (include "lanekeeper-hub.terminationGracePeriodSeconds" .) -}}
{{- if gt (add $wait $drain 10) $period -}}
{{- fail (printf "config.tuning: LK_SHUTDOWN_GRACE_SECS (%d) + LK_SHUTDOWN_DRAIN_SECS (%d) + 10 is %d s, and the pod's terminationGracePeriodSeconds is %d: the kubelet would kill a hub that is still draining" $wait $drain (add $wait $drain 10) $period) -}}
{{- end -}}
{{- /*
  The hub answers /healthz after the database, the schema, the web certificate and both CAs have loaded, each bounded by LK_START_TIMEOUT_SECS (8),
  and the kubelet restarts a container whose startup probe has not passed when its window ends. A bound that outlasts the window (with 10 s for
  the process to begin and the probe to ask) would have the pod killed before the hub's own timeout can name the step that is slow.
*/ -}}
{{- $window := mul (int (include "lanekeeper-hub.startupProbePeriodSeconds" .)) (int (include "lanekeeper-hub.startupProbeFailureThreshold" .)) -}}
{{- $start := 8 -}}
{{- if hasKey $tuning "LK_START_TIMEOUT_SECS" -}}{{- $start = int (index $tuning "LK_START_TIMEOUT_SECS") -}}{{- end -}}
{{- if gt (add $start 10) $window -}}
{{- fail (printf "config.tuning: LK_START_TIMEOUT_SECS (%d) + 10 s is %d s, which outlasts the startup probe's %d s: the kubelet would kill a hub that is still starting, before its own timeout names the step that is slow" $start (add $start 10) $window) -}}
{{- end -}}
{{- /*
  The caches fill memory. The request is the P11 budget for the hub at design scale with the default caches (blob 512 MiB, corpus 256 MiB), and the limit
  is the most the kubelet lets the container use: whatever the caches are raised above their defaults has to fit between the two, or a full cache
  gets the pod OOM-killed. (That the budget was measured with the default caches is the assumption; the schema alone allows 4 GiB for each.)
*/ -}}
{{- $blob := 512 -}}
{{- if hasKey $tuning "LK_BLOB_CACHE_MIB" -}}{{- $blob = int (index $tuning "LK_BLOB_CACHE_MIB") -}}{{- end -}}
{{- $corpus := 256 -}}
{{- if hasKey $tuning "LK_CORPUS_CACHE_MIB" -}}{{- $corpus = int (index $tuning "LK_CORPUS_CACHE_MIB") -}}{{- end -}}
{{- $request := int (include "lanekeeper-hub.mebibytes" (dict "name" "resources.requests.memory" "quantity" .Values.resources.requests.memory)) -}}
{{- $limit := int (include "lanekeeper-hub.mebibytes" (dict "name" "resources.limits.memory" "quantity" .Values.resources.limits.memory)) -}}
{{- $raised := sub (add $blob $corpus) 768 -}}
{{- if gt $raised (sub $limit $request) -}}
{{- fail (printf "config.tuning: LK_BLOB_CACHE_MIB (%d) + LK_CORPUS_CACHE_MIB (%d) is %d MiB above the defaults, and the memory limit (%d MiB) is only %d MiB above the request (%d MiB, the P11 budget): a full cache would get the pod OOM-killed" $blob $corpus $raised $limit (sub $limit $request) $request) -}}
{{- end -}}
{{- end }}

{{/*
The security context of the hub's pod and of its container, for the StatefulSet and for the Jobs of the same image alike (crates/hub/HARDENING.md:
"the hub pod's, unchanged"): non-root, a read-only root, no capabilities, RuntimeDefault, and the group that the Secret files (mode 0440) are read by.
*/}}
{{- define "lanekeeper-hub.podSecurityContext" -}}
runAsNonRoot: true
runAsUser: 65532
runAsGroup: 65532
fsGroup: 65532
fsGroupChangePolicy: OnRootMismatch
seccompProfile:
  type: RuntimeDefault
{{- end }}

{{- define "lanekeeper-hub.containerSecurityContext" -}}
allowPrivilegeEscalation: false
privileged: false
readOnlyRootFilesystem: true
capabilities:
  drop: ["ALL"]
{{- end }}

{{/*
The Jobs of the hub's image (`hub migrate`, `hub seed`) have a pod label of their own, not the hub's: the hub's NetworkPolicy selects
app.kubernetes.io/name: lanekeeper-hub and would give them the other replicas, the metadata server and the external hosts. Their own policy
selects this label, and the database's policy admits it on 5432 (charts/db).
*/}}
{{- define "lanekeeper-hub.jobSelectorLabels" -}}
app.kubernetes.io/name: lanekeeper-hub-jobs
{{- end }}

{{/* Takes (list $ "migrate"|"seed"|"jobs"). */}}
{{- define "lanekeeper-hub.jobPodLabels" -}}
{{ include "lanekeeper-hub.jobSelectorLabels" (index . 0) }}
app.kubernetes.io/instance: {{ (index . 0).Release.Name }}
app.kubernetes.io/part-of: lanekeeper
app.kubernetes.io/component: {{ index . 1 }}
{{- end }}

{{- define "lanekeeper-hub.jobLabels" -}}
{{ include "lanekeeper-hub.jobPodLabels" . }}
app.kubernetes.io/managed-by: {{ (index . 0).Release.Service }}
helm.sh/chart: {{ printf "%s-%s" (index . 0).Chart.Name (index . 0).Chart.Version | quote }}
{{- end }}

{{/*
Helm hooks of the events before the hub rolls out, and the Argo CD sync wave that Argo CD makes of the weight (Argo CD runs pre-install and
pre-upgrade hooks as PreSync; the weight orders the hooks of a phase). Takes the weight, a string: the Jobs' ServiceAccount and NetworkPolicy
have -10 and the migration Job 0, so that on a first install, when nothing else of the chart exists yet, the migration pod has both. They
are kept (before-hook-creation, never hook-succeeded): the seed Job, which is not a hook, runs as that account and under that policy.
*/}}
{{- define "lanekeeper-hub.jobHook" -}}
helm.sh/hook: pre-install,pre-upgrade
helm.sh/hook-weight: {{ . | quote }}
helm.sh/hook-delete-policy: before-hook-creation
argocd.argoproj.io/sync-wave: {{ . | quote }}
{{- end }}

{{/*
The seconds a Job may take beyond the timeout it tells its process (LK_MIGRATE_TIMEOUT_SECS, LK_SEED_TIMEOUT_SECS): pulling the image and
starting the process. The deadline is longer than the timeout, so that the process ends itself with a message that names the step.
*/}}
{{- define "lanekeeper-hub.jobDeadlineMargin" -}}300{{- end }}

{{/*
The pod template of a Job of the hub's image (the part under `template:`). The command is the image's ENTRYPOINT, the hub; the Job passes
only `args`. Takes (dict "root" $ "kind" "migrate"|"seed" "configMap" <the ConfigMap that holds the seed file>).

Four volumes at most (crates/hub/HARDENING.md): /tmp, the database CA and the one credential (the migrator's for `migrate`, the application
role's for `seed`, each mounted through its `uri` key only, mode 0440), and for `seed` its file from a ConfigMap, read-only. No service-account
token, no Google identity: the account is the chart's own, with neither.
*/}}
{{- define "lanekeeper-hub.jobPodTemplate" -}}
{{- $root := .root -}}
{{- $kind := .kind -}}
metadata:
  labels:
    {{- include "lanekeeper-hub.jobPodLabels" (list $root $kind) | nindent 4 }}
spec:
  serviceAccountName: lanekeeper-hub-jobs
  automountServiceAccountToken: false
  enableServiceLinks: false
  restartPolicy: Never
  {{- with $root.Values.imagePullSecrets }}
  imagePullSecrets:
    {{- range . }}
    - name: {{ . | quote }}
    {{- end }}
  {{- end }}
  securityContext:
    {{- include "lanekeeper-hub.podSecurityContext" $root | nindent 4 }}
  containers:
    - name: {{ $kind }}
      image: {{ include "lanekeeper-hub.image" $root | quote }}
      imagePullPolicy: IfNotPresent
      {{- if eq $kind "migrate" }}
      args: ["migrate"]
      {{- else }}
      args: ["seed", "/etc/lanekeeper/seed/seed.yaml"]
      {{- end }}
      securityContext:
        {{- include "lanekeeper-hub.containerSecurityContext" $root | nindent 8 }}
      resources:
        {{- toYaml $root.Values.jobs.resources | nindent 8 }}
      env:
        {{- if eq $kind "migrate" }}
        # The migrator's connection URL is a mounted file, never a variable (S7). Only this Job has it (the hub refuses it in the pod).
        - name: LK_MIGRATOR_URL_FILE
          value: /etc/lanekeeper/migrator/uri
        {{- else }}
        - name: LK_DATABASE_URL_FILE
          value: /etc/lanekeeper/db/uri
        {{- end }}
        - name: LK_DB_CA_FILE
          value: /etc/lanekeeper/db-ca/ca.crt
        {{- if eq $kind "migrate" }}
        - name: LK_MIGRATE_TIMEOUT_SECS
          value: {{ $root.Values.migrate.timeoutSeconds | int | toString | quote }}
        {{- else }}
        - name: LK_SEED_TIMEOUT_SECS
          value: {{ $root.Values.seed.timeoutSeconds | int | toString | quote }}
        {{- end }}
      volumeMounts:
        - name: tmp
          mountPath: /tmp
        {{- if eq $kind "migrate" }}
        - name: migrator-credentials
          mountPath: /etc/lanekeeper/migrator
          readOnly: true
        {{- else }}
        - name: db-credentials
          mountPath: /etc/lanekeeper/db
          readOnly: true
        {{- end }}
        - name: db-ca
          mountPath: /etc/lanekeeper/db-ca
          readOnly: true
        {{- if eq $kind "seed" }}
        - name: seed
          mountPath: /etc/lanekeeper/seed
          readOnly: true
        {{- end }}
  volumes:
    - name: tmp
      emptyDir:
        sizeLimit: 256Mi
    {{- if eq $kind "migrate" }}
    # Only the connection URL of the migrator's Secret, readable by the group (0440).
    - name: migrator-credentials
      secret:
        secretName: {{ $root.Values.database.migratorSecretName }}
        defaultMode: 288
        items:
          - key: uri
            path: uri
    {{- else }}
    # The application role's Secret, the one the hub mounts, through its uri key only.
    - name: db-credentials
      secret:
        secretName: {{ $root.Values.database.credentialsSecretName }}
        defaultMode: 288
        items:
          - key: uri
            path: uri
    {{- end }}
    - name: db-ca
      secret:
        secretName: {{ $root.Values.database.caSecretName }}
        items:
          - key: ca.crt
            path: ca.crt
    {{- if eq $kind "seed" }}
    # The seed file names service accounts and holds no secret, so a ConfigMap is right.
    - name: seed
      configMap:
        name: {{ .configMap }}
        items:
          - key: seed.yaml
            path: seed.yaml
    {{- end }}
{{- end }}
