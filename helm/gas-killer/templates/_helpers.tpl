{{/*
Expand the name of the chart.
*/}}
{{- define "gas-killer.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "gas-killer.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "gas-killer.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "gas-killer.labels" -}}
helm.sh/chart: {{ include "gas-killer.chart" . }}
{{ include "gas-killer.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "gas-killer.selectorLabels" -}}
app.kubernetes.io/name: {{ include "gas-killer.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
L1 service name
*/}}
{{- define "gas-killer.l1.fullname" -}}
{{- printf "%s-l1" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Signer service name
*/}}
{{- define "gas-killer.signer.fullname" -}}
{{- printf "%s-signer" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Router service name
*/}}
{{- define "gas-killer.router.fullname" -}}
{{- printf "%s-router" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Node name helper
*/}}
{{- define "gas-killer.node.fullname" -}}
{{- printf "%s-node" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Setup job name
*/}}
{{- define "gas-killer.setup.fullname" -}}
{{- printf "%s-setup" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Shared data PVC name
*/}}
{{- define "gas-killer.shareddata.fullname" -}}
{{- printf "%s-shared-data" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Config ConfigMap name
*/}}
{{- define "gas-killer.config.fullname" -}}
{{- printf "%s-config" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Secret name - supports existing secret or creates new one
*/}}
{{- define "gas-killer.secret.fullname" -}}
{{- if .Values.secrets.existingSecret }}
{{- .Values.secrets.existingSecret }}
{{- else }}
{{- printf "%s-secret" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{/*
Bridge job name
*/}}
{{- define "gas-killer.bridge.fullname" -}}
{{- printf "%s-bridge" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
L2 service name
*/}}
{{- define "gas-killer.l2.fullname" -}}
{{- printf "%s-l2" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Yield distribution job name
*/}}
{{- define "gas-killer.yield-distribution.fullname" -}}
{{- printf "%s-yield-distribution" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
