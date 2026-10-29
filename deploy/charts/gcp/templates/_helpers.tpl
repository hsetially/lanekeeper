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

{{/*
A customer-managed key (S10, Q29). Takes (list "values.path" $key $location) and, for a resource that is not a bucket, a fourth item that
names it ("secret"). Prints the key when it is a full Cloud KMS key name in the location of the bucket, and nothing when there is no key
(the fallback: Google-managed encryption, with a warning from the policy). It stops the render for a name that is not a key or is in
another location, which Cloud Storage and Secret Manager would refuse at apply time.
*/}}
{{- define "lanekeeper-gcp.key" -}}
{{- $name := index . 0 -}}
{{- $key := index . 1 -}}
{{- $location := index . 2 -}}
{{- $what := "bucket" -}}
{{- if gt (len .) 3 -}}{{- $what = index . 3 -}}{{- end -}}
{{- if $key -}}
{{- if not (regexMatch "^projects/[^/]+/locations/[^/]+/keyRings/[^/]+/cryptoKeys/[^/]+$" $key) -}}
{{- fail (printf "%s %q is not a Cloud KMS key (projects/P/locations/L/keyRings/R/cryptoKeys/K) (S10)" $name $key) -}}
{{- end -}}
{{- $keyLocation := index (regexSplit "/" $key -1) 3 -}}
{{- if ne (lower $keyLocation) (lower $location) -}}
{{- fail (printf "%s %q is in %s, and the %s is in %s: a key must be in the location of the %s" $name $key $keyLocation $what $location $what) -}}
{{- end -}}
{{- $key -}}
{{- end -}}
{{- end }}

{{/* The environment label of the buckets and the policy: staging or prod. */}}
{{- define "lanekeeper-gcp.environment" -}}
{{- $env := required "environment is required: staging or prod (the audit checkpoint bucket is locked in prod)" .Values.environment -}}
{{- if not (has $env (list "staging" "prod")) -}}
{{- fail (printf "environment %q must be staging or prod" $env) -}}
{{- end -}}
{{- $env -}}
{{- end }}

{{/*
One bucket, up to the end of the lines every bucket shares. Takes a dict with root (the chart's context), purpose (backup,
audit-checkpoint or docs), cfg (the bucket's values) and path (its place in the values, for messages). The caller adds the lines
that only this bucket has under `spec:`, indented by two.
*/}}
{{- define "lanekeeper-gcp.bucket" -}}
{{- $root := .root -}}
{{- $project := required "project is required: the Google Cloud project of the Config Connector resources" $root.Values.project -}}
{{- $location := required "location is required: the region or multi-region of the buckets" $root.Values.location -}}
{{- $env := include "lanekeeper-gcp.environment" $root -}}
{{- $name := required (printf "%s.name is required: a bucket name is global to Google Cloud" .path) .cfg.name -}}
{{- $key := include "lanekeeper-gcp.key" (list (printf "%s.cmekKey" .path) .cfg.cmekKey $location) -}}
apiVersion: storage.cnrm.cloud.google.com/v1beta1
kind: StorageBucket
metadata:
  name: {{ $name }}
  namespace: {{ $root.Release.Namespace }}
  labels:
    {{- include "lanekeeper-gcp.labels" $root | nindent 4 }}
    lanekeeper.io/purpose: {{ .purpose }}
    lanekeeper.io/environment: {{ $env }}
  annotations:
    cnrm.cloud.google.com/project-id: {{ $project | quote }}
    # Removing this resource, or Argo CD pruning it, must not delete the bucket and what is in it (S9, D40).
    cnrm.cloud.google.com/deletion-policy: abandon
    argocd.argoproj.io/sync-options: Prune=false
    {{- if not $key }}
    # No customer-managed key (Q29): Google-managed encryption. The policy warns; render.sh fails the warning for prod and staging.
    lanekeeper.io/cmek: fallback
    {{- end }}
spec:
  location: {{ $location }}
  storageClass: STANDARD
  # Access is by IAM only (no object ACLs), and the bucket can never be made public (S10).
  uniformBucketLevelAccess: true
  publicAccessPrevention: enforced
  {{- if $key }}
  encryption:
    kmsKeyRef:
      external: {{ $key }}
  {{- end }}
{{- end }}

{{/*
One IAMPolicyMember. Takes a dict with root, name, role, member, and ref (the resourceRef as a ready-made YAML block).
*/}}
{{- define "lanekeeper-gcp.member" -}}
apiVersion: iam.cnrm.cloud.google.com/v1beta1
kind: IAMPolicyMember
metadata:
  name: {{ .name }}
  namespace: {{ .root.Release.Namespace }}
  labels:
    {{- include "lanekeeper-gcp.labels" .root | nindent 4 }}
    lanekeeper.io/purpose: iam
  annotations:
    cnrm.cloud.google.com/project-id: {{ .root.Values.project | quote }}
spec:
  role: {{ .role }}
  member: {{ .member | quote }}
  resourceRef:
    {{- .ref | nindent 4 }}
{{- end }}

{{/* The Google service account that a part of Lanekeeper runs as: hub or db. */}}
{{- define "lanekeeper-gcp.gsa" -}}
{{- printf "serviceAccount:lanekeeper-%s@%s.iam.gserviceaccount.com" .part .root.Values.project -}}
{{- end }}

{{/*
The metadata of the three admission resources (binauthz.yaml): the note, the attestor and the policy. Takes a dict with root, project, env
and name. None of them is deleted from the project when it is removed from Git, and Argo CD does not prune them: without the policy
the project admits every image (S20).
*/}}
{{- define "lanekeeper-gcp.admissionMeta" -}}
name: {{ .name }}
namespace: {{ .root.Release.Namespace }}
labels:
  {{- include "lanekeeper-gcp.labels" .root | nindent 2 }}
  lanekeeper.io/purpose: admission
  lanekeeper.io/environment: {{ .env }}
annotations:
  cnrm.cloud.google.com/project-id: {{ .project | quote }}
  cnrm.cloud.google.com/deletion-policy: abandon
  argocd.argoproj.io/sync-options: Prune=false
{{- end }}

{{/*
The metadata of a key ring, a key or a secret container (09/fix, I1): the things the hub cannot run without, and that hold what a person
put in them. Takes a dict with root, name, purpose, project, env and, for a secret without a customer-managed key, fallback (true).
None of them is deleted from Google Cloud when its Kubernetes resource is removed, and Argo CD does not prune them (S5, S7, S8, S9).
*/}}
{{- define "lanekeeper-gcp.keptMeta" -}}
name: {{ .name }}
namespace: {{ .root.Release.Namespace }}
labels:
  {{- include "lanekeeper-gcp.labels" .root | nindent 2 }}
  lanekeeper.io/purpose: {{ .purpose }}
  lanekeeper.io/environment: {{ .env }}
annotations:
  cnrm.cloud.google.com/project-id: {{ .project | quote }}
  cnrm.cloud.google.com/deletion-policy: abandon
  argocd.argoproj.io/sync-options: Prune=false
  {{- if .fallback }}
  # No customer-managed key (Q29): Google-managed encryption. The policy warns; render.sh fails the warning for prod and staging.
  lanekeeper.io/cmek: fallback
  {{- end }}
{{- end }}

{{/*
The names of the hub's four keys, as JSON ({"agentCa": ..., "replicaCa": ..., "auditCheckpoint": ..., "vault": ...}): all required, and four
different keys. The two CAs are two keys (an agent's certificate must never be a replica's, S5), and no key is used for two things.
*/}}
{{- define "lanekeeper-gcp.keyNames" -}}
{{- $keys := .Values.kms.keys -}}
{{- $names := dict -}}
{{- range $field := list "agentCa" "replicaCa" "auditCheckpoint" "vault" -}}
{{- $_ := set $names $field (required (printf "kms.keys.%s is required: the name of the key (the last part of the hub chart's config.kms.%s)" $field $field) (index $keys $field)) -}}
{{- end -}}
{{- if ne (len (values $names | uniq)) 4 -}}
{{- fail (printf "kms.keys has %v: the agent CA, the replica CA, the audit checkpoint key and the vault key are four different keys, and each is used for one thing (S5, S8, S9)" (values $names | sortAlpha)) -}}
{{- end -}}
{{- toJson $names -}}
{{- end }}

{{/*
The names of the Secret Manager secrets the hub reads, as a JSON list: the six that the hub chart's config.secretManager.* name, and the
four that are constants of the hub's code. Each once: the two CA certificates must be two secrets (the hub exits 2 otherwise), and a name
twice would be one container with two meanings.
*/}}
{{- define "lanekeeper-gcp.secretNames" -}}
{{- $s := .Values.secrets.hub -}}
{{- /* crates/hub-writes/src/config.rs PREVIEW_KEY_SECRET, crates/hub/src/config/git.rs (the defaults of LK_GIT_READ_REF and LK_GIT_WEBHOOK_REF), crates/hub-mcp/src/auth/pat.rs KEY_SECRET_NAME */ -}}
{{- $names := list $s.webCert $s.webKey $s.agentCaCert $s.replicaCaCert $s.entraClient $s.loginCookie "write-preview-key" "github-read-token" "github-webhook-secret" "mcp-token-hmac-key" -}}
{{- if ne (len $names) (len (uniq $names)) -}}
{{- fail (printf "secrets.hub has a name twice, or one of the hub's constants (%v): each secret is one container with one meaning" (sortAlpha $names)) -}}
{{- end -}}
{{- toJson $names -}}
{{- end }}
