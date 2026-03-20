# [1.18.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0...v1.18.0) (2026-03-20)


### Bug Fixes

* add create_topics and delete_topics to KafkaAdmin ([21eb0c1](https://github.com/hyperi-io/hyperi-rustlib/commit/21eb0c190c502fe227975b490edbbaae0daad442))
* add feature gates to gRPC integration tests ([b9bc063](https://github.com/hyperi-io/hyperi-rustlib/commit/b9bc06374c92876f2790bfb47c765e6b00eb701c))
* add start_server_with_routes, scaling/memory endpoints to MetricsManager [skip ci] ([b229749](https://github.com/hyperi-io/hyperi-rustlib/commit/b229749b2cacb2ee1360b035ab83c6b39c3bfb2d))
* auto-emit config reload and rdkafka Prometheus metrics ([9aa2893](https://github.com/hyperi-io/hyperi-rustlib/commit/9aa2893938662d55aea0a983269760aeb29102a7))
* bump lz4_flex upper bound, update deps, fix clippy lints ([a31e07f](https://github.com/hyperi-io/hyperi-rustlib/commit/a31e07f959b304b4de021c9346358cda8589ec82))
* bump lz4_flex upper bound, update deps, fix clippy lints ([f7c04f1](https://github.com/hyperi-io/hyperi-rustlib/commit/f7c04f130dd385c49c930355844360672c0334f8))


### Features

* add metrics-dfe feature with composable metric groups ([e26c2dd](https://github.com/hyperi-io/hyperi-rustlib/commit/e26c2dd99fd3947c4999431ac4855522753c5ccf))

# [1.17.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.8...v1.17.0) (2026-03-20)


### Bug Fixes

* add feature gates to gRPC integration tests ([47cf733](https://github.com/hyperi-io/hyperi-rustlib/commit/47cf733fbac1fae26296421073726b9a044512ae))
* auto-emit config reload and rdkafka Prometheus metrics ([2696cb8](https://github.com/hyperi-io/hyperi-rustlib/commit/2696cb86553d6e0fc0253b7bd8d032388b730ac7))


### Features

* add metrics-dfe feature with composable metric groups ([5727737](https://github.com/hyperi-io/hyperi-rustlib/commit/5727737d804aeb5c78222547e43a822736cbfecd))

## [1.16.8](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.7...v1.16.8) (2026-03-20)


### Bug Fixes

* add start_server_with_routes, scaling/memory endpoints to MetricsManager [skip ci] ([11fe149](https://github.com/hyperi-io/hyperi-rustlib/commit/11fe14942216c4179904d28ea2009c71faed78d9))

## [1.16.7](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.6...v1.16.7) (2026-03-20)


### Bug Fixes

* add create_topics and delete_topics to KafkaAdmin ([53f2ced](https://github.com/hyperi-io/hyperi-rustlib/commit/53f2ced1baad9ef5b7e836ef17c6814b1da53003))

## [1.16.6](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.5...v1.16.6) (2026-03-20)


### Bug Fixes

* add readiness callback to MetricsManager health endpoints ([500ccf3](https://github.com/hyperi-io/hyperi-rustlib/commit/500ccf3f7d8eee6eca741e75b186cc290efc063d))

## [1.16.5](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.4...v1.16.5) (2026-03-19)


### Bug Fixes

* add DfeSource convention for topic naming and consumer groups ([3b0c7da](https://github.com/hyperi-io/hyperi-rustlib/commit/3b0c7daceb9aa091093bad6b9f05f3366757dc0c))
* add from_env/from_env_raw to MemoryGuardConfig, tune defaults ([3c59845](https://github.com/hyperi-io/hyperi-rustlib/commit/3c5984563857f494cbbac505184756ec71d6ce10))
* add readiness callback to MetricsManager health endpoints ([500ccf3](https://github.com/hyperi-io/hyperi-rustlib/commit/500ccf3))

## [1.16.4](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.3...v1.16.4) (2026-03-19)


### Bug Fixes

* add DfeSource convention for topic naming and consumer groups ([3b0c7da](https://github.com/hyperi-io/hyperi-rustlib/commit/3b0c7da))
* add from_env/from_env_raw to MemoryGuardConfig, tune defaults ([3c59845](https://github.com/hyperi-io/hyperi-rustlib/commit/3c59845))
* add concurrent and edge-case tests for MemoryGuard ([b411d1c](https://github.com/hyperi-io/hyperi-rustlib/commit/b411d1c))
* add MemoryGuard — cgroup-aware memory backpressure for OOM prevention ([fba690f](https://github.com/hyperi-io/hyperi-rustlib/commit/fba690f))

## [1.16.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2...v1.16.3) (2026-03-19)


### Bug Fixes

* add concurrent and edge-case tests for MemoryGuard ([b411d1c](https://github.com/hyperi-io/hyperi-rustlib/commit/b411d1c))
* add data quality event helpers (DLQ routing, quality alerts) ([2a6eeb6](https://github.com/hyperi-io/hyperi-rustlib/commit/2a6eeb67109c4fbb59eb77f22622100dc64f3091))
* add DfeMetrics standard metric set with transport labels ([d66d9b3](https://github.com/hyperi-io/hyperi-rustlib/commit/d66d9b3ea9b490ef7057a39c72e4d60a4fd724c6))
* add flat env override helpers, ApplyFlatEnv and Normalize traits ([e583ecf](https://github.com/hyperi-io/hyperi-rustlib/commit/e583ecf650e0b1d8142e6f63a7fd233c88071192))
* add libc dependency for disk-aware capacity management ([7e47351](https://github.com/hyperi-io/hyperi-rustlib/commit/7e47351b1fe3bda50075342fcef8fae91bf97ddf))
* add log spam helper functions (state, sampled, debounced) ([f38296d](https://github.com/hyperi-io/hyperi-rustlib/commit/f38296d6530829422c055f70c15c0f7f016718eb))
* add MemoryGuard — cgroup-aware memory backpressure for OOM prevention ([fba690f](https://github.com/hyperi-io/hyperi-rustlib/commit/fba690f))
* add security event logging framework (OWASP-aligned) ([f08819b](https://github.com/hyperi-io/hyperi-rustlib/commit/f08819b18b01924bceee7f6e30460cbc78366eb0))
* add tracing-throttle layer to logger (opt-in via LOG_THROTTLE_ENABLED) ([842914a](https://github.com/hyperi-io/hyperi-rustlib/commit/842914a34de03a3d84d51dcdf21784474242dcc9))
* address clippy warnings in log helpers (is_multiple_of, cast) ([cc73e5d](https://github.com/hyperi-io/hyperi-rustlib/commit/cc73e5dd936545ef57f9c7335d8986c1ee58fd3d))
* enforce max_spool_bytes limit in TieredSink ([9f9103f](https://github.com/hyperi-io/hyperi-rustlib/commit/9f9103f378df10b33c7321298ca6cc663a102030))
* initialise spool len counter from existing queue on open ([a2c9a65](https://github.com/hyperi-io/hyperi-rustlib/commit/a2c9a65acea8368429173ed7ae64b5a0c05668e5))
* inject service name and version into JSON log output ([e3c70ef](https://github.com/hyperi-io/hyperi-rustlib/commit/e3c70eff1a76ae31e667cb4da6fc9790c12f515e))
* make spool codec configurable, default to zstd level 1 ([90c1c4a](https://github.com/hyperi-io/hyperi-rustlib/commit/90c1c4a212ddc8aa87f12ccbd6b4635ac36ce1f6))

## [1.16.2-dev.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.1...v1.16.2-dev.1) (2026-03-18)


### Bug Fixes

* add DfeMetrics standard metric set with transport labels ([d66d9b3](https://github.com/hyperi-io/hyperi-rustlib/commit/d66d9b3ea9b490ef7057a39c72e4d60a4fd724c6))
* add libc dependency for disk-aware capacity management ([7e47351](https://github.com/hyperi-io/hyperi-rustlib/commit/7e47351b1fe3bda50075342fcef8fae91bf97ddf))
* add log spam helper functions (state, sampled, debounced) ([f38296d](https://github.com/hyperi-io/hyperi-rustlib/commit/f38296d6530829422c055f70c15c0f7f016718eb))
* add tracing-throttle layer to logger (opt-in via LOG_THROTTLE_ENABLED) ([842914a](https://github.com/hyperi-io/hyperi-rustlib/commit/842914a34de03a3d84d51dcdf21784474242dcc9))
* address clippy warnings in log helpers (is_multiple_of, cast) ([cc73e5d](https://github.com/hyperi-io/hyperi-rustlib/commit/cc73e5dd936545ef57f9c7335d8986c1ee58fd3d))
* correct Dockerfile profile assertions in deployment tests ([2cfee48](https://github.com/hyperi-io/hyperi-rustlib/commit/2cfee484a30af9b557da456472db9880100b23e2))
* Dockerfile generator header and Ubuntu 24.04 UID fix ([d8c3c69](https://github.com/hyperi-io/hyperi-rustlib/commit/d8c3c698083c9478d23b4887205f25af526df6e5))
* downgrade rdkafka INFO/Notice logs to debug level ([c58c17f](https://github.com/hyperi-io/hyperi-rustlib/commit/c58c17fa73e2b6290815dcbd9f14e1ff5946a911)), closes [hyperi-io/dfe-loader#5](https://github.com/hyperi-io/dfe-loader/issues/5)
* enforce max_spool_bytes limit in TieredSink ([9f9103f](https://github.com/hyperi-io/hyperi-rustlib/commit/9f9103f378df10b33c7321298ca6cc663a102030))
* initialise spool len counter from existing queue on open ([a2c9a65](https://github.com/hyperi-io/hyperi-rustlib/commit/a2c9a65acea8368429173ed7ae64b5a0c05668e5))
* make spool codec configurable, default to zstd level 1 ([90c1c4a](https://github.com/hyperi-io/hyperi-rustlib/commit/90c1c4a212ddc8aa87f12ccbd6b4635ac36ce1f6))

## [1.16.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.0...v1.16.1) (2026-03-12)


### Bug Fixes

* update crates.io keywords for discoverability ([ad924cd](https://github.com/hyperi-io/hyperi-rustlib/commit/ad924cd9abc8391c7595886622a4f797f966726b))

# [1.16.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.15.0...v1.16.0) (2026-03-11)


### Features

* add ImageProfile for production vs development container images ([870a52c](https://github.com/hyperi-io/hyperi-rustlib/commit/870a52c3444616d5ad8b4d3e34aa343a8824b3b9))

# [1.15.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.14.0...v1.15.0) (2026-03-11)


### Bug Fixes

* resolve clippy bool_comparison in native_deps test ([8bc941d](https://github.com/hyperi-io/hyperi-rustlib/commit/8bc941de300a042f5683023706eabac0e2622849))


### Features

* auto-generate native deps in Dockerfile from feature flags ([9e86810](https://github.com/hyperi-io/hyperi-rustlib/commit/9e86810e741e01254f3cb7a1436d5ab88db0bada))

# [1.14.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.13.2...v1.14.0) (2026-03-11)


### Features

* dynamic-link C deps, bump versions, drop cmake builds ([926d2fe](https://github.com/hyperi-io/hyperi-rustlib/commit/926d2fee601ba49234e4e490cb1df671f97f72ee))

## [1.13.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.13.1...v1.13.2) (2026-03-10)


### Bug Fixes

* add benches, scripts to cargo publish exclude list [skip ci] ([e305716](https://github.com/hyperi-io/hyperi-rustlib/commit/e3057161c90fb45fc70d1032a17469c4a0a0f310))
* add kafka_config module with config_from_file and file-based overrides [skip ci] ([1de187a](https://github.com/hyperi-io/hyperi-rustlib/commit/1de187a67b1414a1c16eaf94383006a546bd81cc))
* modernise to Rust edition 2024 and drop async-trait from traits [skip ci] ([aac49af](https://github.com/hyperi-io/hyperi-rustlib/commit/aac49af0958e44a051b8d7ec4dbfb84967430d08))
* quote on key in workflow to prevent YAML boolean parse ([02e38f8](https://github.com/hyperi-io/hyperi-rustlib/commit/02e38f8285c6cde6778ac1ef4e0bf53b60aa92c2))
* remove explicit ref mut in match ergonomics for Rust stable ([7781de7](https://github.com/hyperi-io/hyperi-rustlib/commit/7781de749c4474e5fcb4844c037f72b93beac180))
* resolve Rust 2024 edition clippy and fmt errors ([fd8b2ab](https://github.com/hyperi-io/hyperi-rustlib/commit/fd8b2abfc649f8b7eeda07c6c07ea14e302f210a))
* sort criterion imports to match rustfmt ordering ([8c0300d](https://github.com/hyperi-io/hyperi-rustlib/commit/8c0300d9bf860581afb1aa987ae2d3eafc055add))
* use secrets inherit and add permissions block ([513d180](https://github.com/hyperi-io/hyperi-rustlib/commit/513d18019755b26362a7589db02e8190c878d565))

## [1.13.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.13.0...v1.13.1) (2026-03-03)


### Bug Fixes

* use configurable base_image in deployment contract instead of hardcoded debian ([a4d659e](https://github.com/hyperi-io/hyperi-rustlib/commit/a4d659e96b2375405a106d3083fb142c2a7b6a23))

# [1.13.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.12.3...v1.13.0) (2026-03-03)


### Bug Fixes

* cache instance_id with OnceLock to fix race condition in tests ([3981a65](https://github.com/hyperi-io/hyperi-rustlib/commit/3981a6546e4a6354fc7e63ecd076af50de0db2a8))


### Features

* add CEL expression evaluation module ([83713e6](https://github.com/hyperi-io/hyperi-rustlib/commit/83713e6bfcfc6f07823a83b91d30bc33dafe2ce7))

## [1.12.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.12.2...v1.12.3) (2026-03-03)


### Bug Fixes

* gate semantic-release on CI success ([72f20a3](https://github.com/hyperi-io/hyperi-rustlib/commit/72f20a349f322be311f67edbc332485abbab2151))

## [1.12.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.12.1...v1.12.2) (2026-03-03)


### Bug Fixes

* resolve remaining clippy pedantic and typos errors ([a399cb2](https://github.com/hyperi-io/hyperi-rustlib/commit/a399cb2a994c1015cc12bda92b7709668713c979))

## [1.12.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.12.0...v1.12.1) (2026-03-03)


### Bug Fixes

* resolve clippy uninlined_format_args warnings ([e0df71b](https://github.com/hyperi-io/hyperi-rustlib/commit/e0df71b253c191db7dd79f5e7fdbaa0e52d1289e))

# [1.12.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.11.0...v1.12.0) (2026-03-03)


### Features

* add shared io module and file output sink ([be188c6](https://github.com/hyperi-io/hyperi-rustlib/commit/be188c6d627998e67708ecaac548f04b9a11124d))
* add standard CLI framework and TUI metrics dashboard ([dc9e909](https://github.com/hyperi-io/hyperi-rustlib/commit/dc9e9095efc1a57234beaeab10d59c7b47ebdde4))

# [1.11.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.10.0...v1.11.0) (2026-03-03)


### Bug Fixes

* correct Helm template brace escaping in generate.rs ([c0bc194](https://github.com/hyperi-io/hyperi-rustlib/commit/c0bc1946e7b400b61f0f0efe2570cfab6a964a25))


### Features

* add deployment artifact generation (Dockerfile, Helm chart, Compose) ([f7d20a2](https://github.com/hyperi-io/hyperi-rustlib/commit/f7d20a2ccba91d61d5c4bf057667f69929857c3c))

# [1.10.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.9.2...v1.10.0) (2026-03-02)


### Bug Fixes

* serialise env_compat unit tests to prevent parallel env var races ([b603991](https://github.com/hyperi-io/hyperi-rustlib/commit/b603991623b335f68966efc7bb4fe53db837bc93))


### Features

* add unified DLQ module with file and Kafka backends ([f901918](https://github.com/hyperi-io/hyperi-rustlib/commit/f9019186cde6e7815108b0574ee3eb86c72e4040))

## [1.9.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.9.1...v1.9.2) (2026-03-02)


### Bug Fixes

* update ci submodule with mawk compat fix for cargo version bump ([7ced55d](https://github.com/hyperi-io/hyperi-rustlib/commit/7ced55dca58a583bdcb7bbfd2b04fdfd6e97edd7))

## [1.9.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.9.0...v1.9.1) (2026-03-02)


### Bug Fixes

* correct Cargo.toml version (CI version bump failed for v1.9.0) ([9f5ad2d](https://github.com/hyperi-io/hyperi-rustlib/commit/9f5ad2d243168f21fd685eba17581e3dc4cb8762))

# [1.9.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.9...v1.9.0) (2026-03-02)


### Features

* add scaling pressure module for KEDA autoscaling ([031e210](https://github.com/hyperi-io/hyperi-rustlib/commit/031e210a95e0f0d04e972b7b9c975b705aab1794))

## [1.8.9](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.8...v1.8.9) (2026-03-02)


### Bug Fixes

* rustfmt and dprint formatting alignment ([8e86117](https://github.com/hyperi-io/hyperi-rustlib/commit/8e8611773e968a0a916580253b7f0bbcac8ab75f))

## [1.8.8](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.7...v1.8.8) (2026-03-02)


### Bug Fixes

* clippy match_wild_err_arm in vector compat tests ([7faa406](https://github.com/hyperi-io/hyperi-rustlib/commit/7faa4063984f8dd76a979bc8e8ae64bc9cdb37a9))

## [1.8.7](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.6...v1.8.7) (2026-03-02)


### Bug Fixes

* clippy manual_let_else in vector compat tests ([56d7ecc](https://github.com/hyperi-io/hyperi-rustlib/commit/56d7ecce080d0210caa20a65038d4e30352ed119))

## [1.8.6](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.5...v1.8.6) (2026-03-02)


### Bug Fixes

* clippy single_match_else and match_same_arms ([5782670](https://github.com/hyperi-io/hyperi-rustlib/commit/578267080954520c34a9b0b4fd322d37481f3b21))

## [1.8.5](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.4...v1.8.5) (2026-03-02)


### Bug Fixes

* masking layer, coloured output, integration tests ([5817bc0](https://github.com/hyperi-io/hyperi-rustlib/commit/5817bc0c526473440f22324bf19408f836b9eb9b))
* vector compat integration tests, vault_env env leak fix ([ce0294a](https://github.com/hyperi-io/hyperi-rustlib/commit/ce0294ad55ed86ca53db6e2a161a045e16258358))

## [1.8.4](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.3...v1.8.4) (2026-03-02)


### Bug Fixes

* update deps to latest versions, migrate tonic/prost to 0.14 ([c11aca0](https://github.com/hyperi-io/hyperi-rustlib/commit/c11aca05526c52ca8219e8d2167e64a921c3e93a))

## [1.8.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.2...v1.8.3) (2026-03-02)


### Bug Fixes

* dprint formatting for float match arm ([5913655](https://github.com/hyperi-io/hyperi-rustlib/commit/591365547a0493a9073bd68ac235714b610e8fa4))

## [1.8.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.1...v1.8.2) (2026-03-02)


### Bug Fixes

* use non-constant float in clippy test ([2cefd58](https://github.com/hyperi-io/hyperi-rustlib/commit/2cefd5834c31b44d152cd08e5720320ba264c0e4))

## [1.8.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.8.0...v1.8.1) (2026-03-02)


### Bug Fixes

* clippy approx_constant lint and standardise release runner ([a1a3258](https://github.com/hyperi-io/hyperi-rustlib/commit/a1a3258d8504a681e254ef645c64c2f9b5891701))

# [1.8.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.7.2...v1.8.0) (2026-03-01)


### Features

* add gRPC transport with Vector wire protocol compatibility ([bb7985e](https://github.com/hyperi-io/hyperi-rustlib/commit/bb7985e5db132b9288973728293557c60d9fc477))

## [1.7.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.7.1...v1.7.2) (2026-02-28)


### Bug Fixes

* remove arc runner config, use github-hosted runners ([d161450](https://github.com/hyperi-io/hyperi-rustlib/commit/d16145037a25d795870f2e22c2d29e7baa25c4af))

## [1.7.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.7.0...v1.7.1) (2026-02-28)


### Bug Fixes

* clippy and fmt issues in version_check module ([ef5375e](https://github.com/hyperi-io/hyperi-rustlib/commit/ef5375e6d7c397c35a1db8059ffaf2dcba29c339))

# [1.7.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.6.0...v1.7.0) (2026-02-28)


### Features

* deployment contract validation and startup version check ([822adba](https://github.com/hyperi-io/hyperi-rustlib/commit/822adbae940434c5e6de3561713860929c2ee11d))

# [1.6.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.5.4...v1.6.0) (2026-02-28)


### Features

* align config cascade with hyperi-pylib unified spec ([398034c](https://github.com/hyperi-io/hyperi-rustlib/commit/398034c0a114e51c776841d06b2aa9e6b3a7ac93))

## [1.5.4](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.5.3...v1.5.4) (2026-02-25)


### Bug Fixes

* remove target-cpu=native to fix SIGILL in CI ([cd5df48](https://github.com/hyperi-io/hyperi-rustlib/commit/cd5df4860ba1373a2b30c18ab50c4d711b499709))

## [1.5.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.5.2...v1.5.3) (2026-02-25)


### Bug Fixes

* add markdownlint-cli2 ignore config for generated/vendored files ([7fd0840](https://github.com/hyperi-io/hyperi-rustlib/commit/7fd0840e5a2d79c6b477ed3d87b5ef291f4296a2))

## [1.5.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.5.1...v1.5.2) (2026-02-25)


### Bug Fixes

* resolve markdownlint errors in docs and CONTRIBUTING ([fc80dcc](https://github.com/hyperi-io/hyperi-rustlib/commit/fc80dccdfbc802b9187622d3db3cf96bedfde35a))

## [1.5.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.5.0...v1.5.1) (2026-02-25)


### Bug Fixes

* resolve CI quality failures (typos, clippy, fmt, gitleaks, test) ([9c51c94](https://github.com/hyperi-io/hyperi-rustlib/commit/9c51c94943ee5cc7ee636722e731908d0daa98cd))

## [1.4.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.4.2...v1.4.3) (2026-02-17)


### Bug Fixes

* rename package from hs-rustlib to hyperi-rustlib ([bbb797a](https://github.com/hyperi-io/hyperi-rustlib/commit/bbb797a2b2351eb241fb79a7cd3b26e2ba9a08b7))

## [1.4.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.4.1...v1.4.2) (2026-02-16)


### Bug Fixes

* update cargo publish registry from hypersec to hyperi ([62c6393](https://github.com/hyperi-io/hyperi-rustlib/commit/62c63939bd53eb881197bdbe18b311c6b952a785))

## [1.4.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.4.0...v1.4.1) (2026-02-16)


### Bug Fixes

* add default spread to MetricsConfig in full_demo example ([d1eb919](https://github.com/hyperi-io/hyperi-rustlib/commit/d1eb919b7826365923c7487d2ed3f7d8c00688fd))

# [1.4.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.3.12...v1.4.0) (2026-02-16)


### Features

* add directory-config store, otel-metrics, and git2 integration ([a4f3938](https://github.com/hyperi-io/hyperi-rustlib/commit/a4f3938549d2b577920f6ae68eb2d27049e57801))

## [1.3.12](https://github.com/hypersec-io/hyperi-rustlib/compare/v1.3.11...v1.3.12) (2026-02-06)


### Bug Fixes

* apply cargo fmt formatting ([8f70877](https://github.com/hypersec-io/hyperi-rustlib/commit/8f70877cec8c66f2f8adef87e81fc98a64099d8b))

## [1.3.11](https://github.com/hypersec-io/hyperi-rustlib/compare/v1.3.10...v1.3.11) (2026-02-06)


### Bug Fixes

* resolve clippy single_char_pattern and uninlined_format_args lints ([028e4d6](https://github.com/hypersec-io/hyperi-rustlib/commit/028e4d6c2581fe5d00a3e6121d56f5bd7c4fd0f7))

## [1.3.10](https://github.com/hypersec-io/hyperi-rustlib/compare/v1.3.9...v1.3.10) (2026-02-06)


### Bug Fixes

* resolve clippy float_cmp and case_sensitive_extension lints ([c4d538b](https://github.com/hypersec-io/hyperi-rustlib/commit/c4d538b82863168465fbe74d02feb164650e294b))

## [1.3.9](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.8...v1.3.9) (2026-02-03)


### Bug Fixes

* **tests:** change CWD to empty dir for hardcoded defaults test ([70cd34a](https://github.com/hypersec-io/hs-rustlib/commit/70cd34a3ee6ad332620bed5dbb07da804cde6a4f))

## [1.3.8](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.7...v1.3.8) (2026-02-03)


### Bug Fixes

* **docs:** add missing SecretSource import in secrets doctest ([aa0f293](https://github.com/hypersec-io/hs-rustlib/commit/aa0f2936a6ecdc1582e41bece638ff8a61d6b543))

## [1.3.7](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.6...v1.3.7) (2026-02-03)


### Bug Fixes

* **tests:** isolate hardcoded defaults test from CWD config files ([489160b](https://github.com/hypersec-io/hs-rustlib/commit/489160b946a54bbe778def97bdb9f388ff4e0e68))

## [1.3.6](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.5...v1.3.6) (2026-02-03)


### Performance Improvements

* **ci:** remove CARGO_BUILD_JOBS limit on release runner ([a2fcd4a](https://github.com/hypersec-io/hs-rustlib/commit/a2fcd4a481eeb4a391b0998d2ab6789da968252d))

## [1.3.5](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.4...v1.3.5) (2026-02-03)


### Bug Fixes

* **test:** ensure VAULT_ADDR cleared in openbao fallback test ([64bfc49](https://github.com/hypersec-io/hs-rustlib/commit/64bfc49bf1e4bd9a018a2b168dc40c2ffc58595f))

## [1.3.4](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.3...v1.3.4) (2026-02-03)


### Bug Fixes

* **ci:** use buildjet-32vcpu runner for release ([e9077e0](https://github.com/hypersec-io/hs-rustlib/commit/e9077e09eaa17a0814b9587bed10dfb7a174350e))

## [1.3.3](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.2...v1.3.3) (2026-02-03)


### Bug Fixes

* **ci:** use org default runner for release workflow ([c65e4a2](https://github.com/hypersec-io/hs-rustlib/commit/c65e4a29c463d337f2fc91879a47784be79f8906))

## [1.3.2](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.1...v1.3.2) (2026-02-03)


### Bug Fixes

* **ci:** use system librdkafka instead of cmake-build ([2fc7e3c](https://github.com/hypersec-io/hs-rustlib/commit/2fc7e3c27eb72edf4b20aa460b216b53310f72fc))

## [1.3.1](https://github.com/hypersec-io/hs-rustlib/compare/v1.3.0...v1.3.1) (2026-02-03)


### Bug Fixes

* **ci:** add system dependencies for rdkafka build ([96afaf2](https://github.com/hypersec-io/hs-rustlib/commit/96afaf223324e4df3cc501736b8e017508c9ccb0))

## [1.2.2](https://github.com/hypersec-io/hs-rustlib/compare/v1.2.1...v1.2.2) (2026-01-20)


### Bug Fixes

* resolve additional clippy lints for CI --all-targets ([3161911](https://github.com/hypersec-io/hs-rustlib/commit/316191110b30d9ccd66da350b76578e8f5983990))

## [1.2.1](https://github.com/hypersec-io/hs-rustlib/compare/v1.2.0...v1.2.1) (2026-01-20)


### Bug Fixes

* resolve stricter clippy lints from Polars-inspired CI ([a41882e](https://github.com/hypersec-io/hs-rustlib/commit/a41882e953ae65e9e676bd45ac6e371e351b3c8c))

# [1.2.0](https://github.com/hypersec-io/hs-rustlib/compare/v1.1.0...v1.2.0) (2026-01-20)


### Bug Fixes

* **config:** set language to rust ([64ce052](https://github.com/hypersec-io/hs-rustlib/commit/64ce0526a8714c65b0604f15720eae1bdeba5c99))


### Features

* **ci:** add Polars-inspired Rust CI workflow ([0df5cc5](https://github.com/hypersec-io/hs-rustlib/commit/0df5cc51f5b033e651e8f0796c0bebc442bba315))

# [1.1.0](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.8...v1.1.0) (2026-01-20)


### Bug Fixes

* add async-trait to transport feature dependencies ([0532aa4](https://github.com/hypersec-io/hs-rustlib/commit/0532aa44a2d97cbb84bba8d8d358bd1255f7fd85))


### Features

* add license module, remove clickhouse-arrow wrapper ([82938c7](https://github.com/hypersec-io/hs-rustlib/commit/82938c7508e63256e2d07ea6a600f4b74760d683))

## [1.0.8](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.7...v1.0.8) (2026-01-19)


### Bug Fixes

* exclude non-essential directories from cargo package ([dce89bf](https://github.com/hypersec-io/hs-rustlib/commit/dce89bf34b01246c2b064750612d9033661866a1))

## [1.0.7](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.6...v1.0.7) (2026-01-19)


### Bug Fixes

* clippy lints for CI compatibility ([9c25671](https://github.com/hypersec-io/hs-rustlib/commit/9c256718c97f94d81d73bb6faefd81833772d751))

## [1.0.6](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.5...v1.0.6) (2026-01-19)


### Bug Fixes

* correct module path in clickhouse_arrow doc tests ([aee6944](https://github.com/hypersec-io/hs-rustlib/commit/aee69441a4aa6838c68f594da43d392dfd8cdf78))

## [1.0.5](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.4...v1.0.5) (2026-01-19)


### Bug Fixes

* **ci:** use correct secret name JFROG_ACCESS_TOKEN ([91270aa](https://github.com/hypersec-io/hs-rustlib/commit/91270aa9d75c92e6756b769e8d414c40c867f0c3))

## [1.0.4](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.3...v1.0.4) (2026-01-19)


### Bug Fixes

* **ci:** use env var for cargo registry token ([82218e9](https://github.com/hypersec-io/hs-rustlib/commit/82218e9d1c2e5dc7d115fdcccd89fc67332eeff9))

## [1.0.3](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.2...v1.0.3) (2026-01-19)


### Bug Fixes

* **ci:** add global-credential-providers for cargo auth ([9ae9ea0](https://github.com/hypersec-io/hs-rustlib/commit/9ae9ea00620b29115153ce142fc6bf3bb89d81c4))

## [1.0.2](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.1...v1.0.2) (2026-01-19)


### Bug Fixes

* **ci:** use echo instead of heredocs for cargo config ([7f82a30](https://github.com/hypersec-io/hs-rustlib/commit/7f82a306c5e80c7743090432e85eb8ed809be586))

## [1.0.1](https://github.com/hypersec-io/hs-rustlib/compare/v1.0.0...v1.0.1) (2026-01-18)


### Bug Fixes

* **ci:** configure Artifactory registry before build step ([cd645fc](https://github.com/hypersec-io/hs-rustlib/commit/cd645fcb16cf8d60ca8431fde65832fb804e8f16))

# 1.0.0 (2026-01-18)


### Features

* add transport abstraction layer (Kafka/Zenoh/Memory) ([8156e33](https://github.com/hypersec-io/hs-rustlib/commit/8156e334479675354bb58074b745be3db791881c))
* **clickhouse:** add ClickHouse client module ([098c346](https://github.com/hypersec-io/hs-rustlib/commit/098c34605f26812f4e0c17ec5615a3975960b993))
* initial MVP of hs-rustlib shared library ([40c3bcd](https://github.com/hypersec-io/hs-rustlib/commit/40c3bcddb5585a2aa4f73bd2339e2a39de96cf9c))
* **transport:** add stateful FormatDetector with auto-locking ([bbf4007](https://github.com/hypersec-io/hs-rustlib/commit/bbf40073a9804cb3d11232331f123aacb53521cf))
