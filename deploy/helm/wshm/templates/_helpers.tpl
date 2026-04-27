{{/*
Expand the name of the chart.
*/}}
{{- define "wshm.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this.
*/}}
{{- define "wshm.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
Chart name and version (label).
*/}}
{{- define "wshm.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Common labels.
*/}}
{{- define "wshm.labels" -}}
helm.sh/chart: {{ include "wshm.chart" . }}
{{ include "wshm.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: wshm
{{- end -}}

{{/*
Selector labels.
*/}}
{{- define "wshm.selectorLabels" -}}
app.kubernetes.io/name: {{ include "wshm.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/*
Service account name.
*/}}
{{- define "wshm.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "wshm.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/*
Image reference (repository:tag).
*/}}
{{- define "wshm.image" -}}
{{- $tag := default .Chart.AppVersion .Values.image.tag -}}
{{- printf "%s:%s" .Values.image.repository $tag -}}
{{- end -}}

{{/*
Secret name to read credentials from (existing or chart-managed).
*/}}
{{- define "wshm.secretName" -}}
{{- if .Values.secrets.existingSecret -}}
{{- .Values.secrets.existingSecret -}}
{{- else -}}
{{- include "wshm.fullname" . -}}
{{- end -}}
{{- end -}}

{{/*
PVC name (existing or chart-managed).
*/}}
{{- define "wshm.pvcName" -}}
{{- if .Values.persistence.existingClaim -}}
{{- .Values.persistence.existingClaim -}}
{{- else -}}
{{- include "wshm.fullname" . -}}
{{- end -}}
{{- end -}}

{{/*
Common pod spec snippets shared by Job and CronJob.
Renders the container env (secret refs + repo + extras), volume mounts,
and command args.
*/}}
{{- define "wshm.containerEnv" -}}
{{- $secretName := include "wshm.secretName" . -}}
- name: GITHUB_TOKEN
  valueFrom:
    secretKeyRef:
      name: {{ $secretName }}
      key: GITHUB_TOKEN
      optional: true
- name: ANTHROPIC_API_KEY
  valueFrom:
    secretKeyRef:
      name: {{ $secretName }}
      key: ANTHROPIC_API_KEY
      optional: true
- name: WSHM_LICENSE_KEY
  valueFrom:
    secretKeyRef:
      name: {{ $secretName }}
      key: WSHM_LICENSE_KEY
      optional: true
{{- if .Values.repo }}
- name: WSHM_REPO
  value: {{ .Values.repo | quote }}
{{- end }}
{{- with .Values.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{- define "wshm.volumeMounts" -}}
- name: data
  mountPath: /data
{{- if .Values.config.enabled }}
- name: config
  mountPath: /etc/wshm
  readOnly: true
{{- end }}
- name: tmp
  mountPath: /tmp
{{- end -}}

{{- define "wshm.volumes" -}}
- name: data
  {{- if .Values.persistence.enabled }}
  persistentVolumeClaim:
    claimName: {{ include "wshm.pvcName" . }}
  {{- else }}
  emptyDir: {}
  {{- end }}
{{- if .Values.config.enabled }}
- name: config
  configMap:
    name: {{ include "wshm.fullname" . }}-config
{{- end }}
- name: tmp
  emptyDir: {}
{{- end -}}

{{/*
Final command args, prepending --config when a ConfigMap is mounted.
*/}}
{{- define "wshm.args" -}}
{{- $args := list -}}
{{- if .Values.config.enabled -}}
{{- $args = append $args "--config" -}}
{{- $args = append $args "/etc/wshm/config.toml" -}}
{{- end -}}
{{- range .userArgs -}}
{{- $args = append $args . -}}
{{- end -}}
{{- toYaml $args -}}
{{- end -}}
