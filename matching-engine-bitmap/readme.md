# Matching Engine — Bitmap Implementation

A single-symbol limit order book and matching engine built in Rust using a bitmap price index + arena allocator as the core data structure.
This is the optimized variant in the **matching-engine-rs** series.
For the baseline, see → [matching-engine-btreemap](https://github.com/Rahulkumar20023/matching-engine-bitmap/blob/matching-engine-btreemap)

> ⚠️ **In-Memory Only** — all order state lives in process memory. There is no persistence, no WAL, no crash recovery. A process restart loses all open orders and book state.

---

## Architecture

    OrderBook
    ├── bitmap/                    ← price presence index (BSR/BSF = O(1) best bid/ask)
    │   └── u64 words[N]
    ├── orderbook/
    │   ├── PriceLevel[MAX_PRICE]  ← flat array, one slot per price tick
    │   │   └── head / tail / count / total_qty
    │   ├── Arena
    │   │   ├── Slot[]             ← slab: Occupied { order, prev, next } | Free { next_free }
    │   │   ├── generation[]       ← per-slot generation counter, survives free/alloc cycles
    │   │   └── free_head          ← intrusive free list
    │   ├── OrderId                ← { index: u32, generation: u32 } — ABA-safe handle
    │   └── OrderBook              ← matching + cancel logic
    └── main.rs                    ← benchmark suite + 1M simulation

---

## How It Works

Orders are stored in a flat arena slab indexed by slot number. Each price level is a doubly-linked list threaded through the arena using slot indices — no heap pointers, no indirection. The bitmap tracks which price ticks have resting orders; finding the best bid or ask is a single LZCNT/TZCNT CPU instruction.

| Operation    | Complexity    | Notes                                                      |
|--------------|---------------|------------------------------------------------------------|
| Insert       | O(1)          | bitmap bit-set + arena slot claim                          |
| Best bid/ask | O(1)          | LZCNT/TZCNT on bitmap word                                 |
| Match        | O(1) per fill | follow arena linked list                                   |
| Cancel       | O(1)          | generation validate → linked list unlink → bitmap clear if last |

---

## Cancel — Generation ID Design

Each arena slot carries a generation: u32 counter alongside its Occupied/Free state. When an order is freed (filled or cancelled), the generation increments. Any OrderId { index, generation } handle whose generation no longer matches the live slot is silently rejected — no corruption, no double-free.

    alloc_order  →  slot generation = G   →  returns OrderId { index, generation: G }
    free_order   →  slot generation = G+1 →  all existing OrderId { gen: G } now stale
    alloc_order  →  slot generation = G+1 →  returns OrderId { index, generation: G+1 }

    cancel_order(OrderId { index: 42, generation: 3 })
      │
      ├─ arena.validate()          → checks generation, confirms Occupied  [O(1)]
      ├─ reads order.price + side  → no extra lookup needed
      ├─ price_to_tick(price)      → tick_idx                              [O(1)]
      └─ book.remove_order(tick_idx, slot_idx)
            ├─ arena.free_order()  → doubly-linked list unlink             [O(1)]
            ├─ generation += 1     → invalidates all stale handles
            └─ if order_count == 0
                  └─ bitmap.clear_bit(tick_idx)                            [O(1)]

Stale handles (order already filled before cancel arrives) return false immediately — no scan, no map lookup.

---

## Why Bitmap + Arena?

BTreeMap's O(log n) insert and heap allocation per price level creates measurable latency under sustained load. A bitmap reduces best-price lookup to a single CPU instruction. An arena eliminates per-order heap allocation entirely — all memory is claimed at startup, freed to a slab, never returned to the OS.

---

## Benchmark Results

**Environment:** Windows 11, Intel Core i5-12450H, Rust --release
**Measurement:** Per-operation Instant::now() with std::hint::black_box
**Unit:** nanoseconds (ns)

### Latency Percentiles

| Workload                                              | mean  | p50   | p90   | p99   | p999    | max   |
|-------------------------------------------------------|-------|-------|-------|-------|---------|-------|
| passive_add                                           | 73ns  | 100ns | 100ns | 300ns | 1,400ns | 130µs |
| mixed_workload (60% passive / 30% aggressive / 10% cancel) | 96ns  | 100ns | 100ns | 300ns | 700ns   | 253µs |
| burst_load overall                                    | 86ns  | 100ns | 100ns | 300ns | 1,400ns | 333µs |
| 1M simulation (70% normal / 20% aggressive / 10% cancel)   | 117ns | 100ns | 200ns | 600ns | 1,600ns | 210µs |

### 1M Order Simulation

    ── ORDER FLOW ──────────────────────────────────────
      Total ops        :    1,000,000
      Buy orders       :      450,060  (45.0%)
      Sell orders      :      450,020  (45.0%)
      Matched          :      401,504  (40.2%)
      Passive          :      498,576  (49.9%)
      Cancelled        :       99,920  (10.0%)
      Rejected         :            0  (0.0%)

    ── THROUGHPUT ──────────────────────────────────────
      Wall time        :      165.2ms
      Throughput       :    6,055,039  ops/sec

    ── LATENCY PERCENTILES ─────────────────────────────
      mean :   117ns
      p50  :   100ns
      p99  :   600ns
      p999 : 1,600ns
      max  :   210µs  ← OS scheduler spike

### Burst Load (5 × 100k orders)

    burst  1: p50=100ns  p99=200ns  max= 84µs
    burst  2: p50=100ns  p99=300ns  max=333µs
    burst  3: p50=100ns  p99=300ns  max= 81µs
    burst  4: p50=100ns  p99=300ns  max= 52µs
    burst  5: p50=100ns  p99=300ns  max= 57µs

p99 stays flat at 200–300ns across all 5 bursts — no rebalancing accumulation under sustained load.

### Latency Distribution (1M simulation)

    <100ns   23.25%  ███████████
    <500ns   73.79%  ████████████████████████████████████
     <1µs     2.66%  █
     <2µs     0.23%
     <5µs     0.04%
    <10µs     0.02%
    <50µs     0.01%
    ≥100µs    0.00%

97.04% of all operations complete under 1µs.

### Latency Over Time (per 100k batch)

           batch   p50(ns)   p99(ns)   max(ns)
            100k       100       500    210,700
            200k       100       500    162,400
            300k       100       700    131,400
            400k       100       700     19,300
            500k       100       600     28,300
            600k       100       700     39,500
            700k       100       600     13,800
            800k       100       500      6,200
            900k       100       500     80,200
           1000k       100       500     13,600

p99 stable across all 10 batches — no degradation as book depth grows.

---

## Known Limitations

**1. Windows Timer Resolution Floor**
Instant::now() on Windows has ~100ns resolution. Operations faster than 100ns snap to 100ns. True p50 may be lower than reported. Linux TSC gives ~1ns resolution for accurate sub-100ns measurement.

**2. In-Process Measurement Only**
Numbers reflect pure matching logic. Not included: network I/O, TCP stack, FIX parsing, risk checks, persistence, or market data dissemination. A production system adds 10–50µs on top.

**3. Single-Threaded**
No contention. Real concurrent order ingestion would raise p999 depending on queue design (SPSC, MPSC, or Disruptor pattern).

**4. Hot Cache**
1M orders over MID..MID+40 ticks keeps the working set in L2/L3 throughout the run. A multi-symbol engine with a wider price range would see higher latency from cache misses.

**5. Synthetic Order Distribution**
Orders are generated with a uniform-spread RNG. Real order flow clusters more heavily around mid-price with occasional large outlier orders.

---

## Folder Structure

    matching-engine-bitmap/
    ├── Cargo.toml
    ├── README.md
    └── src/
        ├── main.rs              — benchmark suite + 1M simulation
        ├── bitmap/
        │   └── bitmap.rs        — 3-level bitmap (LZCNT/TZCNT best bid/ask)
        └── orderbook/
            ├── order.rs         — Order, Side, OrderType, TimeInForce
            ├── order_id.rs      — OrderId { index: u32, generation: u32 }
            ├── slot.rs          — Slot { generation, state: Occupied | Free }
            ├── arena.rs         — Arena allocator with generation validation
            ├── pricelevel.rs    — PriceLevel { head, tail, order_count, total_qty }
            ├── BuyBook.rs       — BidBook (best_bid via bitmap MSB)
            ├── AskBook.rs       — AskBook (best_ask via bitmap LSB)
            └── orderbook.rs     — OrderBook: add_limit_order, cancel_order, match_orders

---

## Running

    # Clone
    git clone https://github.com/YOUR_USERNAME/matching-engine-bitmap
    cd matching-engine-bitmap

    # Run full benchmark suite
    cargo test --release -- --nocapture

    # Run 1M simulation only
    cargo test simulate_1m_orders --release -- --nocapture

    # Run mixed workload (cancel included)
    cargo test bench_mixed_workload --release -- --nocapture

---

## What's Next

- [x] Cancel order — generation-ID based, O(1), ABA-safe
- [ ] Risk engine (position limits, fat-finger checks, order-type validation)
- [ ] Write-ahead log (WAL) for crash recovery and persistence
- [ ] WebSocket gateway (tokio + axum) for order ingress and market data egress
- [ ] Multi-symbol routing — single engine, multiple order books
- [ ] Criterion benchmarks with HTML latency histogram reports

---

## References

- [Rust Arena Allocator Pattern](https://manishearth.github.io/blog/2021/03/15/arenas-in-rust/)
- [LMAX Disruptor](https://lmax-exchange.github.io/disruptor/)
- [QuantCup Winning Solution](https://gist.github.com/druska/d6ce3f2bac74db08ee9007cdf98106ef)
- [Mechanical Sympathy Blog](https://mechanical-sympathy.blogspot.com/)
- [x86 BSR/BSF Instructions](https://www.felixcloutier.com/x86/bsr)
