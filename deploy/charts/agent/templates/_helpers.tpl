{{/*
Labels. `app.kubernetes.io/name` alone selects the agent's pod in its NetworkPolicy (crates/agent/HARDENING.md); the instance label
tells two releases apart.
*/}}
{{- define "lanekeeper-agent.selectorLabels" -}}
app.kubernetes.io/name: lanekeeper-agent
{{- end }}

{{- define "lanekeeper-agent.podLabels" -}}
app.kubernetes.io/name: lanekeeper-agent
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: lanekeeper
{{- end }}

{{- define "lanekeeper-agent.labels" -}}
{{ include "lanekeeper-agent.podLabels" . }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
{{- end }}

{{/*
The image, by digest. A tag is never deployed (S19), so a digest that is not sha256:<64 hex digits> stops the render, whatever
the schema said.
*/}}
{{- define "lanekeeper-agent.image" -}}
{{- $digest := required "image.digest is required: a tag is never deployed (S19)" .Values.image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "image.digest %q is not sha256:<64 hex digits>: a tag is never deployed (S19)" $digest) -}}
{{- end -}}
{{- printf "%s@%s" .Values.image.repository $digest -}}
{{- end }}

{{/*
The hub's CA, trimmed. It is public, so it may be in a values file and in a ConfigMap; a private key may not be in either, and
a file that is not a certificate is no CA. Stops the render otherwise.
*/}}
{{- define "lanekeeper-agent.hubCa" -}}
{{- $pem := required "hubCa.pem is required: the agent pins the hub's CA (S6), and without it cannot trust the hub" .Values.hubCa.pem | trim -}}
{{- if contains "PRIVATE KEY" $pem -}}
{{- fail "hubCa.pem holds a private key; it must hold the hub's CA certificate and nothing secret (S7)" -}}
{{- end -}}
{{- if not (hasPrefix "-----BEGIN CERTIFICATE-----" $pem) -}}
{{- fail "hubCa.pem is not a PEM certificate (it must start with -----BEGIN CERTIFICATE-----)" -}}
{{- end -}}
{{- $pem -}}
{{- end }}

{{/*
The namespace the config-server runs in (crates/agent/HARDENING.md, "The config-server's namespace"): one explicit name, required. The schema
already refuses a name that is not one and the three namespaces of the cluster's own components; this stops the render again for those, and for
the agent's own namespace, which the config-server does not run in. The egress rule and LK_CONFIG_SERVER_URL both take it from here, so the two
cannot differ.
*/}}
{{- define "lanekeeper-agent.configServerNamespace" -}}
{{- $ns := required "configServer.namespace is required: the namespace the swimlane's config-server runs in (crates/agent/HARDENING.md)" .Values.configServer.namespace -}}
{{- if not (regexMatch "^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$" $ns) -}}
{{- fail (printf "configServer.namespace %q is not a namespace name" $ns) -}}
{{- end -}}
{{- if has $ns (list "kube-system" "kube-public" "kube-node-lease") -}}
{{- fail (printf "configServer.namespace is %s, a namespace of the cluster's own components: the config-server runs in its swimlane's namespace, and a rule there would reach whatever is labelled like it (S17)" $ns) -}}
{{- end -}}
{{- if eq $ns .Release.Namespace -}}
{{- fail (printf "configServer.namespace is %s, the agent's own namespace: the config-server does not run in the agent's namespace (crates/agent/HARDENING.md)" $ns) -}}
{{- end -}}
{{- $ns -}}
{{- end }}

{{/*
LK_CONFIG_SERVER_URL: the service by its full name, because a short name resolves only in the agent's own namespace.
*/}}
{{- define "lanekeeper-agent.configServerUrl" -}}
{{- printf "http://%s.%s.svc.cluster.local:8888" (.Values.configServer.service | default "csp-configuration-server") (include "lanekeeper-agent.configServerNamespace" .) -}}
{{- end }}

{{/*
How the agent joins (S5, Q26): Workload Identity, the join token, or Workload Identity first and the token when it is not
available. One of the two is required; stops the render otherwise.
*/}}
{{- define "lanekeeper-agent.joinMode" -}}
{{- if and .Values.serviceAccount.gcpServiceAccount .Values.joinToken.enabled -}}auto
{{- else if .Values.serviceAccount.gcpServiceAccount -}}workload-identity
{{- else if .Values.joinToken.enabled -}}token
{{- else -}}
{{- fail "the agent has no way to join the hub: set serviceAccount.gcpServiceAccount (Workload Identity) or joinToken.enabled (S5, Q26)" -}}
{{- end -}}
{{- end }}
