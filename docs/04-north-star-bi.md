# soma-analytics — North Star: Rivaling Tableau / Power BI / Looker

> The ambition is to rival the BI incumbents. The way you *actually* do that is **not** by cloning their feature lists — it's by winning a wedge where they're structurally weak and becoming infrastructure underneath them. This doc grounds that strategy in a source-/benchmark-based teardown of Power BI, Tableau, Looker, and the AI-native challengers (ThoughtSpot, Sigma, Hex, Omni, Metabase/Superset, dbt/Cube), then maps an honest capability gap and a phased climb. Companion to docs/01–03.
>
> Discipline holds: Phase 1 stays the lean wedge (docs/02). This is the *trajectory*, not a Phase-1 scope expansion.

---

## 1. Positioning

**soma-analytics is the governed, AI-native semantic + data layer that products embed natively and that Tableau, Power BI, and Looker *connect to* as a Postgres-wire data source — before it replaces them for customers who don't need their complexity.** It is not another dashboard tool. The core invariant is architectural, not a feature: a single **governed semantic compiler, in Rust, that is the mandatory validation gate** between any caller — a human, an LLM, or an existing BI tool — and any SQL execution. A metric exists only if it's in the versioned manifest; a query runs only if the compiler accepts it; the AI **cannot** return a syntactically valid but semantically wrong answer because the compiler's type system forbids it. Every incumbent retrofits governance onto a tool built for ad-hoc authoring; soma starts from the opposite end.

---

## 2. The wedge — *power them before you replace them*

**Be the governed semantic layer that Tableau, Power BI, Grafana, and Metabase connect to via `pgwire`.** The beachhead = the **pgwire facade** + the governed compiler we're already building. A team gets: (1) one metric registry — "revenue" has exactly one definition, compiled at apply-time; (2) AI queries that are always valid because the LLM submits a structured query the Rust compiler validates before any SQL; (3) **every existing BI tool connects to soma as if it were Postgres, zero reconfiguration.**

Why incumbents can't easily counter it: Power BI's semantic model is a VertiPaq import file, not a wire server; Tableau's semantics are proprietary to Data Cloud; LookML is a developer-bottlenecked DSL — **none expose a pgwire endpoint others can connect to.** The wedge is defensible because it doesn't ask anyone to rip-and-replace their BI tool; it becomes the **infrastructure under it**, and infrastructure is far stickier than a competing dashboard. Land as the brain; grow into the whole stack.

---

## 3. Why the incumbents are beatable (structural weaknesses)

These aren't bugs they can patch — they're consequences of their architecture:

- **Governance sprawl is endemic.** Give every analyst a desktop authoring tool with no schema guard and you get conflicting "versions of truth" — documented: 5,000-user orgs accumulate **10,000+ unmanaged reports in ~2 years**. soma makes metric sprawl *structurally impossible* — one manifest, compiled.
- **AI is bolted onto ungoverned models.** Microsoft's own docs admit Copilot "can produce inaccurate… answers" and is "nondeterministic"; 90%+ of models have empty metadata, and there are documented RLS-bypass issues. A **dbt 2026 benchmark put raw text-to-SQL at 64–90% vs semantic-layer-grounded at 98–100%.** soma routes every NL query through the compiler — the AI literally can't emit an invalid/ungoverned query.
- **Power BI is Windows-only** (no native Mac/Linux desktop; ~4,000-vote request declined) — decisive friction for the Mac/Linux engineering + data-science persona soma targets.
- **DAX is a proprietary dead-end** — 1–6 months to master, no portability, catastrophic on high-cardinality. soma speaks SQL/semantic over pgwire; every existing SQL skill works day one.
- **Cost cliffs are brutal** — Pro is $14/user/mo, but AI (Fabric F64) is **$8,411/mo**, ISV embedding $4,206–8,411/mo, **no white-label at any tier**; a 100-user org with AI runs **$130K+/yr**. Tableau TCO is worse. soma's cost = one Postgres + Redis + a Rust binary; flat per-deployment, no per-seat.
- **Embedding is weak for ISVs** — iFrame-based, COOP re-auth conflicts, per-customer workspace provisioning, non-portable RLS. soma is headless: scoped tokens + a soma-ui `<Dashboard>` widget in the host's own design system.

---

## 4. Honest capability gap (incumbent → soma today → soma plan)

| Capability | Incumbent | soma today | soma plan |
|---|---|---|---|
| **Speed engine** | VertiPaq/Hyper in-memory columnar, 10:1 compression, sub-second on star schemas | Postgres + Redis TTL cache; correct, live latency | Phase 2: Postgres **materialized-view rollups** + Tokio refresh + aggregate-aware routing. Phase 3: **pure-Rust columnar engine (DataFusion/Polars)** over Parquet snapshots; **DuckDB (C++ FFI) only as a fallback** for big un-aggregated scans |
| **Interactive viz** | VizQL drag-to-shelf, bitset cross-filter, 20–50 ms | soma-ui SVG charts (Area/Bar/Line/Pie/Radar/Radial) + `<Dashboard>` widget; no cross-filter | Phase 2: **Vega-Lite** grammar wrapped in Leptos. Phase 3: **Mosaic**-style cross-filter pushed to the columnar engine (1–10 ms on 10M+ rows); WebGPU for dense series. *Assembled, not invented.* |
| **Modeling / calc** | DAX / LOD (proprietary, deep, hard) | Rust compiler: measures/dims/segments/one-hop joins w/ cardinality, two-leg fan-out, bind-param safety | Phase 2: filtered measures, fixed-grain (LOD-equivalent), member policies. Phase 3: symmetric aggregates, time-intelligence CTEs. **Win = governance (one metric = one plan), not DAX expressiveness** |
| **Connectors** | 160+ native (Power BI), gateways | One read-only Postgres (cross-schema = all soma services) | Phase 2: **pgwire inverts the problem** — every BI tool connects to *us* for free. Phase 3: multi-warehouse dialect trait. *Connector necessity sidestepped, not matched* |
| **Self-service authoring** | Drag-to-shelf / Power Query (Windows-only) | `soma-cli apply` + API (engineer persona) | Phase 1.5: **NL→query** + AI panel generation. Phase 2: lightweight builder UI from `/meta`. Phase 3: **AI-first authoring** — the intentional bypass of VizQL |
| **Governance** | Metric sprawl; manual CoE; proprietary | **Compile-time property** — unknown member = error; audited; git-versionable manifest | Phase 2: governance-as-code (`--dry-run` CI linter), lineage `/meta`, member visibility. Phase 3: OSI YAML export. Phase 4: ontology governance |
| **AI / NL** | Copilot ($8,411/mo F64); silent wrong answers | Flag-gated seam; LLM gets governed vocab only, compiler validates | Phase 1.5: activate `/ai/query` (no raw-SQL fallback). Phase 2: BIRD-style eval harness + public benchmark, certified queries, MCP server. Phase 3: entity reasoning over the ontology |
| **Embedding** | iFrame, per-customer workspaces, no white-label, F32+ | Headless: scoped HMAC tokens, CORS query API, JS client, soma-ui widget; tenant isolation via compiler-injected WHERE | Phase 1.5: token hardening (jti + revocation). Phase 2: pgwire embed path. **Flat per-deployment pricing — 10k viewers cost the same as 100** |
| **Collaboration / publishing** | Teams/SharePoint, scheduled delivery, Pulse digests | Saved dashboards/panels per tenant | Phase 2: Tokio-scheduled email/webhook delivery. Phase 3: Slack/webhook metric digests (Pulse-equivalent, but compile-validated). Phase 4: collab + version history |
| **Cost / licensing** | Per-seat + capacity SKUs; $130K+/yr w/ AI; no white-label | Infra-only (Postgres + Redis + binary) | **OSS the Rust compiler** (Apache-2.0) for adoption; commercialize the managed service **flat per deployment**, AI credits the only usage dimension — undercuts every embedded tier *structurally* |

---

## 5. The make-or-break bets

1. **The Rust compiler is the trust boundary that makes AI analytics deterministic.** Pure crate; every query (human, LLM, or pgwire) validated against the manifest before SQL. Out-of-scope = typed error, never a guess. *Risk:* calc-expressiveness scope-creep into "a second DAX" — mitigate by adding each calc type as a named phase item with a passing integration test, measured against *real customer metric requests*, not DAX parity.
2. **A pure-Rust columnar engine is the speed layer** (DataFusion/Polars over Parquet snapshots exported from Postgres MVs by the Tokio scheduler). DataFusion also serves as the pgwire query planner (pure Rust, no FFI — the GreptimeDB pattern). *Honest note:* DuckDB beats VertiPaq on some shapes (high-cardinality GROUP BY 2.7s vs 17s; larger-than-RAM via disk-spill) — so **DuckDB (C++ FFI) is the named fallback** if we ever need to scan big *un-aggregated* data. Our rollups are aggregated (small, in-memory), so pure-Rust is the default and stays all-Rust. *Risk:* single-writer snapshot refresh under high read concurrency — mitigate by sequencing (MV first, columnar engine only after measuring p95) and keeping the engine read-only from query handlers.
3. **The pgwire facade turns soma into infrastructure** that powers Tableau/Power BI/Grafana/Metabase rather than fighting them. *Risk:* per-tool wire-protocol quirks (pg_catalog introspection, prepared statements, Tableau vs Power BI discovery) — ~6–10 weeks for a basic facade; **integration-test against real Tableau/Power BI/Grafana/psql before declaring it done.**
4. **Open-source the compiler (Apache-2.0)** to drive manifest-format adoption — the Cube/dbt playbook (open core, monetize runtime). *Risk:* dbt MetricFlow went Apache-2.0 (Oct 2025) and has a head start — mitigate by **publishing the accuracy benchmark early** (governed vs raw text-to-SQL, reproducible on the OSS compiler) and targeting the analytics-engineering persona.

---

## 6. What NOT to build (ponytail at grand scale)

- **No drag-to-shelf VizQL clone** — Tableau has 25 years there; NL-first is the strategic bypass.
- **No DAX** — a YAML manifest compiling to SQL; governance over expressiveness for the target persona.
- **No connector library** — pgwire inverts it; tools connect to us.
- **No general text-to-SQL fallback** — it collapses the entire governance guarantee. Out-of-scope = typed error.
- **No Node.js sidecar / Cube runtime wrap** — not separable from Node (docs/03).
- **No custom columnar store from scratch** — MVs (Phase 2) + an embedded engine (Phase 3) cover it.
- **No grammar-of-graphics from scratch** — Vega-Lite/Mosaic is 6–8 weeks vs 2–4 engineer-years.
- **No per-seat / per-query pricing** — flat per-deployment is the structural advantage; per-seat destroys the embedded economics.
- **No ontology (Actions/write-back) before the metrics layer + pgwire distribution exist** — Phase 4, not now.

---

## 7. The phased climb

- **Phase 1 — Minimum governed semantic layer (current).** compile()+CompileError+two-leg fan-out; metadata CRUD + Redis cache + single read-only audit pool; `/query`, `/meta`, `/embed/token`, model + dashboard/panel CRUD; soma-ui `<Dashboard>`; SDK + CLI + JS client; **proof: a soma-audit "Reports" tab powered by soma-analytics.**
- **Phase 1.5 — AI seam + pgwire signal.** `/ai/query` (structured output through the compile() gate, no raw-SQL fallback) + AI panel generation; `description` in `/meta`; embed-token hardening; member visibility; **initial `soma-pgwire`** with the explicit goal that Tableau Desktop, Grafana, and psql all connect.
- **Phase 2 — Performance + governance-as-code + BI compatibility.** Postgres MV rollups + Tokio scheduler + aggregate-aware routing; refreshKey probe + stale-while-revalidate; pgwire hardened against real BI tools; governance-as-code CI linter; lineage `/meta`; BIRD-style eval benchmark; MCP server; real soma-iam swap; **Vega-Lite** in soma-ui.
- **Phase 3 — Columnar speed + interactive cross-filter + multi-warehouse.** Pure-Rust embedded engine (DataFusion/Polars; DuckDB fallback) over Parquet snapshots; Mosaic-style cross-filter; lambda pattern (sealed history `UNION` live tail); multi-warehouse dialect trait; OSI YAML export; **open-source the compiler.**
- **Phase 4 — Ontology + write-back (the Palantir trajectory).** Entity/link graph over `soma_infra::kg` + pgvector; entity-level NL reasoning; **Action types (write-back)**; scenarios; collaboration + version history; proactive metric digests. The moat Power BI/Tableau architecturally can't build.

---

## 8. Honest kill criteria (where this fails)

State these now so we measure against them:

- **Phase 1 scope-creeps** — if a working `compile()`+`execute()` round-trip over soma-audit takes **>8 weeks**, YAGNI discipline has failed.
- **pgwire can't satisfy real Tableau/Power BI Desktop** — then the "power them first" wedge has no distribution.
- **The AI seam is silently wrong** — if an LLM query passes `compile()` but returns a number an engineer recognizes as wrong (wrong grain/join/period), trust is unrecoverable.
- **The columnar engine can't hit sub-100 ms cross-filter on 50M+ rows** on standard instances — the "governed *and* fast" positioning collapses.
- **The manifest format gets ignored** in favor of dbt MetricFlow/OSI YAML — the OSS distribution play generates no gravity.
- **Cost-as-infrastructure doesn't beat Power BI's $14/seat already-in-Microsoft-365** in SMB — bundling wins where it's strongest; soma must lead with embedding + governance + AI, not price alone.
- **Enterprise governance tooling gaps** — if audit/lineage isn't rich enough for regulated industries, the enterprise wedge stalls.

---

## 9. The one-line truth

We don't beat Tableau/Power BI by drawing prettier charts. We beat them by being the **governed, AI-correct, embeddable, all-Rust data brain** they themselves plug into — then growing the visualization and operational (ontology) layers on top, on our terms. Phase 1 is small on purpose; the north star is large on purpose; the bridge between them is discipline.
