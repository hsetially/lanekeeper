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
{{- end }}
