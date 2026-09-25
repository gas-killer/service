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
Whether the bundled Anvil runs as a simulation fork beside an external chain RPC.

Distinct from LOCAL mode, where the same workload is the chain itself rather than a fork of one.
Rendered as a string so callers can test it: `include "gas-killer.simFork.enabled" . | eq "true"`.
*/}}
{{- define "gas-killer.simFork.enabled" -}}
{{- if and (.Values.l1.simFork).enabled (ne .Values.global.environment "LOCAL") (not (and .Values.l1.enabled .Values.secrets.forkUrl)) -}}
{{- fail "l1.simFork.enabled needs l1.enabled=true and secrets.forkUrl set. Without them no fork renders and the fleet simulates against HTTP_RPC, whose trace cap clamps silently." -}}
{{- end -}}
{{- if and .Values.l1.enabled .Values.secrets.forkUrl (.Values.l1.simFork).enabled (ne .Values.global.environment "LOCAL") -}}
{{- if (.Values.simRpc).url -}}
{{- fail "simRpc.url and l1.simFork.enabled both set: the fleet simulates against exactly one endpoint. Unset one." -}}
{{- end -}}
true
{{- else -}}
false
{{- end -}}
{{- end }}

{{/*
The URL the router and every node simulate against (SIM_HTTP_RPC), or empty to leave extraction
on HTTP_RPC. One helper for both deployments so they cannot disagree — a divergence would change
storage_updates on one side and fork the quorum's digests, exactly like simProfile.
*/}}
{{- define "gas-killer.simRpcUrl" -}}
{{- if (.Values.simRpc).url -}}
{{- .Values.simRpc.url -}}
{{- else if include "gas-killer.simFork.enabled" . | eq "true" -}}
{{- printf "http://%s:%v" (include "gas-killer.l1.fullname" .) .Values.l1.service.port -}}
{{- end -}}
{{- end }}

{{/*
Whether the sim fork runs behind the re-fork proxy sidecar. Only meaningful for a sim fork: in
LOCAL mode Anvil is the chain, its blocks are its own, and nothing needs re-forking.
*/}}
{{- define "gas-killer.simFork.refork" -}}
{{- if and (include "gas-killer.simFork.enabled" . | eq "true") ((.Values.l1.simFork).refork).enabled -}}
true
{{- else -}}
false
{{- end -}}
{{- end }}

{{/*
Simulation profile (GK_SIM_PROFILE) shared by the router and every node. Both deployments render
this one helper, so they cannot be given different values — a divergence would change the derived
storage_updates on one side and fork the quorum's digests.

Rejects unbounded unless the deployment's own Anvil is explicitly started with its block gas limit
disabled, via l1.extraArgs carrying --disable-block-gas-limit. The l1 container overrides the
image's entrypoint, so extraArgs is the only way the flag reaches Anvil.

Measured on anvil 1.5.1, the flag does NOT gate debug_traceCall: a 104M-gas call traces fine on a
default 60M-limit anvil, with or without blockOverrides, because the flag governs block
construction rather than tracing. The gate is kept anyway because the cap that actually bites is a
node-level one — geth's --rpc.gascap, which hosted providers set and which does clamp traces,
silently returning a truncated result rather than an error. Requiring the flag keeps the
simulation endpoint's cap an explicit deployment decision rather than a property of whichever
client happens to be behind it.

Says nothing about TESTNET without a sim fork: there the cap belongs to an endpoint the chart
cannot see, which is what l1.simFork.enabled exists to bring in-cluster.
*/}}
{{- define "gas-killer.simProfile" -}}
{{- $profile := .Values.global.simProfile | default "chain" -}}
{{- if not (has $profile (list "chain" "unbounded")) -}}
{{- fail (printf "global.simProfile must be \"chain\" or \"unbounded\", got %q" $profile) -}}
{{- end -}}
{{- $bundledAnvil := or (eq .Values.global.environment "LOCAL") (include "gas-killer.simFork.enabled" . | eq "true") -}}
{{- $capLifted := contains "--disable-block-gas-limit" (.Values.l1.extraArgs | default "") -}}
{{- if and (eq $profile "unbounded") $bundledAnvil (not $capLifted) -}}
{{- fail "global.simProfile=unbounded needs the bundled Anvil to run with --disable-block-gas-limit, or an above-block-limit call OOGs inside the tracer and analysis returns a truncated payload instead of an error. Set l1.extraArgs=\"--disable-block-gas-limit\"." -}}
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
Deploy-target job name
*/}}
{{- define "gas-killer.deployTarget.fullname" -}}
{{- printf "%s-deploy-target" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Generate router key job name
*/}}
{{- define "gas-killer.generate-router-key.fullname" -}}
{{- printf "%s-generate-router-key" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Schnorr operator-set job name
*/}}
{{- define "gas-killer.schnorr-operators.fullname" -}}
{{- printf "%s-schnorr-operators" (include "gas-killer.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Path the operator-set job records its registry address at, on the shared volume. The router reads
it from there as a named record, so a job that finishes after the router is serving still lands.
*/}}
{{- define "gas-killer.schnorrRegistryRecord" -}}
/app/.nodes/schnorr_stake_registry.txt
{{- end }}

{{/*
The quorum signature scheme (SIGNATURE_SCHEME) the router and every node run with. Any value other
than schnorr fails here rather than installing a fleet that cannot sign.
*/}}
{{- define "gas-killer.signatureScheme" -}}
{{- $scheme := .Values.global.signatureScheme | default "schnorr" | trim | lower -}}
{{- if not (has $scheme (list "" "schnorr")) -}}
{{- fail (printf "global.signatureScheme must be \"schnorr\", got %q" .Values.global.signatureScheme) -}}
{{- end -}}
schnorr
{{- end }}
