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
Simulation profile (GK_SIM_PROFILE) shared by the router and every node.
Validated here so a typo fails at `helm install` rather than crash-looping every
pod on the binary's own panic. Both deployments read this single global value, so
they cannot disagree — a divergence would fork the quorum's signed payloads.
*/}}
{{- define "gas-killer.simProfile" -}}
{{- $profile := .Values.global.simProfile | default "chain" -}}
{{- if not (has $profile (list "chain" "unbounded-v1" "unbounded-v1-xl")) -}}
{{- fail (printf "global.simProfile must be \"chain\", \"unbounded-v1\" or \"unbounded-v1-xl\", got %q" $profile) -}}
{{- end -}}
{{- $profile -}}
{{- end }}

{{/*
Simulation executor (GK_SIM_EXECUTOR) shared by the router and every node
(gas-analyzer#169). Validated here so a typo fails at `helm install` rather
than crash-looping every pod on the binary's own panic
(SimExecutor::parse). Both deployments read this single global value, so
they cannot disagree — a divergent executor choice is not itself
consensus-breaking (local and RPC execution byte-agree by construction,
see gas-analyzer#169's differential tests) but defeats the point of
choosing "local" (e.g. no in-process RPC access to a 35GB overlay) if only
some operators honor it.
*/}}
{{- define "gas-killer.simExecutor" -}}
{{- $executor := .Values.global.simExecutor | default "rpc" -}}
{{- if not (has $executor (list "rpc" "local")) -}}
{{- fail (printf "global.simExecutor must be \"rpc\" or \"local\", got %q" $executor) -}}
{{- end -}}
{{- $executor -}}
{{- end }}

{{/*
UNBOUNDED_V3 guest VM (gkvm precompile): "true" when any guest program or
artifact is configured. Router and every node render the same initContainer,
env slots and mount from the same global lists, so they cannot disagree on
the installed set.
*/}}
{{- define "gas-killer.guestVm.enabled" -}}
{{- if or .Values.global.guestPrograms .Values.global.guestArtifacts -}}true{{- end -}}
{{- end }}

{{/*
Validates global.guestPrograms / guestArtifacts / guestVmConsumers so a typo
fails at `helm install` rather than crash-looping every pod on the binary's
own startup panic (gkvm_host_from_env / guest_vm_consumers_from_env). Guest
programs under simExecutor "rpc" are refused here for the same reason the
binary refuses them: no guest VM exists behind debug_traceCall, so every
guest-VM task would become a signed GkVmUnavailable revert transition.
*/}}
{{- define "gas-killer.guestVm.validate" -}}
{{- $digest := "^0x[0-9a-fA-F]{64}$" -}}
{{- $name := "^[A-Za-z0-9][A-Za-z0-9._-]*$" -}}
{{- if and (include "gas-killer.guestVm.enabled" .) (ne (include "gas-killer.simExecutor" .) "local") -}}
{{- fail "global.guestPrograms / global.guestArtifacts require global.simExecutor \"local\" (guest programs do not run under the rpc executor)" -}}
{{- end -}}
{{- $seen := dict -}}
{{- range $i, $p := (.Values.global.guestPrograms | default list) -}}
{{- if not (regexMatch $name ($p.name | default "")) -}}
{{- fail (printf "global.guestPrograms[%d].name must match %s, got %q" $i $name ($p.name | default "")) -}}
{{- end -}}
{{- if hasKey $seen $p.name -}}
{{- fail (printf "global.guestPrograms[%d].name %q is used twice" $i $p.name) -}}
{{- end -}}
{{- $_ := set $seen $p.name true -}}
{{- if not $p.url -}}
{{- fail (printf "global.guestPrograms[%d] (%s): url must be set" $i $p.name) -}}
{{- end -}}
{{- if not (regexMatch $digest ($p.programHash | default "")) -}}
{{- fail (printf "global.guestPrograms[%d] (%s): programHash must be 0x + 64 hex digits, got %q" $i $p.name ($p.programHash | default "")) -}}
{{- end -}}
{{- end -}}
{{- $seen = dict -}}
{{- range $i, $a := (.Values.global.guestArtifacts | default list) -}}
{{- if not (regexMatch $name ($a.name | default "")) -}}
{{- fail (printf "global.guestArtifacts[%d].name must match %s, got %q" $i $name ($a.name | default "")) -}}
{{- end -}}
{{- if hasKey $seen $a.name -}}
{{- fail (printf "global.guestArtifacts[%d].name %q is used twice" $i $a.name) -}}
{{- end -}}
{{- $_ := set $seen $a.name true -}}
{{- if not $a.baseUrl -}}
{{- fail (printf "global.guestArtifacts[%d] (%s): baseUrl must be set" $i $a.name) -}}
{{- end -}}
{{- if not $a.files -}}
{{- fail (printf "global.guestArtifacts[%d] (%s): files must list the bundle's files in manifest order" $i $a.name) -}}
{{- end -}}
{{- range $f := $a.files -}}
{{- if not (regexMatch $name $f) -}}
{{- fail (printf "global.guestArtifacts[%d] (%s): file name must match %s, got %q" $i $a.name $name $f) -}}
{{- end -}}
{{- end -}}
{{- if not (regexMatch $digest ($a.artifactRoot | default "")) -}}
{{- fail (printf "global.guestArtifacts[%d] (%s): artifactRoot must be 0x + 64 hex digits, got %q" $i $a.name ($a.artifactRoot | default "")) -}}
{{- end -}}
{{- end -}}
{{- range $i, $c := (.Values.global.guestVmConsumers | default list) -}}
{{- if not (regexMatch "^0x[0-9a-fA-F]{40}$" ($c.consumer | default "")) -}}
{{- fail (printf "global.guestVmConsumers[%d].consumer must be 0x + 40 hex digits, got %q" $i ($c.consumer | default "")) -}}
{{- end -}}
{{- if not (regexMatch $digest ($c.programHash | default "")) -}}
{{- fail (printf "global.guestVmConsumers[%d].programHash must be 0x + 64 hex digits, got %q" $i ($c.programHash | default "")) -}}
{{- end -}}
{{- end -}}
{{- end }}

{{/*
initContainer downloading the guest programs and artifact bundles into the
guest-vm emptyDir: programs to programs/<name>.elf, each bundle's files to
artifacts/<name>/<file>. Integrity is enforced by the service itself: at
startup it keccaks every program against GK_GUEST_PROGRAM_HASH[_N] and
rebuilds every bundle's Merkle v3 root against GK_GUEST_ARTIFACT_ROOT[_N],
and refuses to boot on a mismatch — a corrupted download cannot be served.
*/}}
{{- define "gas-killer.guestVm.initContainer" -}}
- name: fetch-guest-vm
  image: {{ .Values.global.guestDownloaderImage | quote }}
  command:
    - sh
    - -c
    - |
      set -e
      mkdir -p /guest/programs /guest/artifacts
      {{- range $p := (.Values.global.guestPrograms | default list) }}
      echo "Downloading guest program {{ $p.name }} ({{ $p.url }})..."
      curl -fSL --retry 5 --retry-delay 10 -o "/guest/programs/{{ $p.name }}.elf" "{{ $p.url }}"
      {{- end }}
      {{- range $a := (.Values.global.guestArtifacts | default list) }}
      mkdir -p "/guest/artifacts/{{ $a.name }}"
      {{- range $f := $a.files }}
      echo "Downloading guest artifact {{ $a.name }}/{{ $f }}..."
      curl -fSL --retry 5 --retry-delay 10 -o "/guest/artifacts/{{ $a.name }}/{{ $f }}" "{{ $a.baseUrl }}/{{ $f }}"
      {{- end }}
      {{- end }}
      echo "Guest programs and artifacts downloaded."
  volumeMounts:
    - name: guest-vm
      mountPath: /guest
{{- end }}

{{/*
GK_GUEST_PROGRAM[_N] / GK_GUEST_ARTIFACT[_N] env slots, in list order: the
first entry takes the bare names, the rest _1, _2, ... without gaps
(GuestProgramSet::from_env stops at the first missing slot). An artifact
slot's path is the bundle's files joined with ":" in manifest order.
*/}}
{{- define "gas-killer.guestVm.env" -}}
{{- $mount := .Values.global.guestMountPath -}}
{{- range $i, $p := (.Values.global.guestPrograms | default list) }}
{{- $slot := ternary "" (printf "_%d" $i) (eq $i 0) }}
- name: GK_GUEST_PROGRAM{{ $slot }}
  value: "{{ $mount }}/programs/{{ $p.name }}.elf"
- name: GK_GUEST_PROGRAM_HASH{{ $slot }}
  value: {{ $p.programHash | quote }}
{{- end }}
{{- range $i, $a := (.Values.global.guestArtifacts | default list) }}
{{- $slot := ternary "" (printf "_%d" $i) (eq $i 0) }}
{{- $paths := list }}
{{- range $f := $a.files }}
{{- $paths = append $paths (printf "%s/artifacts/%s/%s" $mount $a.name $f) }}
{{- end }}
- name: GK_GUEST_ARTIFACT{{ $slot }}
  value: {{ join ":" $paths | quote }}
- name: GK_GUEST_ARTIFACT_ROOT{{ $slot }}
  value: {{ $a.artifactRoot | quote }}
{{- end }}
{{- end }}

{{/*
GK_GUEST_VM_CONSUMERS: the requiresGuestVm consumer registry, rendered
whatever the executor — it is what makes an operator WITHOUT the guest VM
abstain instead of signing the GkVmUnavailable revert transition.
*/}}
{{- define "gas-killer.guestVm.consumers" -}}
{{- $entries := list -}}
{{- range $c := (.Values.global.guestVmConsumers | default list) -}}
{{- $entries = append $entries (printf "%s=%s" $c.consumer $c.programHash) -}}
{{- end -}}
{{- join "," $entries -}}
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
