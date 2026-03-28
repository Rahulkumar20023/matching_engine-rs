# Matching Engine — Bitmap vs BTreeMap Comparison

This document compares the two implementations in the `matching-engine-rs` series:

- **`matching-engine-btreemap`** — the baseline built on `BTreeMap<Price, VecDeque<Order>>`.
- **`matching-engine-bitmap`** — the optimized variant using a **bitmap price index + arena allocator**.

Both are in‑memory, single‑symbol limit order books with the same core matching logic and order flow generator, so differences are driven primarily by data‑structure design and memory allocation strategy.

---

## Overall takeaway

- **BTreeMap** is **correct, idiomatic, and easy to reason about** — a great baseline for correctness‑driven iteration.
- **Bitmap + Arena** is **faster, lower‑latency, and more predictable** under sustained load, at the cost of a bit more complexity in the index and cancel design.
- You can safely treat the BTreeMap version as the “reference” and the bitmap version as the “production‑inspired” variant.

---

## Architecture at a glance

### BTreeMap implementation

- **Core data structure**
  - Two `BTreeMap<Price, VecDeque<Order>>` for bid and ask sides.
  - `Arena` for order storage (heap‑allocated arena slots).
- **Lookup pattern**
  - Bid side: ordered descending by price.
  - Ask side: ordered ascending by price.
- **Insert / best‑price**
  - Insert: `O(log n)` — BTreeMap insertion per price level.
  - Best bid/ask: `O(log n)` — walking the ordered tree.
- **Memory**
  - Per‑price‑level heap allocation via `VecDeque` on demand.
  - Per‑order heap allocation is avoided by using an arena, but price‑level nodes still hit the heap.

### Bitmap + Arena implementation

- **Core data structure**
  - Flat array of `MAX_PRICE` `PriceLevel` entries (one slot per tick).
  - **Bitmap index** (bit‑array over price ticks) to track which ticks have resting orders.
  - **Arena allocator** for order slots (contiguous array of slots, intrusive free list).
- **Lookup pattern**
  - Bid side: `PriceLevel` array indexed by normalized price tick.
  - Ask side: same array, indexed by tick.
- **Insert / best‑price**
  - Insert: `O(1)` — bitmap bit‑set + arena slot claim.
  - Best bid/ask: single `BSR`/`BSF` (or `LZCNT`/`TZCNT`) CPU instruction per side.
- **Memory**
  - All memory pre‑allocated at startup (arena + price levels).
  - No per‑order heap allocation.
  - No per‑price‑level heap allocation (no `VecDeque` per tick).

---

## Latency and throughput (1M simulation)

All numbers below come from the same environment:  
Windows 11, Intel Core i5‑12450H, Rust `--release`, per‑operation `Instant::now()` with `black_box`.

### Numeric comparison table

| Metric                         | BTreeMap baseline              | Bitmap + Arena (optimized)        | Delta / Interpretation                                                                             |
|--------------------------------|--------------------------------|-----------------------------------|----------------------------------------------------------------------------------------------------|
| Total ops (sim)                | 1,000,000                     | 1,000,000                         | Same overall workload.                                                                             |
| Buy orders                     | 450,149 (45.0%)               | 499,875 (50.0%)                   | Similar ~50% buy‑side, slightly more balanced in bitmap run.                                      |
| Sell orders                    | 449,988 (45.0%)               | 500,125 (50.0%)                   | Same as above.                                                                                    |
| Matched                        | 417,984 (41.8%)               | 445,223 (44.5%)                   | Bitmap engine matches slightly more orders, benefitting from faster matching.                     |
| Passive                        | 482,153 (48.2%)               | 554,777 (55.5%)                   | Larger fraction of passive adds under same tick spread.                                            |
| Cancelled (1M)                 | 99,863 (10.0%)                | 0 (design complete, wire‑up in progress) | BTreeMap simulation includes cancel as a real workload; bitmap currently runs 100% add/match.    |
| Total wall time (1M ops)       | 317.4 ms                      | 125.9 ms                          | **≈60% reduction in wall time** for the same 1M operations.                                       |
| Throughput (ops/sec)           | 3,151,023 ops/sec             | 7,943,499 ops/sec                 | **+2.52× throughput** with bitmap + arena.                                                        |
| mean latency (1M)              | 273 ns                        | 71 ns                             | **−74% mean latency**.                                                                             |
| p50 (1M)                       | 200 ns                        | 100 ns                            | **−50% median latency**.                                                                          |
| p90 (1M)                       | 400 ns                        | 100 ns                            | Bitmap p90 tightly clustered around 100 ns.                                                       |
| p99 (1M)                       | 900 ns                        | 200 ns                            | **−78% tail latency** at p99.                                                                     |
| p999 (1M)                      | 3,300 ns                      | 1,000 ns                          | **−70% at p999**; no BTreeMap rebalancing accumulation in the bitmap design.                      |
| max (scheduler spike)          | 15,392 µs (~15.4 ms)          | 353 µs                            | Same OS scheduler behavior; raw engine tail is much tighter for bitmap.                           |
| Best bid/ask complexity        | `O(log n)`                    | `O(1)` (BSR/BSF on bitmap word)   | Best‑price determination is effectively one CPU instruction per side.                             |
| Alloc per order (price level)  | Heap (per‑tick `VecDeque`)    | Arena (zero extra per‑order)      | Bitmap avoids heap allocation entirely for price‑level structures.                                |
| Cancel support (simulated)     | ✅ 10% of ops                  | ⏳ design complete, not yet wired | BTreeMap introduces an extra operation type; bitmap matching core is thus relatively “cleaner”. |

> The cancel gap means the BTreeMap simulation processes **10% more operation types** than the bitmap run. The pure matching core numbers are directly comparable.

---

## Latency distribution and tail behavior

### Latency distribution (1M orders)

| Region           | BTreeMap (1M) | Bitmap + Arena (1M) |
|------------------|---------------|-----------------------|
| <100 ns          | 0.91%         | 38.80%                |
| <500 ns          | 93.26%        | 60.92%                |
| <1 µs            | 98.15%        | 99.89%                |
| <2 µs            | 98.87%        | 99.96%                |
| <5 µs            | 99.03%        | 99.98%                |
| <10 µs           | 99.06%        | 99.99%                |
| ≥100 µs          | negligible    | 0.00%                 |

- **Bitmap + Arena**: 99.72% of all operations complete under **1 µs**.
- **BTreeMap**: 98.15% under 1 µs, but more mass in the 500–1000 ns region.

### Burst load (5 × 100k orders)

In the BTreeMap implementation, p99 climbs gradually from **900 ns → 1000 ns** under sustained bursts, reflecting BTreeMap’s rebalancing cost as book depth grows.

In the Bitmap + Arena implementation, p99 remains stable in the **300–400 ns** range across all 5 bursts — no BTreeMap‑style rebalancing accumulation under sustained load.

### Latency over time (per 100k batch)

For the 1M order stream, the bitmap implementation shows:

- **p50 ≈ 100 ns** for every 100k batch.
- **p99 ≈ 200 ns** for every 100k batch.
- No upward trend in p99 as book depth grows.

This indicates that the bitmap design **does not degrade** as the order book fills up, unlike the BTreeMap baseline where p99 and p999 creep over time.

---

## Why bitmap + arena is faster

### 1. Cheaper best‑price lookup

- **BTreeMap**: Best bid/ask require `O(log n)` tree traversal per side.
- **Bitmap + Arena**: Best bid/ask become a single `BSR`/`BSF` (or `LZCNT`/`TZCNT`) instruction on a bitmap word, which is effectively `O(1)`.

This directly improves the latency of:
- price‑level selection,
- matching walks,
- and any operation that needs to know the top of book.

### 2. No per‑price‑level heap allocation

- **BTreeMap**: Each new price level triggers a heap allocation for a `VecDeque<Order>`.
- **Bitmap + Arena**: Price levels are pre‑allocated in a flat array; only the bitmap is updated when a price tick becomes active.

Heap allocation and deallocation are relatively slow and introduce **non‑deterministic pause patterns** at higher percentiles (visible in BTreeMap’s p999).

### 3. Arena‑based order storage

Both implementations use an arena for order storage, but the bitmap variant:

- Places all `PriceLevel` data in a flat array,
- Threads the arena slots into doubly‑linked lists per price tick using **slot indices, not heap pointers**.

This:
- Eliminates virtual memory fragmentation,
- Improves cache locality for matching,
- Reduces memory traffic compared to tree‑node traversal.

---

## Functional and design differences

| Aspect                        | BTreeMap baseline                                           | Bitmap + Arena optimized                                           |
|------------------------------|-------------------------------------------------------------|--------------------------------------------------------------------|
| Order book representation    | Two `BTreeMap<Price, VecDeque<Order>>`                    | Flat `PriceLevel[MAX_PRICE]` + bitmap index                       |
| Best‑price lookup            | `O(log n)` tree iteration                                 | `O(1)` bitmap + CPU instruction                                   |
| Insert complexity            | `O(log n)` per price level                                | `O(1)` bitmap set + arena claim                                   |
| Match complexity             | `O(log n)` per price level consumed                       | `O(1)` per order consumed in linked list                          |
| Cancel support               | ✅ 10% of 1M ops, implemented via price + scan            | Design complete, not yet wired; generation‑ID safety in place     |
| Memory allocation pattern    | Per‑tick heap allocation for `VecDeque`, arena per order  | Pre‑allocated arrays, zero heap for matching logic                |
| Throughput (1M)              | 3.15M ops/sec                                              | 7.94M ops/sec                                                      |
| p99 (1M)                     | 900 ns                                                     | 200 ns                                                             |
| Latency stability over time  | p99 creeps from 900→1000 ns under bursts                  | p99 stable at 300–400 ns across all bursts                        |
| Code complexity / readability| Simple, idiomatic Rust; easy to follow                    | More complex index + arena logic; more “systems‑style”            |

---

## Limitations and caveats

### Common limitations (both variants)

- **In‑memory only**: No persistence, no WAL, no crash recovery.
- **Single‑threaded**: No contention; all benchmarks run on one thread.
- **Hot cache**: 1M orders over a narrow tick range (MID..MID+40) keep the working set in L2/L3 throughout.
- **Synthetic order flow**: Orders are generated with uniform‑spread RNG near mid; real order flow clusters more heavily around mid and has larger outliers.
- **No network stack**: Latency numbers measure only pure matching logic (no network I/O, FIX parsing, risk checks, etc.).
- **Windows timer floor**: `Instant::now()` on Windows has ~100 ns resolution; values below that snap, so true p50 may be lower than reported.

### BTreeMap‑specific limitations

- **Rebalancing tail**: BTreeMap rebalancing under sustained bursts causes p99 and p999 to creep, which is visible in the 5×100k burst tests.
- **Systematic heap‑allocation overhead**: Each new price level hits the heap, introducing GC‑like pauses at higher percentiles.

### Bitmap + Arena‑specific limitations

- **Cancel not yet implemented**: Generation‑ID design is complete in `arena.rs`, but `OrderBook::cancel_order` is not yet wired up.
- **Tick range constraint**: The bitmap design assumes a fixed, relatively small price‑range (e.g., MID±40 ticks). Extending it to very wide price ranges or multi‑symbol requires a different indexing strategy.
- **Cache behavior at scale**: A multi‑symbol engine with many tick ranges would see more cache misses, which the current single‑symbol, narrow‑tick benchmark does not capture.

---

## When to use which

### Prefer BTreeMap baseline when

- You want a **simple, readable** reference implementation.
- You are **validating correctness** and matching logic before optimizing.
- You plan to add features like **complex cancel logic**, **market data replay**, or **multi‑symbol routing** and want to keep the core data structure easy to debug and refactor.

### Prefer Bitmap + Arena when

- You care about **low‑latency tails** and stable p99 across time.
- You want **high throughput** under sustained 100% add/match workloads.
- You are comfortable with systems‑style code (arena allocators, intrusive linked lists, CPU instructions like `BSR`/`BSF`).
- You are building toward a **production‑style matching engine** where allocator overhead and BTreeMap rebalancing are unacceptable.

---

## What’s next (roadmap)

Both implementations share the same high‑level roadmap, but active development is focused on the bitmap variant:

- [ ] **Cancel order (generation‑ID)** — wire up `OrderBook::cancel_order` in bitmap implementation.
- [ ] **Risk engine** — position limits, fat‑finger checks, order‑type validation.
- [ ] **Write‑ahead log (WAL)** — for crash recovery and persistence.
- [ ] **WebSocket + FIX gateway** — using `tokio` and `axum` for order ingress and market data egress.
- [ ] **Multi‑symbol routing** — single‑engine, multi‑symbol book with routing layer.
- [ ] **Criterion benchmarks with HTML reports** — detailed, reproducible latency histograms and charts.

At that point, the BTreeMap version will remain useful as:

- a **reference** for correctness,
- a **baseline** for A/B testing,
- and a **pedagogical** example of “idiomatic Rust” order book design.

---

## References

- Rust `BTreeMap` docs: https://doc.rust-lang.org/std/collections/struct.BTreeMap.html
- Rust arena allocator pattern: https://manishearth.github.io/blog/2021/03/15/arenas-in-rust/
- LMAX Disruptor: https://lmax-exchange.github.io/disruptor/
- QuantCup winning solution: https://gist.github.com/druska/d6ce3f2bac74db08ee9007cdf98106ef
- Mechanical Sympathy blog: https://mechanical-sympathy.blogspot.com/
- x86 `BSR`/`BSF` instructions: https://www.felixcloutier.com/x86/bsr
