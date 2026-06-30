# soma-analytics — Part A: Competitor Analysis & Recommendations

> Research deliverable for the AI-native semantic layer. Phase-1 target: a **minimum lovable semantic layer** — define a governed data model, compile a semantic query to SQL, execute it, embed the results in other products. Long-term trajectory: a governed ontology + operational analytics platform (the Palantir Foundry direction).
>
> Sources: official docs for Cube, dbt/MetricFlow, Looker/LookML, Malloy, AtScale, Palantir Foundry; Spider 1.0/2.0 and Cube's semantic-layer-for-AI benchmark for the AI thesis. Load-bearing claims in this document were independently fact-checked in a separate verification pass.

---

## 1. The landscape in one paragraph

Every serious semantic layer makes the same core bet: **define metrics and dimensions once over your tables, then compile user queries down to warehouse-native SQL** rather than executing analytics itself. Cube, dbt/MetricFlow, LookML, Malloy, and AtScale are all *compile-and-pushdown* engines — they generate SQL and let the database do the work. They differ mainly in (a) the modeling syntax, (b) how they keep multi-table aggregations correct across joins (the "fan-out" problem), and (c) their materialization/caching strategy for speed. Palantir Foundry is the outlier and the eventual competitor: its **Ontology** is not a read-only metrics layer at all — it is an operational object graph with **write-back Actions**, so it closes the read→decide→act loop that pure metrics layers cannot.

---

## 2. Comparison table

| Tool | Modeling model | Compile / Execute | Caching / Pre-aggregation | Query interfaces | Killer feature |
|---|---|---|---|---|---|
| **Cube.dev** | YAML/JS/Python **cubes** (1 per table/SQL) with measures, dimensions, **joins with declared cardinality** (`many_to_one`/`one_to_many`/`one_to_one`), segments; **views** as a governance facade; `extends` inheritance | Compiles model → warehouse-native SQL and **pushes down**. SQL API path uses Apache DataFusion + `egg` to plan. Three execution strategies: pre-agg hit, decomposed sub-queries post-processed in-memory, or full pushdown. **Runtime: Node.js** (Cube Store + SQL API are Rust) | **Two layers**: short-lived in-memory result cache (keyed on query + securityContext) and **pre-aggregations** — rollups materialized as Parquet in **Cube Store** (Rust columnar engine), incremental + time-partitioned. Both in OSS | REST/JSON, GraphQL, **Postgres-wire SQL API**, WebSocket. MCP server + DAX are Cube-Cloud-only | **Pre-aggregations**: declare a rollup in ~5 lines of YAML; Cube builds Parquet partitions and auto-routes matching queries to them. Sub-second latency, zero warehouse changes |
| **dbt SL / MetricFlow** | YAML **semantic models** (entities with typed keys = cardinality, dimensions incl. time/SCD2, measures) + separate **metrics** (simple/ratio/derived/cumulative/conversion); **saved queries** | Pure compile-to-warehouse SQL (**Python**). Builds a dataflow DAG, resolves join paths through the **typed entity graph** (structurally prevents fan-out & chasm joins), emits one SQL string | Relies on the warehouse's own query cache. Declarative caching is Enterprise-only. **No Cube-style pre-agg engine in OSS** | JDBC/ADBC (Arrow Flight), GraphQL, Python SDK, partner connectors — **all dbt-Cloud-gated**. OSS CLI has no served API | **Typed entity join graph** — the planner *refuses* unsafe joins, making multi-table aggregation silently-wrong-proof |
| **LookML (Looker)** | 3-file DSL: model (connection+explores), view (table binding, dimensions, measures, `primary_key`), explore (join graph w/ cardinality + `sql_on`) | Pure compile-to-warehouse SQL; Looker never holds data. Dialect translation per connection | Query-result cache with **datagroup** invalidation (sentinel SQL / interval); **PDTs** (persistent derived tables) materialized in a scratch schema, swap-before-drop | REST API 4.0, **Open SQL Interface (JDBC)**, **HMAC-signed embed URLs** + Embed SDK (postMessage), Gemini conversational analytics | **Symmetric aggregates** — auto-rewrites `SUM`/`COUNT` across fan-out joins via MD5-hash dedup arithmetic, triggered by declared `primary_key` + relationship |
| **Malloy** | Code-first `.malloy`: `source:` pairs table with dimensions/measures (incl. **filtered measures**), `join_one`/`join_many` (explicit cardinality), `view:` saved patterns; **model = query language** | Pure compile-to-SQL (TypeScript/ANTLR, MIT). Dialects: BigQuery, Snowflake, Postgres, DuckDB, Databricks, Trino. `nest:` → aggregating subqueries in one round-trip with auto fan-out correction | None built-in; relies on warehouse cache | `.malloy` files primary; Publisher server (Node, OSS) exposes REST + MCP; React SDK; VS Code ext. No JDBC/MDX | **Symmetric nested aggregation** (`nest:`) in a single SQL round-trip + model/query symmetry → zero-friction reuse |
| **AtScale** | YAML **SML** (Apache-licensed since 2024) in Git: datasets, dimensions w/ hierarchies, additive/semi-additive/distinct measures, M2M bridges, cubes. Pure metadata | **Virtual proxy**: intercepts MDX/DAX/SQL, resolves to SML, checks aggregate catalog, generates pushdown SQL. All compute in the warehouse | **Autonomous aggregates**: monitors real query patterns, materializes rollup tables **inside the user's warehouse** (demand-driven/user-defined/hinted), incremental w/ watermarks | **XMLA/MDX** (Excel), **Postgres wire** (JDBC/ODBC), **DAX/Tabular** (Power BI DirectQuery), REST, Sheets | **Autonomous aggregate awareness** — sub-second BI on billion-row tables with no manual MV management, across MDX/DAX/SQL |
| **Palantir Foundry Ontology** | 4 layers: **Object Types**, **Link Types** (named graph edges w/ cardinality), **Action Types** (mutation schemas w/ validation + side effects), **Functions** (TS/Python). Interfaces for polymorphism. No SQL DDL | **Does NOT compile to SQL/pushdown.** Own Object Storage V2 (indexed objects), reads via Object Set Service + Spark aggregation. Foundry *is* the store | Incremental indexing as continuous materialization; Spark aggregations; embedded/offline sync. No explicit rollups | REST v1/v2, **OSDK** (code-genned typed clients), Workshop iframe embedding, WASM offline Ontology | **Actions = atomic write-back** to the Ontology and source systems → closes read→analyze→decide→act. + Scenarios (what-if) + AIP agents. No pure metrics layer has this |

**The one correctness idea every engine agrees on:** *declare join cardinality on every join.* It's the input that lets the planner avoid fan-out (double-counting when a one→many join multiplies fact rows). Cube, MetricFlow, LookML, and Malloy all require it. soma-analytics will too.

---

## 3. Recommendation 1 (the hardest decision): the semantic compiler — **build a minimal Rust compiler**

Three options were on the table:

| Option | What it is | Verdict |
|---|---|---|
| **(a) Wrap Cube.dev OSS** | Operate Cube's Node.js stack beside the Rust services | ❌ Rejected |
| **(b) Build a minimal Rust compiler** | Parse a declarative model → generate parameterized SQL → execute via `sqlx` | ✅ **Chosen for Phase 1** |
| **(c) Build on Apache DataFusion** | Embed a Rust query engine as the execution/optimization layer | ❌ Rejected (for Phase 1) |

### Why (b)

- **Single-runtime is a load-bearing platform value, not a preference.** `soma-platform/CLAUDE.md` makes "all-Rust, consume soma-infra" non-negotiable. Wrapping Cube means running its **~5-process Node deployment** (API instances, Refresh Worker, Cube Store router + workers, 3–8 GB RAM per process type) and a second language runtime to monitor and upgrade beside every Rust service. Phase 1 needs none of that.
- **Phase-1 scope is genuinely small.** One configured Postgres source, a governed model, query→SQL compilation, an embed-ready REST API with scoped row-level security. That is a few hundred lines of Rust: `serde`-deserialize the model registry → validate `measures[]`/`dimensions[]` against the whitelist → emit **parameterized** SQL via `sqlx::QueryBuilder` with bind variables → execute on the existing soma-infra pool → cache results in soma-infra Redis.
- **The fan-out trap has a cheap, correct Phase-1 answer.** Copy Cube's own multi-fact strategy: when a query spans two fact tables, emit **two sub-SELECTs aggregated independently and JOIN them on shared dimension keys** ("two-leg" pattern) instead of one big join. This is well-understood SQL — correct results without implementing LookML-style symmetric-aggregate (MD5 dedup) math, which is deferred.
- **DataFusion is the wrong tool when data already lives in Postgres.** DataFusion executes **in-process over Arrow** — by default it would pull whole Postgres tables into the service to aggregate them, a catastrophic regression for OLAP over a database that already has a capable engine. The `datafusion-table-providers` + `datafusion-federation` crates that enable real pushdown are **alpha-tier**; betting Phase 1 on them is unjustified. (DataFusion becomes interesting in Phase 3 *if* multi-source federation is ever needed.)
- **The industry is validating Rust here.** Cube's own next-gen engine (Tesseract) is a Rust rewrite — directional evidence that Rust is the long-term-correct substrate. We just skip the Node baggage that still ships around it today.

### The honest risks (and mitigations)

| Risk | Mitigation |
|---|---|
| Fan-out edge cases (3-way fan-out, mixed additive/non-additive measures) are silently-wrong, not just slow | Require a `relationship` cardinality on **every** join; compiler **refuses to load** a model with an undeclared join cardinality. One aggregate-value integration test per join path against a Postgres fixture before shipping |
| No pre-aggregation → every query hits Postgres live; large tables may exceed UI latency budgets | Phase 2 adds Postgres `MATERIALIZED VIEW` rollups + aggregate-awareness in the compiler. Phase 1: per-cube result cache + TTL |
| No Postgres-wire API → BI tools (Tableau/Superset) can't connect as if to a DB | Phase 2 adds a `pgwire`-crate facade exposing the same query contract. Phase 1 embedding is REST + JS client |
| Model evolution (renaming a measure) breaks embedded consumers | Carry a `model_version` field from day one; deprecation warning for one cycle before hard-error |
| "Minimal compiler" scope-creeps into a multi-month project | Isolate the compiler in a pure `crates/soma-semantic` crate (no I/O), hard-bound to: measures, dimensions, filters, segments, time-grain, single-level joins. Everything else is Phase 2 |

### Addendum — "can't we just take Cube's Rust and use it directly?" (investigated against the live repo, June 2026 — verdict: not viable today)

This was investigated explicitly because Cube *is* mostly Rust and *is* fully open source. Both premises check out — and neither helps, because the Rust isn't the part we'd need.

- ✅ **It's genuinely OSS and forkable.** The whole repo is **Apache-2.0** (a few client packages MIT); no commons-clause, no SSPL/BUSL. Fork, self-host, and modify are all permitted. Cloud-only features (AI analyst, console, SSO, audit) aren't license-gated — they simply don't exist in Core. The SQL API, pre-aggregations, multi-tenancy, and all connectors **are** in the Apache-2.0 Core.
- ✅ **It's ~53–61% Rust** — 17 crates across 3 workspaces (`cubestore`, `cubesql`, `cubesqlplanner`, `cubenativeutils`, `cuberockstore`, …).
- ❌ **But the semantic compiler we'd reuse is Node, not Rust.** Model evaluation — parsing the YAML/JS/Python model, `COMPILE_CONTEXT`, dynamic/multi-tenant schemas, even `sql: () => …` expressions — runs **entirely in a Node.js VM** (`@cubejs-backend/schema-compiler`) and **has not moved to Rust** in any release through **v1.6.64 (Jun 2026)**. SQL generation is Node by default; the Rust planner ("Tesseract" / `cubesqlplanner`) is **opt-in preview** (3 env flags; in preview since Oct 2024) and, even when enabled, **calls back into Node via Neon N-API** to resolve members/joins/expressions.
- ❌ **The Rust crates aren't consumable as libraries.** `cubesqlplanner` is built as a **Node native addon** (`@cubejs-backend/native`, a Neon cdylib): it has no standalone entry point, **none of the crates are on crates.io**, all are versioned `0.1.0` with no stable/semver API, and they pin a **private fork of DataFusion + sqlparser-rs** that conflicts with the upstream ecosystem. `cubestore` (the rollup store) *does* run standalone — but it's a cache engine, useless without the Node orchestrator deciding what to materialize.

**Reuse paths, scored:**

| Path | Feasible today? | Verdict |
|---|---|---|
| (a) Run Cube OSS as-is (Node + Rust, multi-process) | Yes | Low tech risk, **high architectural cost** — a Node sidecar (+ Cube Store process) violates single-runtime. Only worth it if a customer needs pre-aggregations/multi-warehouse *now* |
| (b) Fork & strip to Rust-only | **No** | The Rust is **not separable** — `cubesqlplanner`'s `NativeContext` is only implemented for Neon/Node, and the schema/driver/auth layer is wholly TypeScript. Stripping Node = rewriting the model evaluator in Rust = **same scope as building from scratch**, but inheriting Cube's abstractions + an unstable private DataFusion fork |
| (c) Vendor/embed just `cubesqlplanner` | **No** | Neon-coupled, not published, no stable API, DataFusion fork conflict. `cubenativeutils` *does* define a swappable `InnerTypes`/`NativeContext` trait (a theoretical non-Node door, even a planned `pyo3` path) — but **no non-Neon implementation exists**, and you'd still need to feed it a Rust-native schema source the TS layer currently owns |

**Conclusion:** "use Cube's Rust directly" still drags in the Node.js runtime and preview-grade code. The "necessary changes" to avoid that are ~the same work as the minimal compiler, minus the control and plus a permanent upstream-tracking tax. **The build recommendation holds.** We still *borrow Cube's design* (model shape, member naming, the two-leg fan-out, pre-agg concepts) — Part A §7 — without taking the code.

**Future-flip trigger (revisit then, not before):** all three of — (1) Tesseract reaches **GA with the model evaluator fully in Rust** (no Node in the hot path), (2) `cubesqlplanner` is **published to crates.io with a versioned public API**, and (3) the **DataFusion fork is synced upstream** (or offers a stable vendored ABI). Given Tesseract has sat in preview since Oct 2024, that's plausibly **12–24+ months out**. If it lands, embedding Cube's compiler could beat maintaining our own — re-evaluate at that point.

---

## 4. Recommendation 2: what does the service query? — **one configured Postgres, via a separate pool**

- **Metadata** (the model registry, tokens) lives in the service's **own** Postgres — `soma_infra::connect_from_env()` reading `DATABASE_URL`.
- **The queried data source** is a **separate, service-owned pool** — `soma_infra::db::connect(&PoolConfig::new(ANALYTICS_DB_URL))` reading a distinct `ANALYTICS_DB_URL`. Compiled SQL executes here. Keeping the two pools distinct is a clean trust/operational boundary and the smallest answer that works.
- **Multi-warehouse (Snowflake/BigQuery/Redshift/DuckDB) is explicitly deferred.** When it arrives, the right move is a generic query-dispatch trait (mirroring how `soma_infra::storage::StorageClient` abstracts S3/Azure) with dialect-specific SQL emitters behind it — *not* adopting DataFusion federation prematurely.

---

## 5. The AI thesis soma-analytics adopts

**Natural language targets the governed semantic layer (NL → validated semantic query → compiled SQL); raw text-to-SQL is a fallback-of-last-resort, never the primary path.** This is structural, not just empirical:

- Frontier LLMs score **~86% on Spider 1.0** (clean academic schemas) but collapse to roughly **6–10% on Spider 2.0** (real enterprise schemas: 1,000+ columns, multi-step workflows). The gap is **information-theoretic** — no model closes it without grounding.
- Cube's paired benchmark across three frontier models showed that adding a **~4 KB semantic-layer document raised accuracy from ~46–50% to ~68–69%** across all of them — and **model choice became statistically indistinguishable**. The context mattered enormously; the model barely mattered.
- The mechanism (identical across Cube, dbt, AtScale): the semantic layer **reduces the task** from "write correct SQL against an unknown schema" (unbounded error surface) to "pick the right named measure + dimension from a finite, governed enumeration" (bounded, verifiable). The LLM **cannot** hallucinate a join, reference a missing column, or bypass row-level security when the layer structurally forbids expressing those operations.

**Practical architecture:** the LLM receives the `GET /meta` introspection response (the enumerated, governed measures/dimensions + descriptions), decomposes the question into the **same structured query body** the API already accepts (`measures[]`, `dimensions[]`, `filters[]`, `time_grain`), and submits it to `POST /query`. The compiler validates against the model registry, generates parameterized SQL, enforces scoped row filters, returns rows. **The LLM never touches raw schema or emits SQL.** For out-of-scope questions, return a structured "out of scope" error — **never** silently fall back to arbitrary SQL, which would destroy the governance guarantee. (A supervised text-to-SQL escape hatch, with explicit "this is not a governed metric" disclosure, is an optional Phase-2 path.)

---

## 6. Embedding into other products (the explicit Phase-1 requirement)

The goal — *"embed semantic-layer-powered charts/reports/dashboards directly into other products, like cube.dev"* — is delivered the way Cube delivers it: **headless**. Four minimal components, no iframes:

1. **Server-side token mint** — `POST /api/v1/embed/token` (authenticated by the host product's own session, not public) takes `{user_id, row_filters:[{column,value}]}` and returns a **short-lived (≤10 min) signed token**, signed with a dedicated `ANALYTICS_EMBED_SECRET` (kept separate from `DATABASE_URL`/internal auth). The token's `tenant_id` is always sourced from the caller's verified principal — not the request body. `row_filters` are **structured equality objects** — no arbitrary SQL in the token (the injection-safe Cube approach, not Superset's raw-`rls.clause`).
2. **CORS-enabled query endpoint** — `POST /api/v1/query` accepts the structured query with the embed token. Middleware verifies the token, extracts `tenant_id` + `row_filters`, and injects them as **mandatory `AND` conditions in the WHERE clause** before the compiler runs. The handler **never trusts caller-supplied tenant params.** CORS allow-list is per-deployment, never wildcard in prod.
3. **Thin JS client** — `@soma-analytics/client`: `SomaClient(fetchToken, { apiUrl }).query(body)` returns a `ResultSet` with `.series()` / `.tableData()` adapters that feed **whatever chart library the host already uses** (Recharts, Chart.js, D3, …). soma-analytics ships **zero chart code** and is chart-agnostic. `fetchToken` is a host-implemented callback the client calls transparently before token expiry.
4. **Meta API** — `GET /api/v1/meta` returns the governed, tenant-scoped list of queryable measures/dimensions. Powers both the AI planner and a host-built query-builder UI.

> **Why headless and not a chart kit:** soma-ui **already ships chart primitives** — `AreaChart`, `BarChart`, `LineChart`, `PieChart`, `RadarChart`, `RadialChart` (pure-SVG Leptos, used today in soma-observe's dashboard). Embedded dashboards therefore render charts using existing soma-ui components; there is no charts workstream to unblock. The genuinely new pieces are: (1) a saved dashboard/panel **definition store** in soma-analytics (`08_fct_dashboards` + `09_fct_panels`), and (2) a thin `<Dashboard>`/`<AnalyticsPanel>` **composition widget** added to soma-ui (platform rule: reusable UI → soma-ui). The primary goal is **internal embedding** into soma products (soma-iam "Activity", soma-audit "Reports", …) using the platform's normal auth; the §11.4 scoped-token path serves external 3rd-party products with the same machinery, as a secondary use case.

---

## 7. What soma-analytics copies vs defers

### Copy now (Phase 1)
- **Cube** — the **two-leg sub-SELECT** join strategy for multi-fact queries (fan-out without symmetric-aggregate math).
- **Cube / LookML / MetricFlow** — **required join cardinality** on every join; validated at model-load.
- **Cube** — **embed scoping to a cube**: Phase-1 embed tokens are locked to a named cube (`allowed_cube`); raw cube SQL is never exposed. Member-curated Views (governance facade over cubes) are deferred to Phase 2.
- **Cube** — **scoped signed embed tokens** minted server-side, with structured (not SQL) row filters injected into WHERE.
- **Cube** — **Redis result cache** keyed on SHA-256 of the canonically-serialized query (+ tenant + filters). Per-cube TTL.
- **MetricFlow** — **typed entity join graph**: entity key types as metadata; planner refuses M2M joins without an explicit bridge.
- **LookML** — **datagroup-style cache invalidation** (a named sentinel query / TTL shared by cache entries); **`primary_key` as a first-class field** for fan-out detection.
- **Embedding** — thin `@soma-analytics/client` + `fetchToken` callback pattern.

### Defer (named future home)
- **Pre-aggregations** → Postgres `MATERIALIZED VIEW` rollups + compiler aggregate-awareness (Phase 2); Cube-Store-style Parquet much later.
- **Postgres-wire SQL API** (Tableau/Superset) → `pgwire`-crate facade, new `crates/soma-pgwire` (Phase 2).
- **DataFusion** as execution/planning layer → only if multi-source federation is ever needed (Phase 3).
- **Symmetric aggregates** (MD5 dedup) → Phase 2, only where the two-leg pattern proves insufficient.
- **Multi-warehouse dialects** → query-dispatch trait in soma-infra + per-dialect emitters (Phase 3).
- **Dashboard definition store + `<Dashboard>`/`<AnalyticsPanel>` composition widget** → the saved-dashboard layer (`08_fct_dashboards` + `09_fct_panels`) and the thin soma-ui composition widget are **Phase 1** (§11.2/§11.3 of the spec). **Note:** soma-ui already ships the chart primitives (`AreaChart`, `BarChart`, `LineChart`, `PieChart`, `RadarChart`, `RadialChart`); what was missing was the dashboard-level composition and definition store, now addressed in Phase 1.
- **AI panel-generation** → NL → validated `SemanticQuery` → inferred `chart_type` → draft `09_fct_panels` row (Phase 1.5; §8 of the spec). A full drag-drop self-service builder stays Phase 2.
- **Raw text-to-SQL fallback** → supervised, disclosed escape hatch (Phase 2).
- **Foundry-style Ontology** (objects/links/**Actions** write-back, Scenarios) → mapped onto `soma_infra::kg` + pgvector (Phase 3) — see Part B §13.
- **Filtered measures** (Malloy-style per-measure WHERE predicate) → Phase 2.
- SCD2 dimensions, HyperLogLog approx-distinct, WebSocket subscriptions, per-tenant compiled schemas — all Phase 2+.

---

## 8. What the Ontology buys (and why it's deferred)

A pure metrics layer answers *"what happened?"* read-only. Foundry's Ontology adds **write-back Actions** (atomic mutations to the object graph and source systems), a **named object–link graph** (not just star-schema joins), and **Scenarios** (fork state for what-if). That is the leap from analytics to an operational system of record — the real Palantir moat. It is correctly **out of Phase 1**: it requires the metrics layer to be stable and trusted first. The substrate already exists in `soma_infra::kg` (objects→`KgNode`, links→`KgEdge`, semantic search→`vector_search_cosine`), so the path is *named, not invented later* (Part B §13).

---

*Next: Part B — the Phase-1 spec, wired to the platform out of the box.*
