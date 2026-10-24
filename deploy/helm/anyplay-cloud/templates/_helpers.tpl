{{- define "anyplay-cloud.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "anyplay-cloud.fullname" -}}
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

{{- define "anyplay-cloud.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "anyplay-cloud.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "anyplay-cloud.selectorLabels" -}}
app.kubernetes.io/name: {{ include "anyplay-cloud.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "anyplay-cloud.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "anyplay-cloud.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "anyplay-cloud.image" -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) }}
{{- end }}

{{/* Seconds between SIGTERM and SIGKILL: the drain, then up to 10 seconds to close, then slack. */}}
{{- define "anyplay-cloud.gracePeriod" -}}
{{- if kindIs "invalid" .Values.terminationGracePeriodSeconds }}
{{- add (int .Values.config.drainSeconds) 25 }}
{{- else }}
{{- .Values.terminationGracePeriodSeconds }}
{{- end }}
{{- end }}

{{/* The SMTP port the network policy opens. */}}
{{- define "anyplay-cloud.smtpPort" -}}
{{- if .Values.config.smtpPort }}
{{- .Values.config.smtpPort }}
{{- else if eq .Values.config.smtpTls "tls" }}
{{- 465 }}
{{- else if eq .Values.config.smtpTls "none" }}
{{- 25 }}
{{- else }}
{{- 587 }}
{{- end }}
{{- end }}

{{/* Stops the install with a clear message when something required is missing or wrong. */}}
{{- define "anyplay-cloud.validate" -}}
{{- if not .Values.secrets.existingSecret }}{{ fail "secrets.existingSecret: name the Secret that holds the database URL (docs/kubernetes.md)" }}{{ end }}
{{- if not .Values.signingKey.existingSecret }}{{ fail "signingKey.existingSecret: name the Secret that holds the signing key (docs/kubernetes.md)" }}{{ end }}
{{- if not .Values.natsUrl }}{{ fail "natsUrl: a cluster needs NATS, such as nats://nats.nats.svc:4222" }}{{ end }}
{{- if and (eq .Values.config.mail "smtp") (not .Values.config.smtpHost) }}{{ fail "config.smtpHost: required with config.mail = smtp (config.mail = log is for tests only)" }}{{ end }}
{{- if lt (int (include "anyplay-cloud.gracePeriod" .)) (add (int .Values.config.drainSeconds) 11) }}{{ fail "terminationGracePeriodSeconds must be longer than config.drainSeconds plus the 10 seconds a node needs to close its connections" }}{{ end }}
{{- end }}
