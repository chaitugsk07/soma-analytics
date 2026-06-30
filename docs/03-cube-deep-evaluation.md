# soma-analytics — Cube Deep Evaluation (from source)

> Hands-on evaluation of Cube's full feature set, read from the **cloned source** at `99_ref/cube` (v1.6.x, Apache-2.0) — not just docs. Eight agents studied one subsystem each (semantic model, caching, pre-aggregations/Cube Store, drivers/connections, query+embedding APIs, orchestration/multitenancy, AI), verifying claims against actual files. Goal: decide what soma-analytics copies, when, and how — given all-Rust + one shared Postgres (audit-first) + soma-infra.

---

## 1. Headline verdict

**Studying the source confirms the build-vs-buy call — and strengthens it. No subsystem flips it.**

- **Tesseract** (Cube's Rust SQL planner, `rust/cube/cubesqlplanner`) is a **Neon N-API addon loaded into the Node process** — it cannot be imported into a pure-Rust binary, and the model evaluator that feeds it is Node-only. Build `soma-semantic` from scratch. ✅
- **Cube Store** (`cubestored`, `rust/cubestore`) is a standalone Rust binary, but **driving its pre-agg builds/refresh requires the Node.js `cubejs-query-orchestrator`** — running it pulls the whole Node runtime back in. ❌ for us.
- **cubesql** (the Postgres-wire proxy) is coupled to Cube's internal `MetaContext`, its private DataFusion + `egg` forks, and the Cube REST API as backend — not embeddable. Build our own thin `soma-pgwire` instead (much simpler — we only translate a SELECT subset to a `SemanticQuery`). ❌ as a lib.

**The important reframe (this is the answer to your DuckDB/point-in-time idea):** Cube's "scheduled-refresh → fast columnar store → instant reports" is **not one monolith** — it decomposes into two separable concerns, both of which we build in Rust with **no external process**:
1. a **point-in-time snapshot store** → **Postgres materialized views** first, a **pure-Rust embedded columnar engine (DataFusion/Polars; DuckDB as C++ fallback)** later;
2. a **refresh scheduler** → a **Tokio background task**, not Cube's Node `RefreshScheduler`.

So you get the capability you want; you just don't take Cube's mechanism to get it.

---

## 2. The point-in-time store for fast reports (your core question)

How soma-analytics delivers "scheduled rollups → super-fast report inference":

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **Postgres `MATERIALIZED VIEW` rollups** (`REFRESH … CONCURRENTLY`) | Zero new deps; same sqlx pool + shared Postgres we already own; full SQL compat (an MV is just a table to the planner); concurrent refresh serves the prior snapshot while rebuilding; the aggregate-match routing is a **pure function** we can write now at zero cost | Row-oriented (columnar scans slower than Parquet); `CONCURRENTLY` needs a unique index + full rewrite (no incremental partitions); no S3/Parquet tiering | ✅ **Phase 2** |
| **Embedded pure-Rust columnar engine (Apache DataFusion / Polars)** | True columnar/vectorized over Arrow/Parquet snapshots; sub-ms scans of aggregated rollups (small, fit in memory); single-process, no sidecar; **pure Rust, no FFI/allocator issues, fits the all-Rust value**; DataFusion is battle-tested (Cube Store/GlareDB/InfluxDB 3 build on it). DuckDB (C++ via the `duckdb` FFI crate) remains a fallback if a mature larger-than-memory engine is ever required — but it's a C++ dependency (~60 MB, jemalloc allocator care), so not the default. | Less turnkey than DuckDB (assemble more pieces); larger-than-memory support weaker than DuckDB; you maintain the snapshot load/refresh path; single-writer (Tokio `RwLock`) | 🔭 **Phase 3, only if measured** |
| **`cubestored` standalone** | Purpose-built columnar store; Parquet partitioning, incremental windows, rollup_join | **Requires the Node.js orchestrator** to drive builds/refresh → reintroduces the entire Node runtime; separate process + blob store + MySQL-wire client | ❌ **Never** (kills single-runtime) |

**Recommendation:** **Phase 2 = Postgres materialized views** + a Tokio refresh scheduler + **aggregate-awareness routing** (a query that matches a registered rollup is transparently rewritten to `SELECT … FROM mat_view_x`, no API change). Write the `rollup_match(...) -> Option<MatViewName>` pure function **now** in `soma-semantic` — it returns `None` (no rollups registered), costs nothing, and unit-tests the dormant path. **Phase 3 = a pure-Rust embedded engine (DataFusion or Polars)** only when an MV's rollup query is a *demonstrated, measured* latency bottleneck (proposed trigger: rollup MV p95 > ~200 ms under normal load) — not speculatively. DuckDB (C++ via the `duckdb` FFI crate) is the fallback if a mature larger-than-memory engine is ever needed. **Role note:** DataFusion was correctly rejected as a *live source-Postgres* engine (it pulls whole tables in-process, no pushdown) — but it is the *right* tool for querying *already-materialized rollup snapshots* (different role, opposite verdict).

---

## 3. Caching upgrades worth copying (beyond our TTL cache)

Our Phase-1 Redis result cache (key = `sha256(query + tenant + row_filters + cube.model_version)`, per-cube TTL) is correct. Cube's model adds three upgrades, in priority order:

1. **`refreshKey`/datagroup probe (Phase 2.5)** — the big one. A cheap staleness probe (e.g. `SELECT MAX(updated_at) FROM soma_audit."…aud_events"`) re-run at a short independent interval (~30 s); serve stale **only if the probe value is unchanged**. Converts us from TTL-only to **data-event-driven** invalidation, without Redis key-pattern scanning. Store as `refresh_key_sql` on `03_fct_cubes` (probe value cached at `saq:v1:rk:<cube_id>`).
2. **Stale-while-revalidate via Tokio** — after serving a hit, fire the probe + optional re-execute in a `tokio::spawn`; write-new-before-delete-old to avoid a miss window. Keeps p50 flat while staying fresh.
3. **Cache modes** — optional `cache_mode` on `SemanticQuery` (`stale-while-revalidate | must-revalidate | no-cache`) for pinned dashboards vs ad-hoc exploration.
- **Skip** an L1 in-process LRU — localhost Redis round-trips (~0.1 ms) make it marginal; revisit only if profiling shows it.
- (Note: Cube *removed* Redis from its own source; only memory + Cube Store backends remain. Our Redis use via soma-infra is independent and fine.)

---

## 4. Live vs scheduled queries — phase mapping

- **Phase 1 = live only** (compile + execute on the read-only Postgres pool per request), accelerated by the Redis TTL cache. Correct for audit-scale data and exploration/low-frequency dashboards.
- **Trigger for scheduled/pre-agg:** a rollup that exceeds ~500 ms even cache-warm, or an explicit "scheduled report" (fixed end-of-day/weekly snapshot where speed matters and staleness is fine).
- **Phase 2** = scheduled Postgres MV rollups + aggregate-awareness routing. **Phase 2.5** = refreshKey probe. **Phase 3** = a pure-Rust columnar engine (DataFusion/Polars) for snapshot queries (DuckDB as C++ fallback) + the **lambda pattern** (sealed history MV/snapshot `UNION` a live trailing-window tail) for near-real-time on large data — all buildable in `soma-semantic`, no `cubestored`.

---

## 5. Connections / drivers

- **Phase 1 (now):** one read-only `sqlx` `PgPool` over the shared Postgres (audit schema), `after_connect` sets `timezone=UTC`. Metadata pool (`DATABASE_URL`) is the second pool.
- **Topology answer (you confirmed shared Postgres):** a **single READ_USER pool already reaches all schemas** (`soma_audit`, `soma_iam`, …) cross-schema — **no pool-per-source registry is needed** for cross-service reports. `02_fct_data_sources` becomes a *routing label*, not a DSN store. (A pool-per-source registry only returns if a non-soma external warehouse is added later.)
- **Phase 2:** `crates/soma-pgwire` (the `pgwire` crate) — auth maps PG user/password → verified API key → `QueryScope`, translates a safe SELECT subset to `SemanticQuery`. **Unlocks Superset, Metabase, Grafana, Tableau with zero client changes.** Far simpler than `cubesql` (we don't need general pushdown).
- **DuckDB driver (Cube's driver):** not used as soma's embedded Phase-3 engine — soma's embedded engine choice is pure-Rust DataFusion/Polars (§2); DuckDB C++ is a fallback only. Cube's DuckDB *source driver* (a separate concept) is a real feature of cubestored but is not relevant to soma's internal routing. Routing stays internal to `soma-storage`; no new API surface.

---

## 6. Feature catalog (Cube → soma-analytics)

| Cube feature | OSS? | soma need | Phase / mapping |
|---|---|---|---|
| Semantic model + SQL compilation (schema-compiler / Tesseract) | Yes (Tesseract preview, off by default) | **Must — built from scratch** (`soma-semantic`); Tesseract is the *design* reference (MemberSymbol/JoinTree/CTE chain), not importable | Phase 1 |
| Query result cache (2-tier LRU + backend, refreshKey) | Yes | **Valuable** — refreshKey probe is the named freshness upgrade | Phase 1 (TTL) → 2.5 (probe) |
| Pre-aggregations + Cube Store | Yes | **Valuable concept** — deliver via Postgres MV (Phase 2), pure-Rust DataFusion/Polars engine (Phase 3; DuckDB as C++ fallback) | not via cubestored. NB: soma's embedded ENGINE choice is pure-Rust DataFusion/Polars, not DuckDB — see §2. |
| Driver model / connection types | Yes | **Must conceptually** — informs the `DataSourceDriver` trait | Phase 1 (1 pool) → 1.5 (multi only if external) |
| REST / GraphQL / **SQL (pgwire)** APIs | Yes | **Must** REST+meta; **valuable** pgwire | Phase 1 (REST/meta) → 2 (pgwire) |
| JWT securityContext + member-level access policies + `queryRewrite` | Core (`userAttributes` is Cloud-only) | **Must** — request-layer row-filter injection now; member visibility later | Phase 1 (row filters) → 1.5/2 (member policy) |
| Orchestration / RefreshScheduler / multitenancy / queue | Yes | **Valuable** — the round-robin + backoff refresh algorithm informs our Tokio scheduler | Phase 2 |
| AI: semantic grounding, certified queries, evals, MCP, SQL-API planning | Grounding/certified concepts OSS; **Agent/Chat/MCP/evals are Cloud-only** | **Must** (our own AI seam) — copy the grounding + certified-query few-shot pattern; build MCP ourselves if wanted | Phase 1.5 (NL→query) → 2 (evals, MCP) |
| Cube Store standalone (`cubestored`) | Yes | **Skip** — needs the Node orchestrator | replaced by MV + pure-Rust DataFusion/Polars (DuckDB as fallback) |
| Cloud-only (signed embedding, lineage Metadata API, DAX, Semantic Layer Sync, Analytics Chat, BYOM) | No | Borrow the **lineage-enriched meta** shape concept; skip the rest | Phase 2 (meta lineage) |

---

## 7. Build-vs-buy: confirmed, with the source-level "why"

No reversals. Specifically: (1) Tesseract = Neon-bound Node addon, model evaluator is Node — not a library. (2) `cubestored` runs standalone but **only the Node orchestrator can drive its builds** — using it imports Node. (3) `cubesql` is bound to internal `MetaContext` + private DataFusion/`egg` forks + the Cube REST backend. (4) The single nuance — the *cubestored concept* (scheduled columnar rollups) **is** what you want, and it's achievable in pure Rust (MV + pure-Rust DataFusion/Polars snapshot store + Tokio scheduler; DuckDB as C++ fallback) with far less overhead. **Build-not-buy on the mechanism; adopt the concept.**

---

## 8. Updated phased roadmap

**Phase 1 (current target):** `soma-semantic` compiler (single-cube + one-hop join, two-leg fan-out, all `CompileError`s, `validate_sql_fragment`); `soma-storage` (metadata CRUD + read-through Redis TTL cache + `model_version` invalidation + single read-only audit pool); `soma-api` (`POST /query`, `GET /meta`, `POST /embed/token`, model + dashboard/panel CRUD, flag-gated `POST /ai/query`); `soma-server` boot (two pools, CORS); `soma-sdk` + `soma-cli` + `@soma-analytics/client`; **soma-ui** `<AnalyticsPanel>`/`<Dashboard>` over existing charts; `soma_infra::crypto::hmac_sha256_verify` (the one new primitive). **Cheap seams to add now (near-zero cost, avoid future migrations):** `description` column on dimensions/measures (powers `/meta`); a dormant `rollup_match()` pure fn returning `None`. *(`ai_hint` column + `certified_queries` table are proposed but optional — see §10.)*

**Phase 1.5:** activate `POST /ai/query` (`soma_infra::llm` + forced structured `SemanticQuery` output → through `compile()` gate, no raw-SQL fallback); AI panel generation; populate `/meta` with `description`/`ai_hint`; member-visibility (`member_visible` flag enforced in `/meta`); embed-token hardening (`jti` + Redis revocation set). Multi-source **only if** a non-soma warehouse appears (shared Postgres already covers cross-service).

**Phase 2:** Postgres MV rollups + Tokio refresh scheduler + aggregate-awareness routing; refreshKey probe + stale-while-revalidate + cache modes; `crates/soma-pgwire` (BI tools); member-level access policy table; lineage-enriched `/meta`; NL→query eval harness (execution-based, BIRD-style); lightweight MCP server (`list_cubes` + `run_query`); real soma-iam integration (swap `LocalTokenVerifier`).

**Phase 3:** pure-Rust embedded columnar engine (DataFusion or Polars; DuckDB C++ FFI as fallback, only if measured-insufficient MV latency demands larger-than-memory support) + lambda pre-agg (Tokio RwLock single-writer for snapshot refresh still applies); multi-turn AI memory; multi-warehouse dialect drivers; symmetric aggregates (if two-leg insufficient); WebSocket subscriptions (probe-driven); Cube-style member-curated **Views**; the **ontology layer** (cubes over `soma_infra::kg` nodes/edges + pgvector; write-back Action trait; scenarios) — the Palantir trajectory.

---

## 9. Open decisions (new, from this evaluation)

These are in addition to the spec §15 list (advisory key; tenant UUID — both still recommended as-is). The two big platform questions you raised are now **resolved**: Phase-1 source = **soma-audit** ✅, topology = **one shared Postgres** ✅ (→ single cross-schema read-only pool, no multi-source registry).

1. **pgwire timing** — pull the Postgres-wire SQL API into **Phase 1.5** (unlocks Grafana/Superset/Metabase/Tableau immediately, which matches your "like Grafana" goal) or keep it Phase 2 (after MV routing, so BI clients get pre-agg acceleration)?
2. **Embedded-engine trigger + choice** — accept the proposed explicit threshold (rollup MV p95 > ~200 ms ⇒ evaluate embedded engine) so the Phase-3 decision is data-driven, not vibes? When the trigger fires: pick DataFusion vs Polars vs (fallback) DuckDB based on measured need — prefer pure-Rust DataFusion/Polars (no FFI, no allocator conflict); reach for DuckDB only if larger-than-memory scans require its maturity.
3. **refreshKey probe shape** — user-supplied raw probe SQL (flexible, new trust surface) vs a structured probe (`{table, column, aggregate}` compiled by us, no injection surface)? Recommend structured.
4. **Cheap-seam ponytail call** — add the dormant `certified_queries` table + `ai_hint` column in the Phase-1 migration (avoids a later migration-order dependency) or defer to Phase 1.5 (stricter YAGNI)? Recommend: add `description` now (used in Phase 1); defer `ai_hint`/`certified_queries` to the Phase-1.5 migration.
5. **MCP server** — appetite for a small soma MCP server (Phase 2) so Claude Code / external agents can query governed analytics directly?
