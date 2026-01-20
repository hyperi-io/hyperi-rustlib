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
