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
