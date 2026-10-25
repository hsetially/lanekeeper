{{/*
Labels. The pods of the cluster are labelled by the operator (cnpg.io/cluster); these are for the resources of the chart and for the
policies, which read lanekeeper.io/environment to know what is required.
*/}}
{{- define "lanekeeper-db.labels" -}}
app.kubernetes.io/name: lanekeeper-db
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: lanekeeper
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
lanekeeper.io/environment: {{ include "lanekeeper-db.environment" . }}
{{- end }}

{{/* kind, staging or prod. */}}
{{- define "lanekeeper-db.environment" -}}
{{- required "environment is required: kind, staging or prod" .Values.environment -}}
{{- end }}

{{/*
The image, by digest. A tag is never deployed (S19), so a digest that is not sha256:<64 hex digits> stops the render, whatever the
schema said. The tag is kept in front of the digest for the reader.
*/}}
{{- define "lanekeeper-db.image" -}}
{{- $digest := required "image.digest is required: a tag is never deployed (S19)" .Values.image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "image.digest %q is not sha256:<64 hex digits>: a tag is never deployed (S19)" $digest) -}}
{{- end -}}
{{- if .Values.image.tag -}}
{{- printf "%s:%s@%s" .Values.image.repository .Values.image.tag $digest -}}
{{- else -}}
{{- printf "%s@%s" .Values.image.repository $digest -}}
{{- end -}}
{{- end }}

{{/*
The name of the StorageClass of the data volumes: the CMEK class this chart renders, or the cluster's own with a marker.
*/}}
{{- define "lanekeeper-db.storageClassName" -}}
{{- if .Values.storage.cmekKey -}}lanekeeper-db-cmek{{- else -}}{{ .Values.storage.fallbackClassName }}{{- end -}}
{{- end }}

{{/* Whether this release backs up: always, except in kind, which has no Google (D40). */}}
{{- define "lanekeeper-db.backup" -}}
{{- $env := include "lanekeeper-db.environment" . -}}
{{- if .Values.backup.enabled -}}
true
{{- else if ne $env "kind" -}}
{{- fail (printf "backup.enabled is false in %s: only kind may run without backups (D40)" $env) -}}
{{- end -}}
{{- end }}
