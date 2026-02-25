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
