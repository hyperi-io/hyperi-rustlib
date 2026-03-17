// Project:   hyperi-rustlib
// File:      src/deployment/generate.rs
// Purpose:   Generate deployment artifacts from DeploymentContract
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Generate deployment artifacts (Dockerfile, Helm chart, Compose fragment)
//! from a [`DeploymentContract`].
//!
//! Apps provide ~20% customisation (ports, secrets, config); this module
//! generates ~80% boilerplate (Dockerfile, Helm chart, Compose fragment).

#![allow(clippy::format_push_string)] // Template generators naturally build strings via push_str(&format!(...))

use std::path::Path;

use super::contract::{DeploymentContract, ImageProfile};
use super::error::DeploymentError;

// ============================================================================
// Dockerfile
// ============================================================================

/// Generate a Dockerfile from the deployment contract.
///
/// When `native_deps` is populated (via [`NativeDepsContract::for_rustlib_features`]),
/// the generated Dockerfile automatically includes custom APT repo setup and
/// runtime package installation. If `native_deps` is empty, only base utilities
/// are installed.
#[must_use]
pub fn generate_dockerfile(contract: &DeploymentContract) -> String {
    let binary = contract.binary();

    // EXPOSE line: metrics_port + extra ports
    let expose_ports = {
        let mut ports = vec![contract.metrics_port.to_string()];
        for p in &contract.extra_ports {
            ports.push(p.port.to_string());
        }
        ports.join(" ")
    };

    // CMD line
    let cmd = if contract.entrypoint_args.is_empty() {
        String::new()
    } else {
        let args: Vec<String> = contract
            .entrypoint_args
            .iter()
            .map(|a| format!("\"{a}\""))
            .collect();
        format!("\nCMD [{}]", args.join(", "))
    };

    // Build the apt-get RUN block dynamically from native_deps + image profile
    let apt_block = build_apt_block(&contract.native_deps, contract.image_profile);

    let profile_label = match contract.image_profile {
        ImageProfile::Production => "production",
        ImageProfile::Development => "development",
    };

    format!(
        r#"# Project:   {app_name}
# File:      Dockerfile
# Purpose:   {profile_label} container image
#
# License:   FSL-1.1-ALv2
# Copyright: (c) 2026 HYPERI PTY LIMITED

FROM {base_image}

LABEL io.hyperi.profile="{profile_label}"

{apt_block}
COPY {binary} /usr/local/bin/{binary}
RUN chmod +x /usr/local/bin/{binary}

# Ubuntu 24.04 ships with ubuntu user at UID 1000 — remove before creating appuser
RUN userdel -r ubuntu && useradd --create-home --uid 1000 appuser
USER appuser

EXPOSE {expose_ports}

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -sf http://localhost:{metrics_port}{liveness_path} > /dev/null || exit 1

ENTRYPOINT ["{binary}"]{cmd}
"#,
        app_name = contract.app_name,
        base_image = contract.base_image,
        binary = binary,
        profile_label = profile_label,
        apt_block = apt_block,
        expose_ports = expose_ports,
        metrics_port = contract.metrics_port,
        liveness_path = contract.health.liveness_path,
        cmd = cmd,
    )
}

/// Diagnostic tools installed in development images.
const DEV_TOOLS: &[&str] = &[
    "bash",
    "strace",
    "tcpdump",
    "procps",
    "dnsutils",
    "net-tools",
    "less",
    "jq",
];

/// Build the apt-get RUN block from native deps contract and image profile.
///
/// When custom APT repos are needed (e.g., Confluent for librdkafka), emits
/// the GPG key download, sources list entry, and repo-specific packages.
/// Development profile adds diagnostic tools (strace, tcpdump, etc.).
fn build_apt_block(deps: &super::native_deps::NativeDepsContract, profile: ImageProfile) -> String {
    let mut out = String::with_capacity(512);
    let is_dev = profile == ImageProfile::Development;

    // Base packages always installed (curl needed for healthcheck, ca-certificates for TLS)
    let mut base_pkgs = vec!["ca-certificates", "curl", "netcat-openbsd", "iputils-ping"];

    // If we have custom APT repos, we need gnupg for key import
    if !deps.apt_repos.is_empty() {
        base_pkgs.push("gnupg");
    }

    // Dev profile adds diagnostic tools
    if is_dev {
        base_pkgs.extend_from_slice(DEV_TOOLS);
    }

    if deps.is_empty() {
        // No native deps — simple install
        out.push_str("RUN apt-get update && apt-get install -y --no-install-recommends \\\n");
        out.push_str(&format!("    {} \\\n", base_pkgs.join(" ")));
        out.push_str("    && rm -rf /var/lib/apt/lists/*\n");
        return out;
    }

    // Collect all runtime packages (repo-specific + default)
    let mut runtime_pkgs: Vec<&str> = Vec::new();
    for repo in &deps.apt_repos {
        for pkg in &repo.packages {
            runtime_pkgs.push(pkg);
        }
    }
    for pkg in &deps.apt_packages {
        runtime_pkgs.push(pkg);
    }

    // Build multi-step RUN: base install → repo setup → update → runtime install → cleanup
    out.push_str("# Runtime shared libraries for dynamically-linked Rust crates.\n");

    out.push_str("RUN apt-get update && apt-get install -y --no-install-recommends \\\n");
    out.push_str(&format!("    {} \\\n", base_pkgs.join(" ")));

    // Add each custom APT repo
    for repo in &deps.apt_repos {
        out.push_str(&format!(
            "    && curl -fsSL {} \\\n\
             \x20      | gpg --dearmor -o {} \\\n\
             \x20   && echo \"deb [signed-by={}] \\\n\
             \x20      {} {} main\" \\\n\
             \x20      > /etc/apt/sources.list.d/{}.list \\\n",
            repo.key_url,
            repo.keyring,
            repo.keyring,
            repo.url,
            repo.codename,
            // Derive a stable filename from the keyring path
            std::path::Path::new(&repo.keyring)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("custom-repo"),
        ));
    }

    // Second apt-get update + install runtime packages
    out.push_str("    && apt-get update && apt-get install -y --no-install-recommends \\\n");
    out.push_str(&format!("       {} \\\n", runtime_pkgs.join(" ")));
    out.push_str("    && rm -rf /var/lib/apt/lists/*\n");

    out
}

// ============================================================================
// Docker Compose fragment
// ============================================================================

/// Generate a Docker Compose service fragment from the deployment contract.
#[must_use]
pub fn generate_compose_fragment(contract: &DeploymentContract) -> String {
    let binary = contract.binary();
    let mut out = String::with_capacity(512);

    // Service definition
    out.push_str(&format!(
        "# Generated by hyperi-rustlib deployment module\nservices:\n  {}:\n",
        contract.app_name
    ));

    // Image
    out.push_str(&format!(
        "    image: {}/{}:${{{}_VERSION:-latest}}\n",
        contract.image_registry,
        contract.app_name,
        contract.env_prefix.replace("__", "_")
    ));

    // depends_on
    if !contract.depends_on.is_empty() {
        out.push_str("    depends_on:\n");
        for dep in &contract.depends_on {
            out.push_str(&format!(
                "      {dep}:\n        condition: service_healthy\n"
            ));
        }
    }

    // Ports
    out.push_str("    ports:\n");
    out.push_str(&format!(
        "      - \"{}:{}\"\n",
        contract.metrics_port, contract.metrics_port
    ));
    for p in &contract.extra_ports {
        out.push_str(&format!("      - \"{}:{}\"\n", p.port, p.port));
    }

    // Volumes — config file mount
    out.push_str("    volumes:\n");
    out.push_str(&format!(
        "      - ./config/{}:{}:ro\n",
        contract.config_filename(),
        contract.config_mount_path,
    ));

    // Healthcheck
    out.push_str(&format!(
        "    healthcheck:\n\
         \x20     test: [\"CMD\", \"curl\", \"-sf\", \"http://localhost:{}{}\"]
      interval: 10s\n\
         \x20     timeout: 3s\n\
         \x20     retries: 5\n",
        contract.metrics_port, contract.health.liveness_path,
    ));

    // Entrypoint args
    if !contract.entrypoint_args.is_empty() {
        out.push_str(&format!("    command: [\"{binary}\""));
        for arg in &contract.entrypoint_args {
            out.push_str(&format!(", \"{arg}\""));
        }
        out.push_str("]\n");
    }

    out
}

// ============================================================================
// Helm chart
// ============================================================================

/// Generate a complete Helm chart directory from the deployment contract.
///
/// Writes `Chart.yaml`, `values.yaml`, and all template files to `output_dir`.
///
/// # Errors
///
/// Returns `DeploymentError` if files or directories cannot be created.
pub fn generate_chart(
    contract: &DeploymentContract,
    output_dir: impl AsRef<Path>,
) -> Result<(), DeploymentError> {
    let dir = output_dir.as_ref();
    let templates_dir = dir.join("templates");

    // Create directories
    std::fs::create_dir_all(&templates_dir).map_err(|e| DeploymentError::CreateDir {
        path: templates_dir.display().to_string(),
        source: e,
    })?;

    // Write all chart files
    write_file(dir.join("Chart.yaml"), &gen_chart_yaml(contract))?;
    write_file(dir.join("values.yaml"), &gen_values_yaml(contract))?;
    write_file(
        templates_dir.join("_helpers.tpl"),
        &gen_helpers_tpl(contract),
    )?;
    write_file(
        templates_dir.join("deployment.yaml"),
        &gen_deployment_yaml(contract),
    )?;
    write_file(
        templates_dir.join("service.yaml"),
        &gen_service_yaml(contract),
    )?;
    write_file(
        templates_dir.join("serviceaccount.yaml"),
        &gen_serviceaccount_yaml(contract),
    )?;
    write_file(
        templates_dir.join("configmap.yaml"),
        &gen_configmap_yaml(contract),
    )?;
    write_file(
        templates_dir.join("secret.yaml"),
        &gen_secret_yaml(contract),
    )?;
    write_file(templates_dir.join("hpa.yaml"), &gen_hpa_yaml(contract))?;

    if contract.keda.is_some() {
        write_file(
            templates_dir.join("keda-scaledobject.yaml"),
            &gen_keda_scaledobject_yaml(contract),
        )?;
        write_file(
            templates_dir.join("keda-triggerauth.yaml"),
            &gen_keda_triggerauth_yaml(contract),
        )?;
    }

    write_file(templates_dir.join("NOTES.txt"), &gen_notes_txt(contract))?;

    Ok(())
}

// ============================================================================
// Chart file generators
// ============================================================================

fn gen_chart_yaml(c: &DeploymentContract) -> String {
    format!(
        "apiVersion: v2\n\
         name: {name}\n\
         description: {desc}\n\
         type: application\n\
         version: 0.1.0\n\
         appVersion: \"1.0.0\"\n\
         \n\
         keywords:\n\
         \x20 - hyperi\n\
         \x20 - dfe\n\
         \n\
         maintainers:\n\
         \x20 - name: HyperI\n\
         \x20   url: https://github.com/hyperi-io\n",
        name = c.app_name,
        desc = if c.description.is_empty() {
            &c.app_name
        } else {
            &c.description
        },
    )
}

fn gen_values_yaml(c: &DeploymentContract) -> String {
    let mut out = String::with_capacity(2048);

    // Header comment
    out.push_str(&format!(
        "# {app} Helm chart values\n\
         #\n\
         # Generated by hyperi-rustlib deployment module.\n\
         # Contract points validated by cargo test.\n\
         \n",
        app = c.app_name,
    ));

    // Replicas, image, overrides
    out.push_str(&format!(
        "# -- Number of replicas (ignored when KEDA is enabled)\n\
         replicaCount: 1\n\
         \n\
         image:\n\
         \x20 repository: {registry}/{app}\n\
         \x20 # -- Defaults to Chart appVersion\n\
         \x20 tag: \"\"\n\
         \x20 pullPolicy: IfNotPresent\n\
         \n\
         imagePullSecrets: []\n\
         nameOverride: \"\"\n\
         fullnameOverride: \"\"\n\
         \n",
        registry = c.image_registry,
        app = c.app_name,
    ));

    // Service account
    out.push_str(
        "serviceAccount:\n\
         \x20 create: true\n\
         \x20 annotations: {}\n\
         \x20 # -- If not set, name is generated from fullname\n\
         \x20 name: \"\"\n\
         \n",
    );

    // Pod annotations (Prometheus)
    out.push_str(&format!(
        "# -- Pod annotations (Prometheus scrape config included by default)\n\
         podAnnotations:\n\
         \x20 prometheus.io/scrape: \"true\"\n\
         \x20 prometheus.io/port: \"{port}\"\n\
         \x20 prometheus.io/path: \"{metrics_path}\"\n\
         \n\
         podLabels: {{}}\n\
         \n",
        port = c.metrics_port,
        metrics_path = c.health.metrics_path,
    ));

    // Resources
    out.push_str(
        "resources:\n\
         \x20 requests:\n\
         \x20   cpu: 250m\n\
         \x20   memory: 256Mi\n\
         \x20 limits:\n\
         \x20   cpu: \"2\"\n\
         \x20   memory: 1Gi\n\
         \n",
    );

    // Service
    out.push_str(&format!(
        "# -- Metrics and health endpoint service\n\
         service:\n\
         \x20 type: ClusterIP\n\
         \x20 port: {port}\n\
         \n",
        port = c.metrics_port,
    ));

    // App config section
    out.push_str(&format!(
        "# -- Application configuration (mounted as {})\n",
        c.config_mount_path
    ));
    if let Some(ref config) = c.default_config {
        out.push_str("config:\n");
        // Serialise the config value as YAML and indent by 2
        if let Ok(yaml) = serde_yaml_ng::to_string(config) {
            for line in yaml.lines() {
                if line == "---" {
                    continue;
                }
                out.push_str(&format!("  {line}\n"));
            }
        }
    } else {
        out.push_str("config: {}\n");
    }
    out.push('\n');

    // Secret sections
    for group in &c.secrets {
        out.push_str(&format!(
            "# -- {} credentials\n\
             {}:\n\
             \x20 existingSecret: \"\"\n\
             \x20 secretKeys:\n",
            group.group_name, group.group_name,
        ));
        for env in &group.env_vars {
            out.push_str(&format!("    {}: {}\n", env.key_name, env.secret_key));
        }
        for env in &group.env_vars {
            out.push_str(&format!("  {}: \"\"\n", env.key_name));
        }
        out.push('\n');
    }

    // KEDA section
    if let Some(ref keda) = c.keda {
        out.push_str(&format!(
            "# -- KEDA autoscaling (requires KEDA operator installed)\n\
             keda:\n\
             \x20 enabled: true\n\
             \x20 minReplicaCount: {min}\n\
             \x20 maxReplicaCount: {max}\n\
             \x20 pollingInterval: {poll}\n\
             \x20 cooldownPeriod: {cool}\n\
             \x20 kafka:\n\
             \x20   # -- Scale when consumer group lag exceeds this per partition\n\
             \x20   lagThreshold: \"{lag}\"\n\
             \x20   # -- Wake from zero replicas when lag exceeds this\n\
             \x20   activationLagThreshold: \"{activation}\"\n\
             \x20   # -- Override topic (default: first topic from config)\n\
             \x20   topic: \"\"\n\
             \x20   # -- Override consumer group (default: from config)\n\
             \x20   consumerGroup: \"\"\n\
             \x20 cpu:\n\
             \x20   enabled: {cpu_enabled}\n\
             \x20   # -- CPU utilisation percentage threshold\n\
             \x20   threshold: \"{cpu_threshold}\"\n\
             \n",
            min = keda.min_replicas,
            max = keda.max_replicas,
            poll = keda.polling_interval,
            cool = keda.cooldown_period,
            lag = keda.kafka_lag_threshold,
            activation = keda.activation_lag_threshold,
            cpu_enabled = keda.cpu_enabled,
            cpu_threshold = keda.cpu_threshold,
        ));
    }

    // HPA fallback
    out.push_str(
        "# -- Standard HPA fallback (when KEDA is not installed)\n\
         # Mutually exclusive with keda.enabled\n\
         autoscaling:\n\
         \x20 enabled: false\n\
         \x20 minReplicas: 1\n\
         \x20 maxReplicas: 10\n\
         \x20 targetCPUUtilizationPercentage: 80\n\
         \n\
         nodeSelector: {}\n\
         tolerations: []\n\
         affinity: {}\n",
    );

    out
}

fn gen_helpers_tpl(c: &DeploymentContract) -> String {
    let app = &c.app_name;
    let mut out = String::with_capacity(2048);

    // Standard helpers
    out.push_str(&format!(
        r#"{{{{/*
Expand the name of the chart.
*/}}}}
{{{{- define "{app}.name" -}}}}
{{{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}}}
{{{{- end }}}}

{{{{/*
Create a default fully qualified app name.
Truncated at 63 chars because some K8s name fields are limited.
*/}}}}
{{{{- define "{app}.fullname" -}}}}
{{{{- if .Values.fullnameOverride }}}}
{{{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}}}
{{{{- else }}}}
{{{{- $name := default .Chart.Name .Values.nameOverride }}}}
{{{{- if contains $name .Release.Name }}}}
{{{{- .Release.Name | trunc 63 | trimSuffix "-" }}}}
{{{{- else }}}}
{{{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}}}
{{{{- end }}}}
{{{{- end }}}}
{{{{- end }}}}

{{{{/*
Create chart name and version as used by the chart label.
*/}}}}
{{{{- define "{app}.chart" -}}}}
{{{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}}}
{{{{- end }}}}

{{{{/*
Common labels.
*/}}}}
{{{{- define "{app}.labels" -}}}}
helm.sh/chart: {{{{ include "{app}.chart" . }}}}
{{{{ include "{app}.selectorLabels" . }}}}
{{{{- if .Chart.AppVersion }}}}
app.kubernetes.io/version: {{{{ .Chart.AppVersion | quote }}}}
{{{{- end }}}}
app.kubernetes.io/managed-by: {{{{ .Release.Service }}}}
{{{{- end }}}}

{{{{/*
Selector labels.
*/}}}}
{{{{- define "{app}.selectorLabels" -}}}}
app.kubernetes.io/name: {{{{ include "{app}.name" . }}}}
app.kubernetes.io/instance: {{{{ .Release.Name }}}}
{{{{- end }}}}

{{{{/*
Service account name.
*/}}}}
{{{{- define "{app}.serviceAccountName" -}}}}
{{{{- if .Values.serviceAccount.create }}}}
{{{{- default (include "{app}.fullname" .) .Values.serviceAccount.name }}}}
{{{{- else }}}}
{{{{- default "default" .Values.serviceAccount.name }}}}
{{{{- end }}}}
{{{{- end }}}}
"#,
    ));

    // Secret name helpers — one per secret group
    for group in &c.secrets {
        let helper_name = format!("{}SecretName", to_camel_suffix(&group.group_name));
        out.push_str(&format!(
            r#"
{{{{/*
{group} secret name — use existing or generate from fullname.
*/}}}}
{{{{- define "{app}.{helper}" -}}}}
{{{{- if .Values.{group}.existingSecret }}}}
{{{{- .Values.{group}.existingSecret }}}}
{{{{- else }}}}
{{{{- printf "%s-{group}" (include "{app}.fullname" .) }}}}
{{{{- end }}}}
{{{{- end }}}}
"#,
            app = app,
            group = group.group_name,
            helper = helper_name,
        ));
    }

    out
}

fn gen_deployment_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;
    let mut out = String::with_capacity(4096);

    // Header
    out.push_str(&format!(
        r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{{{ include "{app}.fullname" . }}}}
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
spec:
  {{{{- if not (or .Values.keda.enabled .Values.autoscaling.enabled) }}}}
  replicas: {{{{ .Values.replicaCount }}}}
  {{{{- end }}}}
  selector:
    matchLabels:
      {{{{- include "{app}.selectorLabels" . | nindent 6 }}}}
  template:
    metadata:
      annotations:
        checksum/config: {{{{ include (print $.Template.BasePath "/configmap.yaml") . | sha256sum }}}}
        {{{{- with .Values.podAnnotations }}}}
        {{{{- toYaml . | nindent 8 }}}}
        {{{{- end }}}}
      labels:
        {{{{- include "{app}.labels" . | nindent 8 }}}}
        {{{{- with .Values.podLabels }}}}
        {{{{- toYaml . | nindent 8 }}}}
        {{{{- end }}}}
    spec:
      {{{{- with .Values.imagePullSecrets }}}}
      imagePullSecrets:
        {{{{- toYaml . | nindent 8 }}}}
      {{{{- end }}}}
      serviceAccountName: {{{{ include "{app}.serviceAccountName" . }}}}
      containers:
        - name: {{{{ .Chart.Name }}}}
          image: "{{{{ .Values.image.repository }}}}:{{{{ .Values.image.tag | default .Chart.AppVersion }}}}"
          imagePullPolicy: {{{{ .Values.image.pullPolicy }}}}
"#,
    ));

    // Args
    if !c.entrypoint_args.is_empty() {
        out.push_str("          args:\n");
        for arg in &c.entrypoint_args {
            out.push_str(&format!("            - \"{arg}\"\n"));
        }
    }

    // Ports
    out.push_str(
        "          ports:\n\
         \x20           - name: metrics\n\
         \x20             containerPort: {{ .Values.service.port }}\n\
         \x20             protocol: TCP\n",
    );
    for port in &c.extra_ports {
        out.push_str(&format!(
            "            - name: {name}\n\
             \x20             containerPort: {port}\n\
             \x20             protocol: {proto}\n",
            name = port.name,
            port = port.port,
            proto = port.protocol,
        ));
    }

    // Env vars from secrets
    if !c.secrets.is_empty() {
        out.push_str("          env:\n");
        for group in &c.secrets {
            let helper_name = format!("{}SecretName", to_camel_suffix(&group.group_name));
            out.push_str(&format!(
                "            # {} credentials via Secret (figment env cascade overrides file config)\n",
                group.group_name
            ));
            for env in &group.env_vars {
                out.push_str(&format!(
                    "            - name: {env_var}\n\
                     \x20             valueFrom:\n\
                     \x20               secretKeyRef:\n\
                     \x20                 name: {{{{ include \"{app}.{helper}\" . }}}}\n\
                     \x20                 key: {{{{ .Values.{group}.secretKeys.{key} }}}}\n",
                    env_var = env.env_var,
                    app = app,
                    helper = helper_name,
                    group = group.group_name,
                    key = env.key_name,
                ));
            }
        }
    }

    // Probes
    out.push_str(&format!(
        "          livenessProbe:\n\
         \x20           httpGet:\n\
         \x20             path: {liveness}\n\
         \x20             port: metrics\n\
         \x20           initialDelaySeconds: 10\n\
         \x20           periodSeconds: 10\n\
         \x20           failureThreshold: 3\n\
         \x20         readinessProbe:\n\
         \x20           httpGet:\n\
         \x20             path: {readiness}\n\
         \x20             port: metrics\n\
         \x20           initialDelaySeconds: 5\n\
         \x20           periodSeconds: 5\n\
         \x20           failureThreshold: 2\n\
         \x20         startupProbe:\n\
         \x20           httpGet:\n\
         \x20             path: {liveness}\n\
         \x20             port: metrics\n\
         \x20           failureThreshold: 30\n\
         \x20           periodSeconds: 5\n",
        liveness = c.health.liveness_path,
        readiness = c.health.readiness_path,
    ));

    // Volume mounts
    out.push_str(&format!(
        "          volumeMounts:\n\
         \x20           - name: config\n\
         \x20             mountPath: {config_dir}\n\
         \x20             readOnly: true\n",
        config_dir = c.config_dir(),
    ));

    // Resources
    out.push_str(
        "          {{- with .Values.resources }}\n\
         \x20         resources:\n\
         \x20           {{- toYaml . | nindent 12 }}\n\
         \x20         {{- end }}\n",
    );

    // Volumes
    out.push_str(&format!(
        "      volumes:\n\
         \x20       - name: config\n\
         \x20         configMap:\n\
         \x20           name: {{{{ include \"{app}.fullname\" . }}}}-config\n",
    ));

    // Node selector, affinity, tolerations
    out.push_str(
        "      {{- with .Values.nodeSelector }}\n\
         \x20     nodeSelector:\n\
         \x20       {{- toYaml . | nindent 8 }}\n\
         \x20     {{- end }}\n\
         \x20     {{- with .Values.affinity }}\n\
         \x20     affinity:\n\
         \x20       {{- toYaml . | nindent 8 }}\n\
         \x20     {{- end }}\n\
         \x20     {{- with .Values.tolerations }}\n\
         \x20     tolerations:\n\
         \x20       {{- toYaml . | nindent 8 }}\n\
         \x20     {{- end }}\n",
    );

    out
}

fn gen_service_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;
    let mut out = format!(
        r#"apiVersion: v1
kind: Service
metadata:
  name: {{{{ include "{app}.fullname" . }}}}
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
spec:
  type: {{{{ .Values.service.type }}}}
  ports:
    - port: {{{{ .Values.service.port }}}}
      targetPort: metrics
      protocol: TCP
      name: metrics
"#,
    );

    // Extra ports
    for port in &c.extra_ports {
        out.push_str(&format!(
            "    - port: {port}\n\
             \x20     targetPort: {port}\n\
             \x20     protocol: {proto}\n\
             \x20     name: {name}\n",
            port = port.port,
            proto = port.protocol,
            name = port.name,
        ));
    }

    out.push_str(&format!(
        "  selector:\n\
         \x20   {{{{- include \"{app}.selectorLabels\" . | nindent 4 }}}}\n",
    ));

    out
}

fn gen_serviceaccount_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;
    format!(
        r#"{{{{- if .Values.serviceAccount.create -}}}}
apiVersion: v1
kind: ServiceAccount
metadata:
  name: {{{{ include "{app}.serviceAccountName" . }}}}
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
  {{{{- with .Values.serviceAccount.annotations }}}}
  annotations:
    {{{{- toYaml . | nindent 4 }}}}
  {{{{- end }}}}
automountServiceAccountToken: false
{{{{- end }}}}
"#,
    )
}

fn gen_configmap_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;

    let mut out = format!(
        r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: {{{{ include "{app}.fullname" . }}}}-config
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
data:
  {filename}: |
    {{{{- toYaml .Values.config | nindent 4 }}}}
"#,
        app = app,
        filename = c.config_filename(),
    );

    let _ = &mut out; // keep borrow checker happy
    out
}

fn gen_secret_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;
    let mut out = String::new();
    let mut first = true;

    for group in &c.secrets {
        if !first {
            out.push_str("---\n");
        }
        first = false;

        let helper_name = format!("{}SecretName", to_camel_suffix(&group.group_name));

        out.push_str(&format!(
            "{{{{- if not .Values.{group}.existingSecret }}}}\n\
             apiVersion: v1\n\
             kind: Secret\n\
             metadata:\n\
             \x20 name: {{{{ include \"{app}.{helper}\" . }}}}\n\
             \x20 labels:\n\
             \x20   {{{{- include \"{app}.labels\" . | nindent 4 }}}}\n\
             type: Opaque\n\
             data:\n",
            group = group.group_name,
            app = app,
            helper = helper_name,
        ));

        for env in &group.env_vars {
            out.push_str(&format!(
                "  {{{{ .Values.{group}.secretKeys.{key} }}}}: {{{{ .Values.{group}.{key} | b64enc | quote }}}}\n",
                group = group.group_name,
                key = env.key_name,
            ));
        }

        out.push_str("{{- end }}\n");
    }

    if c.secrets.is_empty() {
        out.push_str("# No secrets defined in deployment contract\n");
    }

    out
}

fn gen_hpa_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;
    format!(
        r#"{{{{- if and .Values.autoscaling.enabled (not .Values.keda.enabled) }}}}
# Standard HPA fallback — use when KEDA operator is not installed.
# Mutually exclusive with keda.enabled (KEDA creates its own HPA).
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: {{{{ include "{app}.fullname" . }}}}
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: {{{{ include "{app}.fullname" . }}}}
  minReplicas: {{{{ .Values.autoscaling.minReplicas }}}}
  maxReplicas: {{{{ .Values.autoscaling.maxReplicas }}}}
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: {{{{ .Values.autoscaling.targetCPUUtilizationPercentage }}}}
{{{{- end }}}}
"#,
    )
}

fn gen_keda_scaledobject_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;

    // Find kafka secret group for trigger auth reference
    let has_kafka_secret = c.secrets.iter().any(|g| g.group_name == "kafka");

    let auth_ref = if has_kafka_secret {
        format!(
            "      authenticationRef:\n\
             \x20       name: {{{{ include \"{app}.fullname\" . }}}}-kafka-auth\n"
        )
    } else {
        String::new()
    };

    format!(
        r#"{{{{- if .Values.keda.enabled }}}}
apiVersion: keda.sh/v1alpha1
kind: ScaledObject
metadata:
  name: {{{{ include "{app}.fullname" . }}}}
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
spec:
  scaleTargetRef:
    name: {{{{ include "{app}.fullname" . }}}}
  minReplicaCount: {{{{ .Values.keda.minReplicaCount }}}}
  maxReplicaCount: {{{{ .Values.keda.maxReplicaCount }}}}
  pollingInterval: {{{{ .Values.keda.pollingInterval }}}}
  cooldownPeriod: {{{{ .Values.keda.cooldownPeriod }}}}
  triggers:
    # Kafka consumer group lag (primary scaler)
    - type: kafka
{auth_ref}      metadata:
        bootstrapServers: {{{{ .Values.config.kafka.brokers | quote }}}}
        consumerGroup: {{{{ .Values.keda.kafka.consumerGroup | default .Values.config.kafka.group_id | quote }}}}
        topic: {{{{ .Values.keda.kafka.topic | default (index .Values.config.kafka.topics 0) | quote }}}}
        lagThreshold: {{{{ .Values.keda.kafka.lagThreshold | quote }}}}
        activationLagThreshold: {{{{ .Values.keda.kafka.activationLagThreshold | quote }}}}
        saslType: scram_sha512
        tls: disable
    {{{{- if .Values.keda.cpu.enabled }}}}
    # CPU utilisation (secondary scaler)
    - type: cpu
      metricType: Utilization
      metadata:
        value: {{{{ .Values.keda.cpu.threshold | quote }}}}
    {{{{- end }}}}
{{{{- end }}}}
"#,
    )
}

fn gen_keda_triggerauth_yaml(c: &DeploymentContract) -> String {
    let app = &c.app_name;

    // Find the kafka secret group
    let kafka_group = c.secrets.iter().find(|g| g.group_name == "kafka");

    if kafka_group.is_none() {
        return "# No kafka secret group — KEDA TriggerAuthentication not generated\n".to_string();
    }

    let helper_name = format!("{}SecretName", to_camel_suffix("kafka"));

    format!(
        r#"{{{{- if .Values.keda.enabled }}}}
apiVersion: keda.sh/v1alpha1
kind: TriggerAuthentication
metadata:
  name: {{{{ include "{app}.fullname" . }}}}-kafka-auth
  labels:
    {{{{- include "{app}.labels" . | nindent 4 }}}}
spec:
  secretTargetRef:
    - parameter: sasl
      name: {{{{ include "{app}.{helper_name}" . }}}}
      key: {{{{ .Values.kafka.secretKeys.username }}}}
    - parameter: password
      name: {{{{ include "{app}.{helper_name}" . }}}}
      key: {{{{ .Values.kafka.secretKeys.password }}}}
{{{{- end }}}}
"#,
    )
}

fn gen_notes_txt(c: &DeploymentContract) -> String {
    let app = &c.app_name;

    format!(
        r#"{app} has been deployed.

1. Get the metrics/health endpoint:
   kubectl port-forward svc/{{{{ include "{app}.fullname" . }}}} {{{{ .Values.service.port }}}}:{{{{ .Values.service.port }}}}
   curl http://localhost:{{{{ .Values.service.port }}}}{liveness}
   curl http://localhost:{{{{ .Values.service.port }}}}{metrics}

{{{{- if .Values.keda.enabled }}}}

2. Check KEDA autoscaling status:
   kubectl get scaledobject {{{{ include "{app}.fullname" . }}}}
   kubectl get hpa
{{{{- end }}}}

3. View logs:
   kubectl logs -l app.kubernetes.io/name={{{{ include "{app}.name" . }}}} -f
"#,
        app = app,
        liveness = c.health.liveness_path,
        metrics = c.health.metrics_path,
    )
}

// ============================================================================
// Helpers
// ============================================================================

/// Convert a group name to camelCase suffix (e.g., "kafka" -> "kafka", "click_house" -> "clickHouse").
fn to_camel_suffix(name: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;

    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(ch.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
        }
    }

    result
}

fn write_file(path: impl AsRef<Path>, content: &str) -> Result<(), DeploymentError> {
    let path = path.as_ref();
    std::fs::write(path, content).map_err(|e| DeploymentError::WriteFile {
        path: path.display().to_string(),
        source: e,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::contract::{PortContract, SecretEnvContract, SecretGroupContract};
    use crate::deployment::keda::KedaContract;
    use crate::deployment::native_deps::NativeDepsContract;

    fn test_contract() -> DeploymentContract {
        DeploymentContract {
            app_name: "dfe-loader".into(),
            binary_name: "dfe-loader".into(),
            description: "High-performance Kafka to ClickHouse data loader".into(),
            metrics_port: 9090,
            health: super::super::HealthContract::default(),
            env_prefix: "DFE_LOADER".into(),
            metric_prefix: "loader".into(),
            config_mount_path: "/etc/dfe/loader.yaml".into(),
            image_registry: "ghcr.io/hyperi-io".into(),
            extra_ports: vec![],
            entrypoint_args: vec!["--config".into(), "/etc/dfe/loader.yaml".into()],
            secrets: vec![
                SecretGroupContract {
                    group_name: "kafka".into(),
                    env_vars: vec![
                        SecretEnvContract {
                            env_var: "DFE_LOADER__KAFKA__USERNAME".into(),
                            key_name: "username".into(),
                            secret_key: "kafka-username".into(),
                        },
                        SecretEnvContract {
                            env_var: "DFE_LOADER__KAFKA__PASSWORD".into(),
                            key_name: "password".into(),
                            secret_key: "kafka-password".into(),
                        },
                    ],
                },
                SecretGroupContract {
                    group_name: "clickhouse".into(),
                    env_vars: vec![SecretEnvContract {
                        env_var: "DFE_LOADER__CLICKHOUSE__PASSWORD".into(),
                        key_name: "password".into(),
                        secret_key: "clickhouse-password".into(),
                    }],
                },
            ],
            default_config: None,
            depends_on: vec!["kafka".into(), "clickhouse".into()],
            keda: Some(KedaContract::default()),
            base_image: "ubuntu:24.04".into(),
            native_deps: NativeDepsContract::default(),
            image_profile: ImageProfile::default(),
        }
    }

    #[test]
    fn test_generate_dockerfile() {
        let contract = test_contract();
        let dockerfile = generate_dockerfile(&contract);

        assert!(dockerfile.contains("FROM ubuntu:24.04"));
        assert!(dockerfile.contains("COPY dfe-loader /usr/local/bin/dfe-loader"));
        assert!(dockerfile.contains("EXPOSE 9090"));
        assert!(dockerfile.contains("localhost:9090/healthz"));
        assert!(dockerfile.contains("ENTRYPOINT [\"dfe-loader\"]"));
        assert!(dockerfile.contains("CMD [\"--config\", \"/etc/dfe/loader.yaml\"]"));
    }

    #[test]
    fn test_generate_dockerfile_with_native_deps() {
        let mut contract = test_contract();
        contract.native_deps = NativeDepsContract::for_rustlib_features(
            &["transport-kafka", "spool", "tiered-sink"],
            "ubuntu:24.04",
        );

        let dockerfile = generate_dockerfile(&contract);

        // Should contain Confluent APT repo setup
        assert!(dockerfile.contains("packages.confluent.io"));
        assert!(dockerfile.contains("confluent-clients.gpg"));
        // Should contain runtime packages
        assert!(dockerfile.contains("librdkafka1"));
        assert!(dockerfile.contains("libssl3"));
        assert!(dockerfile.contains("libzstd1"));
        // Should include gnupg for key import
        assert!(dockerfile.contains("gnupg"));
    }

    #[test]
    fn test_generate_dockerfile_no_native_deps() {
        let mut contract = test_contract();
        contract.native_deps = NativeDepsContract::for_rustlib_features(
            &["cli", "deployment", "logger"],
            "ubuntu:24.04",
        );

        let dockerfile = generate_dockerfile(&contract);

        // No Confluent repo, no runtime packages
        assert!(!dockerfile.contains("confluent"));
        assert!(!dockerfile.contains("librdkafka1"));
        assert!(!dockerfile.contains("gnupg"));
    }

    #[test]
    fn test_generate_dockerfile_bookworm_codename() {
        let mut contract = test_contract();
        contract.base_image = "debian:bookworm-slim".into();
        contract.native_deps =
            NativeDepsContract::for_rustlib_features(&["transport-kafka"], "debian:bookworm-slim");

        let dockerfile = generate_dockerfile(&contract);
        assert!(dockerfile.contains("bookworm main"));
    }

    #[test]
    fn test_generate_dockerfile_production_profile() {
        let contract = test_contract();
        let dockerfile = generate_dockerfile(&contract);

        assert!(dockerfile.contains("Purpose:   production container image"));
        assert!(dockerfile.contains("io.hyperi.profile=\"production\""));
        assert!(!dockerfile.contains("strace"));
        assert!(!dockerfile.contains("tcpdump"));
    }

    #[test]
    fn test_generate_dockerfile_dev_profile() {
        let contract = test_contract().with_dev_profile();
        let dockerfile = generate_dockerfile(&contract);

        assert!(dockerfile.contains("Purpose:   development container image"));
        assert!(dockerfile.contains("io.hyperi.profile=\"development\""));
        assert!(dockerfile.contains("strace"));
        assert!(dockerfile.contains("tcpdump"));
        assert!(dockerfile.contains("procps"));
        assert!(dockerfile.contains("bash"));
        assert!(dockerfile.contains("jq"));
    }

    #[test]
    fn test_generate_dockerfile_dev_with_native_deps() {
        let mut contract = test_contract();
        contract.native_deps =
            NativeDepsContract::for_rustlib_features(&["transport-kafka", "spool"], "ubuntu:24.04");
        let dev = contract.with_dev_profile();
        let dockerfile = generate_dockerfile(&dev);

        // Dev tools present alongside native deps
        assert!(dockerfile.contains("strace"));
        assert!(dockerfile.contains("librdkafka1"));
        assert!(dockerfile.contains("libzstd1"));
        assert!(dockerfile.contains("io.hyperi.profile=\"development\""));
    }

    #[test]
    fn test_with_dev_profile_preserves_contract() {
        let contract = test_contract();
        let dev = contract.with_dev_profile();

        assert_eq!(dev.app_name, contract.app_name);
        assert_eq!(dev.metrics_port, contract.metrics_port);
        assert_eq!(dev.image_profile, ImageProfile::Development);
        assert_eq!(contract.image_profile, ImageProfile::Production);
    }

    #[test]
    fn test_generate_dockerfile_extra_ports() {
        let mut contract = test_contract();
        contract.extra_ports = vec![PortContract {
            name: "http".into(),
            port: 8080,
            protocol: "TCP".into(),
        }];

        let dockerfile = generate_dockerfile(&contract);
        assert!(dockerfile.contains("EXPOSE 9090 8080"));
    }

    #[test]
    fn test_generate_compose_fragment() {
        let contract = test_contract();
        let compose = generate_compose_fragment(&contract);

        assert!(compose.contains("dfe-loader:"));
        assert!(compose.contains("ghcr.io/hyperi-io/dfe-loader"));
        assert!(compose.contains("kafka:"));
        assert!(compose.contains("clickhouse:"));
        assert!(compose.contains("condition: service_healthy"));
        assert!(compose.contains("\"9090:9090\""));
        assert!(compose.contains("loader.yaml:/etc/dfe/loader.yaml:ro"));
    }

    #[test]
    fn test_generate_chart() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();

        generate_chart(&contract, dir.path()).unwrap();

        // Verify files exist
        assert!(dir.path().join("Chart.yaml").exists());
        assert!(dir.path().join("values.yaml").exists());
        assert!(dir.path().join("templates/_helpers.tpl").exists());
        assert!(dir.path().join("templates/deployment.yaml").exists());
        assert!(dir.path().join("templates/service.yaml").exists());
        assert!(dir.path().join("templates/serviceaccount.yaml").exists());
        assert!(dir.path().join("templates/configmap.yaml").exists());
        assert!(dir.path().join("templates/secret.yaml").exists());
        assert!(dir.path().join("templates/hpa.yaml").exists());
        assert!(dir.path().join("templates/keda-scaledobject.yaml").exists());
        assert!(dir.path().join("templates/keda-triggerauth.yaml").exists());
        assert!(dir.path().join("templates/NOTES.txt").exists());
    }

    #[test]
    fn test_chart_yaml_content() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("Chart.yaml")).unwrap();
        assert!(content.contains("name: dfe-loader"));
        assert!(content.contains("description: High-performance Kafka to ClickHouse data loader"));
    }

    #[test]
    fn test_values_yaml_content() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("values.yaml")).unwrap();
        assert!(content.contains("port: 9090"));
        assert!(content.contains("prometheus.io/port: \"9090\""));
        assert!(content.contains("prometheus.io/path: \"/metrics\""));
        assert!(content.contains("lagThreshold: \"1000\""));
        assert!(content.contains("kafka-username"));
        assert!(content.contains("kafka-password"));
        assert!(content.contains("clickhouse-password"));
    }

    #[test]
    fn test_helpers_contain_secret_helpers() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("templates/_helpers.tpl")).unwrap();
        assert!(content.contains("kafkaSecretName"));
        assert!(content.contains("clickhouseSecretName"));
    }

    #[test]
    fn test_deployment_contains_env_vars() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path()).unwrap();

        let content =
            std::fs::read_to_string(dir.path().join("templates/deployment.yaml")).unwrap();
        assert!(content.contains("DFE_LOADER__KAFKA__USERNAME"));
        assert!(content.contains("DFE_LOADER__KAFKA__PASSWORD"));
        assert!(content.contains("DFE_LOADER__CLICKHOUSE__PASSWORD"));
        assert!(content.contains("path: /healthz"));
        assert!(content.contains("path: /readyz"));
        assert!(content.contains("/etc/dfe"));
    }

    #[test]
    fn test_no_keda_files_when_disabled() {
        let mut contract = test_contract();
        contract.keda = None;

        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path()).unwrap();

        assert!(!dir.path().join("templates/keda-scaledobject.yaml").exists());
        assert!(!dir.path().join("templates/keda-triggerauth.yaml").exists());
    }

    #[test]
    fn test_to_camel_suffix() {
        assert_eq!(to_camel_suffix("kafka"), "kafka");
        assert_eq!(to_camel_suffix("clickhouse"), "clickhouse");
        assert_eq!(to_camel_suffix("click_house"), "clickHouse");
        assert_eq!(to_camel_suffix("my-service"), "myService");
    }
}
