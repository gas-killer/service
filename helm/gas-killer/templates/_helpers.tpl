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
Simulation profile (GK_SIM_PROFILE) shared by the router and every node. Both deployments render
this one helper, so they cannot be given different values — a divergence would change the derived
storage_updates on one side and fork the quorum's digests.

Rejects the LOCAL + unbounded-v1 combination unless it is explicitly acknowledged. The profile
lifts the gas limits a tracked function is SIMULATED under, but executing an above-block-limit
call also needs the node serving debug_traceCall to have its own execution cap lifted. In LOCAL
mode that node is the bundled Anvil, whose flags come from the ethereum image's entrypoint rather
than this chart, so the chart cannot make the deployment work on its own — without the flag every
heavy task fails analysis at runtime instead of at install time.
*/}}
{{- define "gas-killer.simProfile" -}}
{{- $profile := .Values.global.simProfile | default "chain" -}}
{{- if not (has $profile (list "chain" "unbounded-v1")) -}}
{{- fail (printf "global.simProfile must be \"chain\" or \"unbounded-v1\", got %q" $profile) -}}
{{- end -}}
{{- if and (eq $profile "unbounded-v1") (eq .Values.global.environment "LOCAL") (not .Values.global.localAnvilUnboundedReady) -}}
{{- fail "global.simProfile=unbounded-v1 in LOCAL mode requires the bundled Anvil to run with --disable-block-gas-limit, which comes from the ethereum image's entrypoint and not this chart. Set global.localAnvilUnboundedReady=true to confirm the image provides it." -}}
{{- end -}}
{{- $profile -}}
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
Router persistent data PVC name
*/}}
{{- define "gas-killer.routerdata.fullname" -}}
{{- printf "%s-router-data" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
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
Kubernetes ServiceAccount name (for Workload Identity with GCP Secret Manager)
*/}}
{{- define "gas-killer.serviceaccount.fullname" -}}
{{- printf "%s-sa" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Key export job name
*/}}
{{- define "gas-killer.keyexport.fullname" -}}
{{- printf "%s-key-export" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Bridge job name
*/}}
{{- define "gas-killer.bridge.fullname" -}}
{{- printf "%s-bridge" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Deploy-target job name
*/}}
{{- define "gas-killer.deployTarget.fullname" -}}
{{- printf "%s-deploy-target" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
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

{{/*
Generate router key job name
*/}}
{{- define "gas-killer.generate-router-key.fullname" -}}
{{- printf "%s-generate-router-key" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
