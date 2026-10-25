{{- define "lanekeeper-gcp.labels" -}}
app.kubernetes.io/name: lanekeeper-gcp
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: lanekeeper
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | quote }}
{{- end }}

{{/* A list of ranges that is not empty and is not the internet (S18). Takes (list "values.path" $ranges). */}}
{{- define "lanekeeper-gcp.requireRanges" -}}
{{- $name := index . 0 -}}
{{- $ranges := index . 1 -}}
{{- if not $ranges -}}
{{- fail (printf "%s is empty (S18)" $name) -}}
{{- end -}}
{{- range $ranges -}}
{{- if or (eq . "0.0.0.0/0") (eq . "::/0") (eq . "*") -}}
{{- fail (printf "%s has %s, which is the whole internet (S18)" $name .) -}}
{{- end -}}
{{- end -}}
{{- end }}

{{/* The expression of one hooks rule: the exact path, and up to four of GitHub's ranges. Takes the list of ranges. */}}
{{- define "lanekeeper-gcp.hooksExpression" -}}
{{- $parts := list -}}
{{- range . -}}
{{- $parts = append $parts (printf "inIpRange(origin.ip, '%s')" .) -}}
{{- end -}}
{{- printf "request.path == '/hooks/github' && (%s)" (join " || " $parts) -}}
{{- end }}
