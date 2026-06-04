# Self-regulation

The data plane regulates itself. A rustlib app sized for steady state does
not fall over when a burst arrives, an upstream stalls, or a transform
balloons memory -- the pipeline slows its own intake, lets the in-flight
work drain, and speeds back up once the pressure clears. This happens
automatically; an app wires nothing.

Self-regulation is **ON by default**. To turn it off (byte-identical to the
pre-governor data path), set one cascade key:

```yaml
self_regulation:
  enabled: false
```

When `enabled = false` the runtime constructs NOTHING -- no pressure
governor, no inbound gate, no byte-budget controller. Every `Option` stays
`None` and the data path is the original whole-batch loop. There is no
half-on state.

See also [BACKPRESSURE.md](BACKPRESSURE.md) (where the brake is applied and
why) and [KAFKA-PATH.md](KAFKA-PATH.md) (how the Kafka GET/PROCESS/SEND
batch sizes feed the loop). The code lives in `src/governor/`.

---

## The three brains

Self-regulation is three distinct controllers with three distinct jobs.
They are NOT interchangeable; each answers a different question.

| Brain | Question | Acts on | Source of truth? |
|---|---|---|---|
| **MemoryGuard** | "Are we about to OOM?" | The HARD pressure signal | YES -- the never-OOM authority |
| **ScalingPressure** | "Do we need more pods?" | KEDA / external scaler signal | Pool sizing, not the data path |
| **UnifiedPressure** | "Should I pull more work right now?" | The inbound gate + byte budget | Derived from the sources above |

- **MemoryGuard** (`src/memory/`) is the source of truth. It tracks
  in-flight ingress bytes (and, when a heap source is wired, the true
  process heap -- see the `set_heap_source` entry in
  [MIGRATIONS.md](MIGRATIONS.md)). Its `pressure_ratio()` is fed into the
  governor as a **HARD** source: never weighted, never masked. A saturated
  soft signal can never lower the combined level below what memory demands.
  This is the never-OOM guarantee.
- **ScalingPressure** (`src/scaling/`) drives horizontal scaling. It emits
  the external-scaler signal KEDA reads to add or remove pods. It is a
  capacity lever, not a data-path lever -- it does not pause intake, it
  asks for more replicas. See [pipeline/SCALING.md](pipeline/SCALING.md).
- **UnifiedPressure** (`src/governor/source.rs`) combines the sources into
  ONE normalised level in `[0.0, 1.0]` under a hysteretic latch. It is what
  the inbound gate and the byte-budget controller both consult. It owns no
  signal of its own -- it is the seam that turns the brains' readings into
  a single pause/resume decision.

### Why memory is HARD and CPU is deliberately dropped

Memory is the only resource that kills the process. Run out of CPU and the
work merely runs slower; run out of memory and the kernel OOM-kills the pod
and in-flight data is lost. So memory is the HARD source -- the one signal
that always gets through.

CPU is deliberately NOT a pressure source:

- **CFS self-corrects.** Under a CPU quota the Linux scheduler throttles the
  process for us. A CPU-bound stage simply takes longer per batch; the
  byte-budget loop sees the longer process time and shrinks on its own. No
  separate CPU brake is needed -- adding one would double-count the same
  signal.
- **CPU saturation surfaces as lag, and lag is KEDA's job.** A pod that
  cannot keep up grows consumer lag; KEDA reads the lag and adds a replica.
  Horizontal scale is the right answer to "not enough CPU", not pausing
  intake on the one pod that is already maxed.

The seam is built to accept a CPU source LATER with **zero API change**.
`UnifiedPressure::add_source` takes any `PressureSource`; a future CPU
source would plug in as a SOFT, weighted source and every existing caller
of `level()` / `should_hold()` is untouched. The decision to drop CPU is a
default, not a wall.

---

## How the loop works

```text
  MemoryGuard.pressure_ratio()  --HARD-->  UnifiedPressure.level()
                                                |
                  +-----------------------------+-----------------------------+
                  |                                                           |
          InboundGate                                              ByteBudgetController
   (pause/resume the SOURCE)                                  (AIMD lever -> sub-block size)
```

- **InboundGate** (`src/governor/gate.rs`) turns the latch into EDGE events:
  `pause()` once on the rising edge, `resume()` once on the falling edge. It
  pauses the inbound SOURCE (stops pulling new work) -- never the outbound
  drain. See [BACKPRESSURE.md](BACKPRESSURE.md) for why gating the drain
  deadlocks.
- **ByteBudgetController** (`src/governor/budget.rs`) is an AIMD
  (additive-increase / multiplicative-decrease) lever that sizes the inbound
  byte budget for a target utilisation `rho ~= 0.7`. Slack grows the budget
  additively; falling behind shrinks it multiplicatively; a memory HARD
  override shrinks IMMEDIATELY regardless of `rho`. See
  [KAFKA-PATH.md](KAFKA-PATH.md) for the full AIMD description and the
  PROCESS byte-budget's place among the three Kafka batch sizes.

The controller starts BIG (`start_bytes`) and lets the decrease loop find
the level -- a cold pipeline is never artificially throttled. While pressure
is LOW the budget sits at its big start value, so a received block becomes a
SINGLE sub-block with no per-record overhead: behaviour matches the
whole-batch loop. Near-zero cost off-pressure.

The governed driver (`BatchEngine::run_governed`) is the run path a
self-regulating app calls. It dispatches on whether the byte budget is wired:
budget present -> stream in sub-blocks sized to the current budget and fold
each block's `(bytes, process_time, ingest_interval)` into the AIMD loop;
budget absent (governor off) -> delegate verbatim to `run_workbatch`,
byte-identical to pre-governor behaviour. The streaming sub-block mechanics
live in [BACKPRESSURE.md](BACKPRESSURE.md).

---

## Observe

Self-regulation is visible, not mysterious. When throttling happens you can
see it.

| Signal | Kind | Meaning |
|---|---|---|
| `inbound_paused` | gauge (0/1) | The inbound gate is currently holding (1) or open (0) |
| `self_regulation_inbound_pauses_total` | counter | Number of pause EDGES (rising transitions), not per-evaluate noise |
| `self_regulation_byte_budget` | gauge | Current AIMD byte budget, per block |
| `pressure_ratio` | gauge | Combined `UnifiedPressure.level()` in `[0, 1]` |

Because the gate fires each edge EXACTLY ONCE (`ObservingActuator` in
`src/governor/gate.rs`), the gauge and counter track real transitions. The
gate also logs a brake-reason line on each edge:

```text
WARN  self-regulation: inbound PAUSED under pressure (memory/back-pressure brake)  source=kafka
INFO  self-regulation: inbound RESUMED, pressure cleared  source=kafka
```

A pause without a matching resume in the logs means the pressure has not
cleared -- check the memory guard and consumer lag.

---

## Tune

All tuning is via the `self_regulation` cascade section (8-layer cascade,
hot-reload, `/config` admin endpoint -- same as every other config section;
see [core-pillars/CONFIG.md](core-pillars/CONFIG.md)).

```yaml
self_regulation:
  enabled: true            # master switch (default true)
  profile: throughput      # throughput | balanced | low_latency -- sizes the AIMD envelope
  pause_above: 0.80        # arm the inbound hold when combined pressure reaches this
  resume_below: 0.65       # release the hold when pressure drops to this (must be < pause_above)
  target_rho: 0.7          # target utilisation for the byte-budget AIMD loop, in (0, 1)
  md_factor: 0.5           # multiplicative-decrease factor, in (0, 1)
```

- **`enabled`** -- the only knob most apps touch. `false` builds nothing.
- **`profile`** -- sizes the AIMD byte-budget envelope (start / ceiling /
  step / record cap). `throughput` starts big with a high ceiling;
  `low_latency` starts small so blocks stay small and bursty. It mirrors the
  Kafka `SelfRegulationProfile` names so one value reads the same regardless
  of transport.
- **`pause_above` / `resume_below`** -- the hysteresis band. The gap between
  them prevents flapping: the latch arms at `pause_above`, releases at
  `resume_below`, and holds its state in between. An inverted or non-finite
  band falls back to the defaults (`0.80` / `0.65`) with a warning rather
  than wedging the governor.
- **`target_rho`** -- how busy to keep the stage. Lower means more headroom
  (safer under bursts); higher means tighter packing (more efficient,
  riskier).
- **`md_factor`** -- how hard to brake when behind or under memory pressure.
  `0.5` halves the budget per decrease step.

Every default is set so an app that configures nothing gets a fully working,
default-ON governor. Bad knobs are sanitised, not fatal.

### Small / memory-tight pods

The default profile is `throughput`, which starts with a large byte budget
(start-big, back-off-on-pressure). That is the right call for a PB/day
ingest pod with headroom, but on a small or memory-tight pod it can spike
in the COLD-START WINDOW -- the very first block is sized to the start
budget before the AIMD loop or the memory-hard override has seen any
pressure to react to. The governor self-corrects after that first block
(the memory-hard override drops the budget the moment in-flight bytes climb
toward the limit), so this is a transient first-block spike, not a steady
state.

There is deliberately NO dedicated "small-pod" preset (YAGNI). For a
small/memory-tight pod, do one of:

- Set a lower start budget directly under `self_regulation` (cap the
  first-block size so the cold-start window cannot overshoot the pod's
  memory limit).
- Use the `balanced` or `low_latency` profile -- both start with a smaller
  byte-budget envelope, so the cold-start first block is correspondingly
  smaller.

Either way the memory-hard override remains the never-OOM backstop; the
profile/start-budget choice only governs how large that first pre-feedback
block is.

### cgroup OOM-kill operational test (release checklist)

The in-process logical never-OOM test asserts the governor's control loop
never lets in-flight bytes exceed the configured limit. It does NOT prove
the process survives a real OS-level cgroup OOM-killer under a hard
container memory limit. The real test -- a memory-limited container under
sustained load, asserting NO cgroup OOM-kill where an ungoverned pipeline
would be killed -- is a RELEASE-CHECKLIST / CI-harness item, run out of
process against a real cgroup. It is not covered by the in-process unit
tests and must be exercised separately before a release.
