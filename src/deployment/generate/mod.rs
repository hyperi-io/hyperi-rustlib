// Project:   hyperi-rustlib
// File:      src/deployment/generate/mod.rs
// Purpose:   Generate deployment artifacts from DeploymentContract
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Generate deployment artifacts (Dockerfile, Helm chart, Compose fragment,
//! container manifest, ArgoCD Application) from a
//! [`DeploymentContract`](crate::deployment::DeploymentContract).
//!
//! Apps provide ~20% customisation (ports, secrets, config); this module
//! generates ~80% boilerplate. Split by artefact kind into submodules;
//! the public surface is unchanged (re-exported here).

mod argocd;
mod common;
mod compose;
mod dockerfile;
mod helm;
mod manifest;

pub use argocd::{ArgocdConfig, generate_argocd_application};
pub use compose::generate_compose_fragment;
pub use dockerfile::{generate_dockerfile, generate_runtime_stage};
pub use helm::generate_chart;
pub use manifest::generate_container_manifest;

// Tests call these private helpers + contract types directly via `use super::*`.
#[cfg(test)]
use crate::deployment::contract::{DeploymentContract, ImageProfile};
#[cfg(test)]
use common::{is_go_identifier, safe_template_lookup, to_camel_suffix};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::contract::{
        OciLabels, PortContract, SecretEnvContract, SecretGroupContract,
    };
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
            schema_version: 2,
            oci_labels: OciLabels::default(),
        }
    }

    #[test]
    fn test_generate_dockerfile() {
        let contract = test_contract();
        let dockerfile = generate_dockerfile(&contract, None);

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

        let dockerfile = generate_dockerfile(&contract, None);

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

        let dockerfile = generate_dockerfile(&contract, None);

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

        let dockerfile = generate_dockerfile(&contract, None);
        assert!(dockerfile.contains("bookworm main"));
    }

    #[test]
    fn test_generate_dockerfile_production_profile() {
        let contract = test_contract();
        let dockerfile = generate_dockerfile(&contract, None);

        assert!(dockerfile.contains("Purpose:   production container image"));
        assert!(dockerfile.contains("io.hyperi.profile=\"production\""));
        assert!(!dockerfile.contains("strace"));
        assert!(!dockerfile.contains("tcpdump"));
    }

    #[test]
    fn test_generate_dockerfile_dev_profile() {
        let contract = test_contract().with_dev_profile();
        let dockerfile = generate_dockerfile(&contract, None);

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
        let dockerfile = generate_dockerfile(&dev, None);

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

        let dockerfile = generate_dockerfile(&contract, None);
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

        generate_chart(&contract, dir.path(), None).unwrap();

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
        generate_chart(&contract, dir.path(), None).unwrap();

        let content = std::fs::read_to_string(dir.path().join("Chart.yaml")).unwrap();
        assert!(content.contains("name: dfe-loader"));
        assert!(content.contains("description: High-performance Kafka to ClickHouse data loader"));
    }

    #[test]
    fn test_values_yaml_content() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path(), None).unwrap();

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
        generate_chart(&contract, dir.path(), None).unwrap();

        let content = std::fs::read_to_string(dir.path().join("templates/_helpers.tpl")).unwrap();
        assert!(content.contains("kafkaSecretName"));
        assert!(content.contains("clickhouseSecretName"));
    }

    #[test]
    fn test_deployment_contains_env_vars() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path(), None).unwrap();

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
    fn test_is_go_identifier() {
        // Valid Go identifiers
        assert!(is_go_identifier("foo"));
        assert!(is_go_identifier("FOO"));
        assert!(is_go_identifier("foo_bar"));
        assert!(is_go_identifier("_underscore_start"));
        assert!(is_go_identifier("foo123"));
        assert!(is_go_identifier("a"));

        // Invalid -- would break Go templates
        assert!(!is_go_identifier("bearer-tokens")); // hyphen
        assert!(!is_go_identifier("foo.bar")); // dot
        assert!(!is_go_identifier("123foo")); // digit-leading
        assert!(!is_go_identifier("")); // empty
        assert!(!is_go_identifier("foo bar")); // space
        assert!(!is_go_identifier("foo:bar")); // colon
    }

    #[test]
    fn test_safe_template_lookup_chooses_form() {
        assert_eq!(
            safe_template_lookup(".Values.auth", "username"),
            ".Values.auth.username"
        );
        assert_eq!(
            safe_template_lookup(".Values.auth", "bearer-tokens"),
            "(index .Values.auth \"bearer-tokens\")"
        );
        assert_eq!(
            safe_template_lookup(".Values.kafka.secretKeys", "kafka-username"),
            "(index .Values.kafka.secretKeys \"kafka-username\")"
        );
    }

    /// Regression for the dfe-receiver canary 2026-05-25 finding:
    /// keda-scaledobject.yaml previously used
    /// `default (index .Values.config.kafka.topics 0)` which `helm lint`
    /// rejects with `error calling index: index of untyped nil` because
    /// Sprig's `default` evaluates both operands. The render must now
    /// use a conditional `if/else if/else` block instead.
    #[test]
    fn test_keda_scaledobject_topic_lookup_is_lint_safe() {
        let contract = test_contract();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path(), None).unwrap();

        let keda_yaml =
            std::fs::read_to_string(dir.path().join("templates/keda-scaledobject.yaml")).unwrap();

        // Old broken form must not appear
        assert!(
            !keda_yaml.contains(
                ".Values.keda.kafka.topic | default (index .Values.config.kafka.topics 0)"
            ),
            "keda-scaledobject.yaml still uses the eagerly-evaluated `default (index ...)` form:\n{keda_yaml}"
        );

        // New conditional form must appear
        assert!(
            keda_yaml.contains("if .Values.keda.kafka.topic"),
            "keda-scaledobject.yaml missing if/else guard for topic lookup:\n{keda_yaml}"
        );
        assert!(
            keda_yaml.contains("else if .Values.config.kafka.topics"),
            "keda-scaledobject.yaml missing fallback branch for config.kafka.topics:\n{keda_yaml}"
        );
    }

    /// Regression for the dfe-receiver canary 2026-05-25 finding:
    /// secret.yaml previously emitted `.Values.x.bearer-tokens` which
    /// Go templates reject ("bad character U+002D '-'"). The render
    /// must now use the `(index .Values.x "bearer-tokens")` form.
    #[test]
    fn test_secret_yaml_handles_hyphenated_key_names() {
        let mut contract = test_contract();
        // dfe-receiver-style hyphenated key_name (token group)
        contract.secrets.push(SecretGroupContract {
            group_name: "auth".into(),
            env_vars: vec![SecretEnvContract {
                env_var: "DFE_RECEIVER__AUTH__BEARER_TOKENS".into(),
                key_name: "bearer-tokens".into(),
                secret_key: "bearer-tokens".into(),
            }],
        });

        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path(), None).unwrap();

        let secret_yaml =
            std::fs::read_to_string(dir.path().join("templates/secret.yaml")).unwrap();
        let deployment_yaml =
            std::fs::read_to_string(dir.path().join("templates/deployment.yaml")).unwrap();

        // Old broken form must not appear anywhere
        assert!(
            !secret_yaml.contains(".Values.auth.bearer-tokens"),
            "secret.yaml still uses broken dot-walked form for hyphenated key:\n{secret_yaml}"
        );
        assert!(
            !secret_yaml.contains(".Values.auth.secretKeys.bearer-tokens"),
            "secret.yaml still uses broken dot-walked form for hyphenated secretKeys lookup:\n{secret_yaml}"
        );
        assert!(
            !deployment_yaml.contains(".Values.auth.secretKeys.bearer-tokens"),
            "deployment.yaml still uses broken dot-walked form for hyphenated secretKeys lookup:\n{deployment_yaml}"
        );

        // Safe index form must appear
        assert!(
            secret_yaml.contains("(index .Values.auth.secretKeys \"bearer-tokens\")"),
            "secret.yaml missing index-form lookup for secretKeys.bearer-tokens:\n{secret_yaml}"
        );
        assert!(
            secret_yaml.contains("(index .Values.auth \"bearer-tokens\")"),
            "secret.yaml missing index-form lookup for value bearer-tokens:\n{secret_yaml}"
        );
        assert!(
            deployment_yaml.contains("(index .Values.auth.secretKeys \"bearer-tokens\")"),
            "deployment.yaml missing index-form lookup for secretKeys.bearer-tokens:\n{deployment_yaml}"
        );

        // Sanity: Go-safe keys (e.g. existing kafka.username) still use dot form
        assert!(
            secret_yaml.contains(".Values.kafka.secretKeys.username"),
            "Go-safe key 'username' should still use dot-walked form:\n{secret_yaml}"
        );
    }

    #[test]
    fn test_generate_argocd_application_default() {
        let contract = test_contract();
        let argo = ArgocdConfig {
            repo_url: "https://github.com/hyperi-io/dfe-loader".into(),
            ..Default::default()
        };
        let yaml = generate_argocd_application(&contract, &argo, None);

        assert!(yaml.contains("apiVersion: argoproj.io/v1alpha1"));
        assert!(yaml.contains("kind: Application"));
        assert!(yaml.contains("name: dfe-loader"));
        assert!(yaml.contains("namespace: argocd"));
        assert!(yaml.contains("repoURL: https://github.com/hyperi-io/dfe-loader"));
        assert!(yaml.contains("targetRevision: main"));
        assert!(yaml.contains("path: chart"));
        assert!(yaml.contains("CreateNamespace=true"));
        assert!(yaml.contains("Schema version: "));
    }

    #[test]
    fn test_generate_argocd_custom_namespace_and_path() {
        let contract = test_contract();
        let argo = ArgocdConfig {
            repo_url: "https://github.com/hyperi-io/dfe-loader".into(),
            dest_namespace: "production".into(),
            chart_path: "deploy/chart".into(),
            target_revision: "v1.0.0".into(),
            sync_wave: 5,
            ..Default::default()
        };
        let yaml = generate_argocd_application(&contract, &argo, None);
        assert!(yaml.contains("namespace: production"));
        assert!(yaml.contains("path: deploy/chart"));
        assert!(yaml.contains("targetRevision: v1.0.0"));
        assert!(yaml.contains("sync-wave: \"5\""));
    }

    #[test]
    fn argocd_config_default_uses_wave_apps() {
        let cfg = ArgocdConfig::default();
        assert_eq!(cfg.sync_wave, crate::deployment::WAVE_APPS);
    }

    #[test]
    fn argocd_config_default_has_no_extra_ignore_differences() {
        let cfg = ArgocdConfig::default();
        assert!(cfg.extra_ignore_differences.is_empty());
    }

    #[test]
    fn generate_argocd_application_emits_default_ignore_differences() {
        let contract = test_contract();
        let argo = ArgocdConfig {
            repo_url: "https://github.com/hyperi-io/dfe-loader".into(),
            ..Default::default()
        };
        let yaml = generate_argocd_application(&contract, &argo, None);
        assert!(yaml.contains("ignoreDifferences:"));
        assert!(yaml.contains("/spec/replicas"));
        assert!(yaml.contains("/spec/clusterIP"));
        assert!(yaml.contains(".webhooks[].clientConfig.caBundle"));
    }

    #[test]
    fn generate_argocd_application_appends_extra_ignore_differences() {
        let contract = test_contract();
        let argo = ArgocdConfig {
            repo_url: "https://github.com/hyperi-io/dfe-loader".into(),
            extra_ignore_differences: vec![
                "- group: apps\n  kind: Deployment\n  jsonPointers:\n    - /spec/template/spec/containers/0/image".into(),
            ],
            ..Default::default()
        };
        let yaml = generate_argocd_application(&contract, &argo, None);
        assert!(yaml.contains("/spec/template/spec/containers/0/image"));
    }

    #[test]
    fn generate_argocd_application_sync_wave_annotation_uses_config_value() {
        let contract = test_contract();
        let argo = ArgocdConfig {
            repo_url: "https://github.com/hyperi-io/dfe-loader".into(),
            sync_wave: crate::deployment::WAVE_TOPICS,
            ..Default::default()
        };
        let yaml = generate_argocd_application(&contract, &argo, None);
        assert!(yaml.contains(r#"argocd.argoproj.io/sync-wave: "-5""#));
    }

    #[test]
    fn test_no_keda_files_when_disabled() {
        let mut contract = test_contract();
        contract.keda = None;

        let dir = tempfile::tempdir().unwrap();
        generate_chart(&contract, dir.path(), None).unwrap();

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

    // ============================================================================
    // Contract Identity Annotation Scheme v1 -- end-to-end wiring tests.
    // The unit tests for ContractIdentity itself live in
    // src/deployment/contract_identity.rs; these verify the three
    // generators each emit the three keys in the right surface.
    // ============================================================================

    fn test_identity() -> crate::deployment::ContractIdentity {
        crate::deployment::ContractIdentity::new(
            "0123456789abcdef0123456789abcdef01234567",
            "ghcr.io/hyperi-io/dfe-loader:v2.7.2",
        )
        .expect("test fixture must be valid")
    }

    #[test]
    fn dockerfile_omits_identity_block_when_none() {
        let dockerfile = generate_dockerfile(&test_contract(), None);
        assert!(!dockerfile.contains("io.hyperi.contract"));
    }

    #[test]
    fn dockerfile_emits_three_identity_labels_when_some() {
        let id = test_identity();
        let dockerfile = generate_dockerfile(&test_contract(), Some(&id));
        assert!(dockerfile.contains("LABEL io.hyperi.contract.version=\"v1\""));
        assert!(dockerfile.contains(
            "LABEL io.hyperi.contract.source-commit=\"0123456789abcdef0123456789abcdef01234567\""
        ));
        assert!(dockerfile.contains(
            "LABEL io.hyperi.contract.image-ref=\"ghcr.io/hyperi-io/dfe-loader:v2.7.2\""
        ));
        // The existing io.hyperi.profile label is unaffected.
        assert!(dockerfile.contains("LABEL io.hyperi.profile=\"production\""));
    }

    #[test]
    fn chart_yaml_omits_identity_block_when_none() {
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&test_contract(), dir.path(), None).unwrap();
        let chart = std::fs::read_to_string(dir.path().join("Chart.yaml")).unwrap();
        assert!(!chart.contains("io.hyperi.contract"));
    }

    #[test]
    fn chart_yaml_emits_three_identity_annotations_when_some() {
        let id = test_identity();
        let dir = tempfile::tempdir().unwrap();
        generate_chart(&test_contract(), dir.path(), Some(&id)).unwrap();
        let chart = std::fs::read_to_string(dir.path().join("Chart.yaml")).unwrap();
        // Top-level annotations block present.
        assert!(chart.contains("\nannotations:\n"));
        assert!(chart.contains("io.hyperi.contract.version: \"v1\""));
        assert!(chart.contains(
            "io.hyperi.contract.source-commit: \"0123456789abcdef0123456789abcdef01234567\""
        ));
        assert!(
            chart.contains("io.hyperi.contract.image-ref: \"ghcr.io/hyperi-io/dfe-loader:v2.7.2\"")
        );
    }

    #[test]
    fn argocd_application_omits_identity_block_when_none() {
        let argo = ArgocdConfig::default();
        let yaml = generate_argocd_application(&test_contract(), &argo, None);
        assert!(!yaml.contains("io.hyperi.contract"));
        // sync-wave is unaffected.
        assert!(yaml.contains("argocd.argoproj.io/sync-wave:"));
    }

    #[test]
    fn argocd_application_emits_three_identity_annotations_when_some() {
        let id = test_identity();
        let argo = ArgocdConfig::default();
        let yaml = generate_argocd_application(&test_contract(), &argo, Some(&id));
        // Both the existing sync-wave AND the three identity keys must appear
        // under the same metadata.annotations block.
        assert!(yaml.contains("argocd.argoproj.io/sync-wave:"));
        assert!(yaml.contains("io.hyperi.contract.version: \"v1\""));
        assert!(yaml.contains(
            "io.hyperi.contract.source-commit: \"0123456789abcdef0123456789abcdef01234567\""
        ));
        assert!(
            yaml.contains("io.hyperi.contract.image-ref: \"ghcr.io/hyperi-io/dfe-loader:v2.7.2\"")
        );
    }

    #[test]
    fn all_three_surfaces_share_the_same_key_prefix() {
        let id = test_identity();
        let argo = ArgocdConfig::default();
        let dir = tempfile::tempdir().unwrap();

        let dockerfile = generate_dockerfile(&test_contract(), Some(&id));
        generate_chart(&test_contract(), dir.path(), Some(&id)).unwrap();
        let chart = std::fs::read_to_string(dir.path().join("Chart.yaml")).unwrap();
        let app = generate_argocd_application(&test_contract(), &argo, Some(&id));

        // The documented grep payoff: every surface mentions the prefix
        // exactly three times (once per key).
        assert_eq!(dockerfile.matches("io.hyperi.contract").count(), 3);
        assert_eq!(chart.matches("io.hyperi.contract").count(), 3);
        assert_eq!(app.matches("io.hyperi.contract").count(), 3);
    }
}
