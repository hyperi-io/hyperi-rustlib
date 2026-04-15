## [2.5.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.5.0...v2.5.1) (2026-04-15)


### Bug Fixes

* add packages:write permission for container job ([108091f](https://github.com/hyperi-io/hyperi-rustlib/commit/108091f04a5ea2a47f8ce73f2a185d38769d1d37))
* clarify header comment wording ([208fd02](https://github.com/hyperi-io/hyperi-rustlib/commit/208fd02af6e578c66acf783e8e23a369b15d6c6a))
* revert header comment to original wording ([43b6031](https://github.com/hyperi-io/hyperi-rustlib/commit/43b60314affe47e1a6f7a23a8d970e464a55c41c))
* use em-dash in doc comment for consistency ([75308d7](https://github.com/hyperi-io/hyperi-rustlib/commit/75308d70b6c95e6e491f3386866b18b2c5f2b48c))

# [2.5.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.6...v2.5.0) (2026-04-11)


### Features

* **filter:** FilteredBatch + take_filtered_dlq_entries() trait method ([4fe1d1c](https://github.com/hyperi-io/hyperi-rustlib/commit/4fe1d1c080ccd7240cb17e331f7851c8283e302a))
* **filter:** transport filter engine core — config, classification, SIMD evaluation ([be3351c](https://github.com/hyperi-io/hyperi-rustlib/commit/be3351c682876b45bc4a6bd6f9aacd8c45e6c686))
* **filter:** wire TransportFilterEngine into all 7 transports ([dc75d25](https://github.com/hyperi-io/hyperi-rustlib/commit/dc75d2580671c3809ff9ee62834142fa0ec76129))
* transport filter engine — design spec complete ([0632854](https://github.com/hyperi-io/hyperi-rustlib/commit/06328543845164ba54fc94a5b14e545a0f49f8cd))


### Performance Improvements

* **filter:** SIMD memmem fast-path for has() — 51% faster, Tier 2/3 CEL ([813da63](https://github.com/hyperi-io/hyperi-rustlib/commit/813da63788e818fad105a326d96716a9ef74655e))

## [2.4.6](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.5...v2.4.6) (2026-04-09)


### Bug Fixes

* make CEL build_context generic over any (&String, &Value) iterator ([7fe0652](https://github.com/hyperi-io/hyperi-rustlib/commit/7fe06526a4c7e5d1acc8f9e3c75ced1a72d441c8))

## [2.4.5](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.4...v2.4.5) (2026-04-09)


### Bug Fixes

* address PR review — clippy, parking_lot, subscribed_topics update ([d79a5a8](https://github.com/hyperi-io/hyperi-rustlib/commit/d79a5a81530c6a750bff8c13040fea01f7247c9a))
* wire kafka topic refresh and make auto-discovery opt-in ([#32](https://github.com/hyperi-io/hyperi-rustlib/issues/32)) ([90791d0](https://github.com/hyperi-io/hyperi-rustlib/commit/90791d02f1d62fd5540f959deccb8553984cb67c))
* wire up kafka topic refresh loop and make auto-discovery opt-in ([3e5305e](https://github.com/hyperi-io/hyperi-rustlib/commit/3e5305e8ba3f1e8a82dc641acd895e2d60f3d3f9))

## [2.4.4](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.3...v2.4.4) (2026-04-02)


### Bug Fixes

* add async sink engine API, fix doc-tests — Phase 2A ([45c307a](https://github.com/hyperi-io/hyperi-rustlib/commit/45c307a44c15ac114822bc6eb0bf70028f7d5904))
* clippy — use flatten() instead of if-let in test loop ([82a50b3](https://github.com/hyperi-io/hyperi-rustlib/commit/82a50b33b51d3ef7acf8376f40fea5655507e20f))

## [2.4.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.2...v2.4.3) (2026-04-02)


### Bug Fixes

* add adversarial worker pool tests — panic recovery, stress, boundaries ([e0e8448](https://github.com/hyperi-io/hyperi-rustlib/commit/e0e8448670554905e42e40de3a87acd5691d3e65))
* add auto_wire and ServiceRuntime integration for BatchEngine ([ea60109](https://github.com/hyperi-io/hyperi-rustlib/commit/ea60109e135c7cf035e543fbc9c0c81d241561f6))
* add BatchAccumulator for bounded batch drain with time/count/bytes thresholds ([311af18](https://github.com/hyperi-io/hyperi-rustlib/commit/311af186d059251db4e5b48c9e405cdf9d91cdb1))
* add BatchEngine adversarial tests — edge cases, boundaries, stress ([b172e1b](https://github.com/hyperi-io/hyperi-rustlib/commit/b172e1bdc4a9c813273363be1b7c305afd731c10))
* add BatchEngine core types — RawMessage, ParsedMessage, accessors ([d43349e](https://github.com/hyperi-io/hyperi-rustlib/commit/d43349ede27bcf8b50af660d8d6837baa3398764))
* add BatchEngine criterion benchmarks ([d299d89](https://github.com/hyperi-io/hyperi-rustlib/commit/d299d89d3082252d22a94cdbb00494b7f90fc121))
* add BatchEngine metrics — auto-registered via MetricsManager ([5d53d9a](https://github.com/hyperi-io/hyperi-rustlib/commit/5d53d9ad97758a2f96e4bbafc8671d3a1fb56549))
* add BatchEngine with process_mid_tier and process_raw ([de19ed3](https://github.com/hyperi-io/hyperi-rustlib/commit/de19ed3d6772f785132e0aaabe9ecbeaf88a4e07))
* add BatchProcessingConfig with cascade support and DFE defaults ([6336449](https://github.com/hyperi-io/hyperi-rustlib/commit/633644933c9e2ea6f94624f27b71f44b8fb701db))
* add engine.run() transport-wired recv→process→commit loop ([857495b](https://github.com/hyperi-io/hyperi-rustlib/commit/857495b5f5e54a5c31d0c6fd779ad1a5d8d83853))
* add FieldInterner — concurrent field name interning with DashMap ([3032cb0](https://github.com/hyperi-io/hyperi-rustlib/commit/3032cb0769037943fd49d625b2272460d6e3312e))
* add filtered counter to PipelineStats ([5cf7934](https://github.com/hyperi-io/hyperi-rustlib/commit/5cf7934666c8b443626365fb5a9bb841053dab4b))
* add missing adversarial tests — skip/fail actions, concurrent, 20K batch ([0b975e2](https://github.com/hyperi-io/hyperi-rustlib/commit/0b975e241637b85c81bf50fceb09b538d1fd1192))
* add NDJSON batch split utilities for parallel line parsing ([e1ca772](https://github.com/hyperi-io/hyperi-rustlib/commit/e1ca772116b846062ea0582a7a3ae7a1ef18b6de))
* add pre-route field extraction via sonic_rs::get_from_slice ([65e961d](https://github.com/hyperi-io/hyperi-rustlib/commit/65e961dd7c4fc282a1c6f10b1519f7579915ade3))
* add regex dep for topic resolver filters ([6abcf8e](https://github.com/hyperi-io/hyperi-rustlib/commit/6abcf8e4f1daeb04eb689eaff47faaa3aca4c3a2))
* add RuntimeContext + startupz integration tests ([dc57737](https://github.com/hyperi-io/hyperi-rustlib/commit/dc57737ce98c3e6dfe38a21e5ca2f370927f2fe8))
* add SIMD parse phase — sonic_rs for JSON, rmp_serde bridge for msgpack ([04e54b9](https://github.com/hyperi-io/hyperi-rustlib/commit/04e54b9740103a64c7af06abade8fd11a269442b))
* add sonic-rs, dashmap, bytes deps for BatchEngine ([fa5ebe7](https://github.com/hyperi-io/hyperi-rustlib/commit/fa5ebe723379e330149b9635e796f0f41a3e81e0))
* add topic resolution fields to KafkaConfig ([7324fcc](https://github.com/hyperi-io/hyperi-rustlib/commit/7324fcc025c3404b47cbe21379c648420cf160b3))
* add TopicRefreshHandle for periodic topic re-resolution ([146fa48](https://github.com/hyperi-io/hyperi-rustlib/commit/146fa48316583775172ba214119aa6830ad290d9))
* add TopicResolver — configurable suppression rules, include/exclude regex ([f6275de](https://github.com/hyperi-io/hyperi-rustlib/commit/f6275de4de164e6d8a7e603529ef5aa5c86f2a78))
* clippy and test fixups for BatchEngine ([dbfa114](https://github.com/hyperi-io/hyperi-rustlib/commit/dbfa11484f484edc477e586156d535ad12a3dc3d))
* clippy fixes — collapsible if, Debug impl, default_trait_access ([adef3b5](https://github.com/hyperi-io/hyperi-rustlib/commit/adef3b53e78f28ec81f0a099c4f0e40ca10ab09e))
* expose rayon pool install() on AdaptiveWorkerPool ([285c560](https://github.com/hyperi-io/hyperi-rustlib/commit/285c560164a4fe9b63256ee0432b856c13a66bc6))
* resolve Prometheus recorder test conflicts — test-safe MetricsManager ([cc5af7b](https://github.com/hyperi-io/hyperi-rustlib/commit/cc5af7ba9ef88b49c7912a512bb1be0947a8fb32))
* resolve Prometheus recorder test conflicts — test-safe MetricsManager ([31a9089](https://github.com/hyperi-io/hyperi-rustlib/commit/31a9089a8c101c6d6e824224ad9f494b4fc68529))
* wire BatchEngine module exports ([058eefb](https://github.com/hyperi-io/hyperi-rustlib/commit/058eefb5270ae4f8915601595fcf97cfcb8a81d5))
* wire TopicResolver into KafkaTransport — auto-discover when topics empty ([c34fc4c](https://github.com/hyperi-io/hyperi-rustlib/commit/c34fc4c11fb3e4d497aedc5f0b727c72b67cf377))

## [2.4.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.1...v2.4.2) (2026-04-01)


### Bug Fixes

* add cgroup resource limits to RuntimeContext + eviction metric ([ce8b774](https://github.com/hyperi-io/hyperi-rustlib/commit/ce8b7747ca915471fa5a0f7869419d8e90b39d2a))
* add missing schema_version and oci_labels fields to test constructors ([490559b](https://github.com/hyperi-io/hyperi-rustlib/commit/490559bd4aa6779a51414c3298c09512cebf92f1))
* deployment contract CI bridge — container manifest, OCI labels, runtime stage ([84b3e00](https://github.com/hyperi-io/hyperi-rustlib/commit/84b3e0051f5cb80f55953159c3ae11613d778fd3))
* gate memory imports in ServiceRuntime for feature-minimal builds ([dc6a7bd](https://github.com/hyperi-io/hyperi-rustlib/commit/dc6a7bdf8daf9470cd7d5981128018be3b5b54b2))

## [2.4.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.4.0...v2.4.1) (2026-04-01)


### Bug Fixes

* add /startupz startup probe endpoint ([515c090](https://github.com/hyperi-io/hyperi-rustlib/commit/515c090d898c472d560be5e116b63e5ad3d9188c))
* inject pod_name, namespace, node_name into JSON log output ([505c2fd](https://github.com/hyperi-io/hyperi-rustlib/commit/505c2fd980dc6e3a412e56d7b4b7d5890955e1ce))

# [2.4.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.3.0...v2.4.0) (2026-04-01)


### Features

* add ServiceRuntime to DfeApp — eliminates per-app boilerplate ([39aa82b](https://github.com/hyperi-io/hyperi-rustlib/commit/39aa82b3b077ced419013e1976617f8a8a8bf5aa))

# [2.3.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.2.1...v2.3.0) (2026-04-01)


### Bug Fixes

* from_cascade falls back to defaults when config not initialised ([2b93167](https://github.com/hyperi-io/hyperi-rustlib/commit/2b93167c335dad0efa46e1f8b1495d2f9a01054e)), closes [hyperi-io/dfe-loader#19](https://github.com/hyperi-io/dfe-loader/issues/19) [hyperi-io/dfe-loader#19](https://github.com/hyperi-io/dfe-loader/issues/19)


### Features

* add RuntimeContext and K8s pre-stop hook delay ([c0f88fe](https://github.com/hyperi-io/hyperi-rustlib/commit/c0f88fe7bed61dd11a65214cec9c93421779d070))

## [2.2.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.2.0...v2.2.1) (2026-03-31)


### Bug Fixes

* cap max_threads at available_parallelism even when explicitly set ([b23d16d](https://github.com/hyperi-io/hyperi-rustlib/commit/b23d16d6a1e2f096e71c86b689d4f91ad22312c8))

# [2.2.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.1.0...v2.2.0) (2026-03-31)


### Features

* add BatchProcessor trait, BatchPipeline, and PipelineStats ([71ae5ab](https://github.com/hyperi-io/hyperi-rustlib/commit/71ae5abbecd395bcf53a9f6fd51ef23d2b853eda))

# [2.1.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v2.0.0...v2.1.0) (2026-03-31)


### Bug Fixes

* remove CARGO_BUILD_JOBS=2 from STATE.md ([5549a44](https://github.com/hyperi-io/hyperi-rustlib/commit/5549a44e4991af24d5ad5a9de02f48701a58bb7c))


### Features

* add metrics-manifest and generate-artefacts CLI subcommands ([b64fcaa](https://github.com/hyperi-io/hyperi-rustlib/commit/b64fcaa1f7631b20f7eba1f0d39c5a3e67ecbac1))

# [2.0.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.23.0...v2.0.0) (2026-03-31)


### Bug Fixes

* add explicit breaking change rule to semantic-release config ([735364f](https://github.com/hyperi-io/hyperi-rustlib/commit/735364f58110055f45c523b84407f692a8ce9f6e))


### BREAKING CHANGES

* DfeMetrics::register() now requires &MetricsManager
parameter. AdaptiveWorkerPool is the new internal vertical scaling
module, bounded and throttled by available CPU and memory.

# [1.23.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.22.0...v1.23.0) (2026-03-31)


### Features

* graduate to v2 — adaptive worker pool and metrics manifest ([58c2e81](https://github.com/hyperi-io/hyperi-rustlib/commit/58c2e81dd988d20b4d07409601deea459ffa8bf6))


### BREAKING CHANGES

* DfeMetrics::register() now requires &MetricsManager
parameter for tight manifest coupling. All downstream dfe-* projects
must update their register() call sites.

# [1.22.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.21.1...v1.22.0) (2026-03-31)


### Bug Fixes

* add serde_json to metrics feature for manifest endpoint ([1c64a1c](https://github.com/hyperi-io/hyperi-rustlib/commit/1c64a1c71670211319b5570c708b249e738de0a1))
* resolve clippy warnings and remove CARGO_BUILD_JOBS limit ([dd2f0aa](https://github.com/hyperi-io/hyperi-rustlib/commit/dd2f0aa0ae2410b8223502878878a6d2190bc97c))
* resolve dfe test failures and advisory warnings ([81a4eb7](https://github.com/hyperi-io/hyperi-rustlib/commit/81a4eb7a6845bf960922904a0ef5314604cb3056))
* update dfe_groups to use _with_labels() for manifest metadata ([958ec98](https://github.com/hyperi-io/hyperi-rustlib/commit/958ec9809c0fe39079436f3367fee9470a3b18d6))


### Features

* add /metrics/manifest endpoint to both server paths ([95dc143](https://github.com/hyperi-io/hyperi-rustlib/commit/95dc1430e0e312d17d66d6fe8ab18b59d3f30799))
* add adaptive worker pool with hybrid rayon + tokio execution ([e0c3cf8](https://github.com/hyperi-io/hyperi-rustlib/commit/e0c3cf87ef060cab6af2bce50256468fa2cb67c8))
* add MetricDescriptor, MetricRegistry, ManifestResponse types ([dd2a62c](https://github.com/hyperi-io/hyperi-rustlib/commit/dd2a62c1a1c814fea2c2c3b6521da457b8912f14))
* add MetricRegistry to MetricsManager with _with_labels() methods ([1b363f0](https://github.com/hyperi-io/hyperi-rustlib/commit/1b363f01a3a13abf41679f5d80d5243b491a057e))
* add worker feature gate with rayon dependency ([99c3c10](https://github.com/hyperi-io/hyperi-rustlib/commit/99c3c1055ec0037a9607982a2725dbb171ca2b2d))

## [1.21.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.21.0...v1.21.1) (2026-03-31)


### Bug Fixes

* add BSL-1.0 to deny.toml allow list ([e59372f](https://github.com/hyperi-io/hyperi-rustlib/commit/e59372f070130fd38237aca832aef5ede1cc7caf))
* add missing license types to deny.toml allow list ([76b2571](https://github.com/hyperi-io/hyperi-rustlib/commit/76b257164f82e6fab465672f4271c004782a47a8))
* remove deprecated deny field from deny.toml ([c086931](https://github.com/hyperi-io/hyperi-rustlib/commit/c08693111c022dbbb991266059d6f135eeb9c5f5))
* remove invalid unmaintained field from deny.toml advisories ([8f0e816](https://github.com/hyperi-io/hyperi-rustlib/commit/8f0e8161e0cdf8bc48c8e1a88541dd4b5de2bd04))
* update deny.toml to cargo-deny 0.19 format ([7b5a977](https://github.com/hyperi-io/hyperi-rustlib/commit/7b5a9770fdd5e1fb922cc3079b7fb635fa7dce59))

# [1.21.0](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.20.2...v1.21.0) (2026-03-27)


### Bug Fixes

* add core pillars doc reference to lib.rs module docs ([76bfd77](https://github.com/hyperi-io/hyperi-rustlib/commit/76bfd77576f860f0cf40e5db771117789b26fcbf))
* add deny.toml, document histogram stub, block single-dot in table names ([e4e18a5](https://github.com/hyperi-io/hyperi-rustlib/commit/e4e18a5d370b358413272b804a34e62a93008257))
* correct recv() metric names and describe_gauge in AppMetrics ([cdec2c1](https://github.com/hyperi-io/hyperi-rustlib/commit/cdec2c121939ab965eb582394275beb1dc05710c))
* register file, pipe, http, and redis transports with health registry ([164eca4](https://github.com/hyperi-io/hyperi-rustlib/commit/164eca4a76382edb9205faebb383e20ba8e207c8))
* replace expect() with proper error handling in shutdown, metrics, http_client ([d775e85](https://github.com/hyperi-io/hyperi-rustlib/commit/d775e851b9e35dd309720f31f5d9ad5a450ca030))


### Features

* add HTTP and Redis DLQ backends ([8ea0af5](https://github.com/hyperi-io/hyperi-rustlib/commit/8ea0af52bfccd4fdbe5eec60585437d3c6df9393))

## [1.20.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.20.1...v1.20.2) (2026-03-27)


### Bug Fixes

* add tag input to workflow_dispatch for publish trigger ([629a6e8](https://github.com/hyperi-io/hyperi-rustlib/commit/629a6e87d6b507d2530814522245fb0909894f06))

## [1.20.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.20.0...v1.20.1) (2026-03-27)


### Bug Fixes

* remove trailing blank line from lib.rs ([74c448e](https://github.com/hyperi-io/hyperi-rustlib/commit/74c448ed68a95192bdeef1f36023351f9f8c7f92))
* validate single versioning publish pipeline ([fdfba9d](https://github.com/hyperi-io/hyperi-rustlib/commit/fdfba9dd3684f740d7bd79fab1d41fd3d98ac7da))
* validate single versioning publish pipeline ([09c01c2](https://github.com/hyperi-io/hyperi-rustlib/commit/09c01c2fef29f31008e0c67e2564dfe8982817d2))
* verify single versioning pipeline end-to-end ([0ac98cb](https://github.com/hyperi-io/hyperi-rustlib/commit/0ac98cb00ccc2dfc746625815d7494f79a2ffbef))

# [1.17.0-dev.16](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.15...v1.17.0-dev.16) (2026-03-26)


### Bug Fixes

* add health registry, shutdown manager, and wire all modules ([21efaa2](https://github.com/hyperi-io/hyperi-rustlib/commit/21efaa2a4d61901dfe479d655ef9af1a5c437580))

# [1.17.0-dev.15](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.14...v1.17.0-dev.15) (2026-03-26)


* feat!: split Transport trait and add 4 new transports + factory ([36c383d](https://github.com/hyperi-io/hyperi-rustlib/commit/36c383d8bcdf96a120bf238cc45e629be984aa47))


### BREAKING CHANGES

* Transport trait split into TransportBase (close,
is_healthy, name), TransportSender (send), and TransportReceiver
(recv, commit, Token). Blanket Transport impl for types with both.

New transport backends:
- File: NDJSON with position tracking and commit persistence
- Pipe: stdin/stdout for Unix pipeline composition
- HTTP: POST to endpoint (send) + embedded axum server (receive)
- Redis/Valkey Streams: XADD/XREADGROUP/XACK with consumer groups

Transport factory:
- AnySender: enum dispatch for runtime transport selection
- AnySender::from_config(): create sender from config cascade
- RoutedSender: per-key dispatch for data originators (receiver/fetcher)

All transports auto-emit dfe_transport_* Prometheus metrics.
648 tests pass.

# [1.17.0-dev.14](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.13...v1.17.0-dev.14) (2026-03-26)


### Bug Fixes

* add metrics instrumentation to all modules ([0a690e4](https://github.com/hyperi-io/hyperi-rustlib/commit/0a690e40c9108f25394cda19d47c98f4774ed30f))
* wire StatsContext into KafkaTransport and add send metrics ([ccfc9ad](https://github.com/hyperi-io/hyperi-rustlib/commit/ccfc9ad10180be10345a05605d0dca0d8601d254))

# [1.17.0-dev.13](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.12...v1.17.0-dev.13) (2026-03-26)


### Bug Fixes

* add metrics instrumentation to gRPC transport ([85839b7](https://github.com/hyperi-io/hyperi-rustlib/commit/85839b78df82daf01153b93bd27f42bd31604a25))

# [1.17.0-dev.12](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.11...v1.17.0-dev.12) (2026-03-24)


### Bug Fixes

* pin reqwest to 0.12 to avoid dual-version conflict ([8dc1b6d](https://github.com/hyperi-io/hyperi-rustlib/commit/8dc1b6dc6e7cf336dc20e434094bad52526d22cc))

# [1.17.0-dev.11](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.10...v1.17.0-dev.11) (2026-03-24)


### Bug Fixes

* remove unused env feature flag ([e7f41fa](https://github.com/hyperi-io/hyperi-rustlib/commit/e7f41fa6c5535e181b44940f400fbc7363ecf285))

# [1.17.0-dev.10](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.9...v1.17.0-dev.10) (2026-03-24)


### Bug Fixes

* add http_client, database URL builders, and cache modules ([eb76b54](https://github.com/hyperi-io/hyperi-rustlib/commit/eb76b54281287692df9632f5318a35ebef3ab25b))

# [1.17.0-dev.9](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.8...v1.17.0-dev.9) (2026-03-24)


### Bug Fixes

* add SensitiveString type, ConfigReloader registry hook, redaction tests ([afe7a1d](https://github.com/hyperi-io/hyperi-rustlib/commit/afe7a1dc03a9eadb8280b6bceb72371888cb5888))

# [1.17.0-dev.8](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.7...v1.17.0-dev.8) (2026-03-24)


### Bug Fixes

* add config redaction, /config endpoint, change notification ([c6a796b](https://github.com/hyperi-io/hyperi-rustlib/commit/c6a796b00868d91d65833b790b47ac2fa812afbe))
* wire scaling, grpc, secrets configs into registry ([c1bd924](https://github.com/hyperi-io/hyperi-rustlib/commit/c1bd924ac88d34db3ff82bb97c4734aa785bb3a5))

# [1.17.0-dev.7](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.6...v1.17.0-dev.7) (2026-03-24)


### Bug Fixes

* add reflectable config registry with auto-registration ([a88f50d](https://github.com/hyperi-io/hyperi-rustlib/commit/a88f50dda5ff7574e7b6eeb1ce9188336d281b77))
* wire module configs into the config cascade ([cdb9f79](https://github.com/hyperi-io/hyperi-rustlib/commit/cdb9f79e58a92cbe6c489098aade1eddcddd8a65))

# [1.17.0-dev.6](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.5...v1.17.0-dev.6) (2026-03-24)


### Bug Fixes

* block regex (matches) in CEL expressions by default ([c192c0a](https://github.com/hyperi-io/hyperi-rustlib/commit/c192c0a24ed24f37a914af1bd6597e61baaa80e3))
* prevent underflow in MemoryGuard::release() ([9b74994](https://github.com/hyperi-io/hyperi-rustlib/commit/9b749949e6d5e34caaeb7bb6402cf465ff50c00e))
* restructure tests to match testing standard ([1539471](https://github.com/hyperi-io/hyperi-rustlib/commit/15394714cb1508ef94882e37e6d1ae9366e0e0a6))

# [1.17.0-dev.5](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.4...v1.17.0-dev.5) (2026-03-23)


### Bug Fixes

* replace invalid Renovate preset :pinActionsToFullSha with helpers:pinGitHubActionDigestsToSemver ([39c09a9](https://github.com/hyperi-io/hyperi-rustlib/commit/39c09a941d68add12ef82a024d2b05c1d3cf6fff))

# [1.17.0-dev.4](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.3...v1.17.0-dev.4) (2026-03-22)


### Bug Fixes

* inline Renovate config (preset resolution broken) ([5f60ad3](https://github.com/hyperi-io/hyperi-rustlib/commit/5f60ad3d4a7083b85a776e20e11d0ded2d12ffa1))

# [1.17.0-dev.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.2...v1.17.0-dev.3) (2026-03-21)


### Features

* add RenderHandle for sharing Prometheus render across tasks ([cd872a6](https://github.com/hyperi-io/hyperi-rustlib/commit/cd872a6e1840b7338c45f1873ae37894501dc58a))

# [1.17.0-dev.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.17.0-dev.1...v1.17.0-dev.2) (2026-03-20)


### Bug Fixes

* bump lz4_flex upper bound, update deps, fix clippy lints ([a31e07f](https://github.com/hyperi-io/hyperi-rustlib/commit/a31e07f959b304b4de021c9346358cda8589ec82))

# [1.17.0-dev.1](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2-dev.6...v1.17.0-dev.1) (2026-03-20)


### Bug Fixes

* add create_topics and delete_topics to KafkaAdmin ([21eb0c1](https://github.com/hyperi-io/hyperi-rustlib/commit/21eb0c190c502fe227975b490edbbaae0daad442))
* add feature gates to gRPC integration tests ([b9bc063](https://github.com/hyperi-io/hyperi-rustlib/commit/b9bc06374c92876f2790bfb47c765e6b00eb701c))
* add start_server_with_routes, scaling/memory endpoints to MetricsManager [skip ci] ([b229749](https://github.com/hyperi-io/hyperi-rustlib/commit/b229749b2cacb2ee1360b035ab83c6b39c3bfb2d))
* auto-emit config reload and rdkafka Prometheus metrics ([9aa2893](https://github.com/hyperi-io/hyperi-rustlib/commit/9aa2893938662d55aea0a983269760aeb29102a7))
* bump lz4_flex upper bound, update deps, fix clippy lints ([f7c04f1](https://github.com/hyperi-io/hyperi-rustlib/commit/f7c04f130dd385c49c930355844360672c0334f8))


### Features

* add metrics-dfe feature with composable metric groups ([e26c2dd](https://github.com/hyperi-io/hyperi-rustlib/commit/e26c2dd99fd3947c4999431ac4855522753c5ccf))

## [1.16.2-dev.6](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2-dev.5...v1.16.2-dev.6) (2026-03-20)


### Bug Fixes

* add readiness callback to MetricsManager health endpoints ([500ccf3](https://github.com/hyperi-io/hyperi-rustlib/commit/500ccf3f7d8eee6eca741e75b186cc290efc063d))

## [1.16.2-dev.5](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2-dev.4...v1.16.2-dev.5) (2026-03-19)


### Bug Fixes

* add from_env/from_env_raw to MemoryGuardConfig, tune defaults ([3c59845](https://github.com/hyperi-io/hyperi-rustlib/commit/3c5984563857f494cbbac505184756ec71d6ce10))

## [1.16.2-dev.4](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2-dev.3...v1.16.2-dev.4) (2026-03-19)


### Bug Fixes

* add DfeSource convention for topic naming and consumer groups ([3b0c7da](https://github.com/hyperi-io/hyperi-rustlib/commit/3b0c7daceb9aa091093bad6b9f05f3366757dc0c))

## [1.16.2-dev.3](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2-dev.2...v1.16.2-dev.3) (2026-03-19)


### Bug Fixes

* add concurrent and edge-case tests for MemoryGuard ([b411d1c](https://github.com/hyperi-io/hyperi-rustlib/commit/b411d1c5a3fb85082da8cd6066248c72dc349d9a))
* add MemoryGuard — cgroup-aware memory backpressure for OOM prevention ([fba690f](https://github.com/hyperi-io/hyperi-rustlib/commit/fba690fb3365d3a4240cba4fee9e894c2f13d40b))

## [1.16.2-dev.2](https://github.com/hyperi-io/hyperi-rustlib/compare/v1.16.2-dev.1...v1.16.2-dev.2) (2026-03-19)


### Bug Fixes

* add data quality event helpers (DLQ routing, quality alerts) ([2a6eeb6](https://github.com/hyperi-io/hyperi-rustlib/commit/2a6eeb67109c4fbb59eb77f22622100dc64f3091))
* add flat env override helpers, ApplyFlatEnv and Normalize traits ([e583ecf](https://github.com/hyperi-io/hyperi-rustlib/commit/e583ecf650e0b1d8142e6f63a7fd233c88071192))
* add security event logging framework (OWASP-aligned) ([f08819b](https://github.com/hyperi-io/hyperi-rustlib/commit/f08819b18b01924bceee7f6e30460cbc78366eb0))
* inject service name and version into JSON log output ([e3c70ef](https://github.com/hyperi-io/hyperi-rustlib/commit/e3c70eff1a76ae31e667cb4da6fc9790c12f515e))

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
