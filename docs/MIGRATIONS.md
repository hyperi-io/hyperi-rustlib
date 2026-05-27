# MIGRATIONS

API surface changes that require consumer adjustment. Indexed by the
rustlib version where the change first ships. Used by the
[rebuild-consumers skill](../.claude/skills/rebuild-consumers/SKILL.md)
when `cargo check` flags breakage on a downstream bump.

Pre-GA discipline: no `BREAKING CHANGE:` footer, no major bump. All
six core DFE apps migrate in lockstep.

---

## Unreleased — pre-GA hardening (branch `fix/secret-yaml-hyphens`)

### `AdaptiveWorkerPool::fan_out_async` return type

| Old | `Vec<Result<R, E>>` |
| New | `Vec<Option<Result<R, E>>>` |

Panicked tasks now surface as `None` instead of being silently dropped
(the old `.flatten()` shortened the output and broke the documented
input-order contract). Caller adjustment:

```rust
// Drop panicked slots (old behaviour):
let results: Vec<Result<R, E>> = pool.fan_out_async(items, f)
    .await
    .into_iter()
    .flatten()
    .collect();

// Or destructure explicitly:
for (i, slot) in pool.fan_out_async(items, f).await.into_iter().enumerate() {
    match slot {
        Some(Ok(r)) => { /* success */ }
        Some(Err(e)) => { /* task returned Err */ }
        None => tracing::error!(idx = i, "task panicked"),
    }
}
```

### `DbConnection.password` type

| Old | `String` |
| New | `SensitiveString` |

Construction via `DbConnection { password: "x".into(), ... }` keeps
working (`From<&str> for SensitiveString` exists). Sites that clone
the plaintext need `.expose()`:

```rust
// Before:
let url = format!("postgres://{}:{}@{}/{}", c.user, c.password, c.host, c.db);
// After:
let url = format!("postgres://{}:{}@{}/{}", c.user, c.password.expose(), c.host, c.db);
```

In-crate URL builders already do this. The change exists so `Debug`
+ `serde` round-trips redact by default.

### `Cache::set` signature

| Old | `fn set(&self, ...) -> ()` |
| New | `fn set(&self, ...) -> Result<(), serde_json::Error>` |

The old form swallowed serialise failures via `Err(_) => return`.
Callers now propagate or `.expect("cache set")` if a panic is the
right escalation:

```rust
cache.set(&key, &value, source).expect("cache set");
// or
cache.set(&key, &value, source)?;
```

### `expose_during` (additive)

New crate-root helper `hyperi_rustlib::expose_during<F, R>(f: F) -> R`
flips a thread-local flag so `SensitiveString` serialises its real
value inside the closure. Wrap any figment / serde round-trip that
must preserve secrets:

```rust
let cfg: Config = expose_during(|| {
    Figment::from(Serialized::defaults(&defaults))
        .merge(Env::prefixed("MYAPP_"))
        .extract()
})?;
```

Required for any consumer using `Figment::from(Serialized::defaults(&Config))`
where `Config` contains `SensitiveString` fields. Symptom of missing
this: secrets land as literal `***REDACTED***` post round-trip and
auth fails.

### `SinkDrain::flush_durable` (additive, default no-op)

New trait method; existing impls compile unchanged. Custom drains
that need true durability override (file flushes the writer, Kafka
calls `producer.flush()`).

### `Dlq` struct (internal layout)

Gained a `cancel: CancellationToken` field. Consumers construct via
`Dlq::spawn` or `Dlq::disabled` and don't touch the struct fields —
no visible change.

### `TransportFilterTierConfig.budget`

New optional field defaulting to permissive values
(`max_ast_nodes=200`, `max_iteration_depth=2`,
`max_payload_bytes=1MiB`). YAML configs without the field continue
to deserialise. To tighten:

```yaml
transport:
  filter_tiers:
    budget:
      max_ast_nodes: 100
      max_iteration_depth: 1
      max_payload_bytes: 524288   # 512 KiB
```

### `StrMatcherSet` (no surface change)

Internal partition into merged-AC + individual matchers. Public API
unchanged (`is_match`, `find`, `find_iter`, `earliest_match`,
`tier_counts`, `len`, `is_empty`).

### `HttpServerConfig.max_connections`

Now actually wired (was silently inert). Default 10,000. Consumers
that set `max_connections: 1` to "disable" filtering will now hit
a hard cap; raise to a realistic number or document the throttling
intent.

### Telemetry / `version-check`

`CheckPayload` no longer includes `instance_id` or `deployment`.
`HYPERI_TELEMETRY=off` opt-out env var. No consumer code change
required; the field rename only affects log lines from rustlib
itself.

---

## Known open issues (not fixed on this branch)

Tracked upstream; each needs its own focused commit. Workarounds
applied at the consumer level until then.

### #35 — Kafka topic auto-discovery race

`KafkaAdmin::list_topics` returns empty when the admin consumer
hasn't finished its bootstrap handshake. Symptom: "Auto-discovery
found no matching topics" at startup even though the topic exists.

**Workaround:** drop `topic_regex` from the config and list topics
explicitly under `topics:`. The explicit-subscribe path bypasses
the resolver.

### #36 — `KafkaTransport` always allocates both roles

`KafkaTransport::new` builds both a `BaseConsumer` and a
`FutureProducer` from the same `ClientConfig`. Producer-only
callers with empty `group.id` get "rdkafka consumer queue not
available" because the consumer half can't construct without a
group.

**Workaround:** set `librdkafka_options.group.id` to a dummy
non-empty value on producer-side configs. The dummy group is
never used; only present so `BaseConsumer` doesn't fail to
construct.

### #37 — `TransportSender::send(key, payload)` overloads `key` as topic

The Kafka impl passes `key` to `FutureRecord::to(key)`, so the
"key" arg is the destination topic, not a partition key. Callers
can't route to a configured topic AND set a partition key in one
call.

**Workaround:** none. Sites needing partition keys must bypass
the trait and use rdkafka directly.

---

## Older releases

Historical migrations live in agent memory at `project_dfe_*_migration.md` (referenced from `MEMORY.md`) until they graduate here.
