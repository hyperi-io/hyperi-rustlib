# TODO - hs-rustlib

**Project Goal:** Rust shared library equivalent to hs-lib (Python) and hs-golib (Go)

**Target:** Production-ready library for HyperSec Rust applications

---

## Current Tasks

### High Priority

- [ ] Add integration tests for metrics HTTP server
- [ ] Implement parity tests against hs-golib (config cascade behavior)
- [ ] Add example application demonstrating all features

### Medium Priority

- [ ] Add more comprehensive config cascade tests (YAML file loading)
- [ ] Implement log output capturing for logger tests
- [ ] Add metrics server graceful shutdown tests

### Low Priority

- [ ] Benchmark config loading performance
- [ ] Add colored log output for text format
- [ ] Document environment variable naming conventions

---

## Completed

- [x] Project setup with feature flags - 2025-12-24
- [x] Environment detection module (K8s/Docker/Container/BareMetal) - 2025-12-24
- [x] Runtime paths module (XDG + container awareness) - 2025-12-24
- [x] Configuration module (7-layer cascade with figment) - 2025-12-24
- [x] Logger module (structured JSON, RFC3339, masking) - 2025-12-24
- [x] Metrics module (Prometheus + process/container) - 2025-12-24
- [x] All 36 unit tests passing - 2025-12-24
- [x] Clippy passing with pedantic warnings - 2025-12-24
- [x] Initial commit and push to GitHub - 2025-12-24

---

## Blocked

None currently.

---

## Backlog (P2)

- [ ] HTTP client module with retry middleware (reqwest-retry)
- [ ] Database URL builders module
- [ ] Cache module with disk backing

---

## Notes

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Feature flags: `config`, `logger`, `metrics`, `runtime`, `env` (always on)
- MVP complete - iterate based on usage feedback

---

**Last Updated:** 2025-12-24
