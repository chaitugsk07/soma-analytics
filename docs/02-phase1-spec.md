# soma-analytics — Part B: Phase-1 Spec

> A **minimum lovable semantic layer**, wired to the soma-platform out of the box. Define a governed model → compile a semantic query to SQL → execute against a configured Postgres → return rows → embed the results in other products. Everything beyond that is deferred (§13).
>
> Discipline: ponytail (YAGNI, shortest working diff, no speculative abstractions). Every backend primitive is consumed from **soma-infra**; every decision/policy stays in the service (THE LINE). Migrations run via **soma-schema**; mutations are audited via **soma-audit**.
>
> Compiler decision (Part A): **build a minimal Rust compiler** (not Cube, not DataFusion). Data source: **one configured Postgres read-only pool targeting a soma service DB** (db-standards DB-1202 READ_USER; recommended first source: soma-audit) — see §11.1. Multi-source deferred to Phase 1.5.
>
> Companions: `docs/03-cube-deep-evaluation.md` — source-level Cube evaluation (point-in-time store, caching, pgwire, AI) + roadmap; `docs/04-north-star-bi.md` — the rival-Tableau/Power BI strategy (wedge, capability gap, Phase 1→4 climb). Phase 1 here stays the lean wedge; docs/04 is the trajectory.

---

## 0. Scope

**In Phase 1:** tenants → data sources → cubes (over a base table or SQL) → dimensions, measures (with aggregation type), single-level joins (with declared cardinality), segments (named filters). Pick a cube/view, select measures + dimensions, apply filters/segments, group + time-grain, get typed rows. Result cache. Model CRUD (audited, auth-guarded). Headless embedding (scoped token + CORS query API + thin JS client + meta API). Optional NL→query behind a feature flag (Phase 1.5). Rust SDK + CLI. Embedded dashboards/reports across soma products — saved dashboards/panels rendered by a shared soma-ui widget over a read-only business-data source (one source first; recommended: soma-audit).

**Not in Phase 1 (see §13):** pre-aggregations, multi-source connection registry — only needed for a non-soma external warehouse (the shared Postgres already covers cross-service reports cross-schema); deferred. Also deferred: multi-warehouse (Snowflake/BigQuery/Redshift/DuckDB), Postgres-wire SQL API, symmetric aggregates, AI panel-generation, the ontology layer, real soma-iam integration.

---

## 1. Crate / workspace layout

Mirrors the canonical soma-vault shape (`crates/*` workspace, `dashboard/` excluded, `migrations/` at root, platform crates as path deps at `../../../`).

```
soma-analytics/
├── Cargo.toml                         # [workspace] members = crates/*  ; exclude = ["dashboard", "sdk/js"]
│                                      # [profile.dev] debug=false, strip=true, incremental=false   (disk discipline)
├── rust-toolchain.toml
├── migrations/                        # soma-schema contract (§4)
│   ├── migration-order.yaml
│   ├── 00_setup/
│   │   └── 01_schema.sql              # CREATE SCHEMA IF NOT EXISTS "soma_analytics"; fn_update_timestamp(); (idempotent)
│   └── 01_migrated/
│       └── 1/
│           ├── 20260629_01_enable-pgcrypto.sql
│           ├── 20260629_02_init-analytics-schema.sql
│           └── 20260629_03_init-dashboards.sql
├── crates/
│   ├── soma-semantic/                 # ★ PURE compiler — model types, query types, validation, model→SQL. NO I/O.
│   │                                  #   (mirrors how soma-vault isolates soma-crypto). Independently unit-testable.
│   ├── soma-storage/                  # metadata DB access (sqlx), model CRUD, result-cache wiring, the data-source pool,
│   │                                  #   audit-wired writes, migration runner. Holds BOTH pools.
│   ├── soma-api/                      # axum router, Principal extractor, auth middleware, CORS, handlers
│   │                                  #   (query, meta, model CRUD, embed token, ai)
│   ├── soma-server/                   # the sole [[bin]] — main.rs boot sequence (§7)
│   ├── soma-sdk/                      # Rust reqwest client (define/list model, run query, mint embed token)
│   └── soma-cli/                      # apply a YAML model file, run a query, list cubes  (thin; high value)
├── sdk/
│   └── js/                            # @soma-analytics/client — thin fetch wrapper for EMBEDDING (no charts). Excluded.
└── dashboard/                         # DEFERRED first-party UI (Leptos CSR + soma-ui). Excluded from workspace.
```

**Why `soma-semantic` is its own crate:** the compiler is pure logic (model → SQL) with no database, no HTTP, no clock. Isolating it makes fan-out correctness independently testable, keeps the I/O crates thin, and lets Phase 2 add a CLI introspection tool or a `pgwire` facade without touching it. This is the single most important structural decision in the spec.

**Test plan:** `soma-semantic` ships `tests/` with Postgres-backed integration tests (via `soma_infra::TestDb`) covering at least: single-cube query, many_to_one join, one_to_many two-leg fan-out, two-leg with a NULL dimension value, a non-additive measure in fan-out → `NonAdditiveMeasureInFanout`, a 3-cube query → `UnsupportedFanout`, filter-value SQL-injection attempt (asserts bound params), and cache-key change on `model_version` bump.

---

## 2. Platform integration checklist (proof nothing is hand-rolled)

Every row cites the **exact** soma-infra / soma-schema / soma-audit symbol consumed, and the cargo feature that gates it. Verified against the installed source.

| Concern | Exact symbol consumed | Crate / feature |
|---|---|---|
| Metadata Postgres pool | `soma_infra::connect_from_env()` | soma-infra `db` |
| **Data-source** pool (separate) | `soma_infra::db::connect(&PoolConfig::new(analytics_db_url))` | soma-infra `db` |
| **Business-data source (read-only)** | `soma_infra::db::connect(&PoolConfig::new(read_only_dsn))` — a READ_USER pool over the target service's Postgres (db-standards DB-1202); see §11.1 | soma-infra `db` |
| **Charts (consumed, not built)** | soma-ui `AreaChart`/`BarChart`/`LineChart`/`PieChart`/`RadarChart`/`RadialChart` + new `<Dashboard>`/`<AnalyticsPanel>` composition widget | soma-ui |
| Telemetry / logging | `soma_infra::telemetry::init()` | soma-infra `tracing` |
| Env config | `soma_infra::config::{require_env, env_or, env_parse}` | soma-infra `config` |
| Graceful shutdown | `soma_infra::signal::shutdown_signal()` | soma-infra `signal` |
| DB-error redaction (API errors) | `soma_infra::errors::redact_db_error(&e)` | soma-infra `errors` |
| HTTP server / SPA / bearer | `soma_infra::web::{serve_with_shutdown, serve_spa, extract_bearer}` | soma-infra `web` |
| **Result cache** (read-hot) | `soma_infra::cache::{connect_from_env, get, set_ex, del}` over `redis::aio::ConnectionManager` | soma-infra `cache` |
| Cache key digest | `soma_infra::crypto::sha256_hex(bytes)` | soma-infra `crypto` |
| **Embed-token signature** | `soma_infra::crypto::hmac_sha256_hex(secret, payload)` (+ a constant-time verify — see §14) | soma-infra `crypto` |
| API-token hashing (lookup) | `soma_infra::crypto::sha256_hex(token_bytes)` | soma-infra `crypto` |
| Data-source DSN at rest | `soma_infra::crypto::{encrypt, decrypt, CryptoKey}` (AES-256-GCM, AAD = tenant_id) (optional — only when a per-source DSN is set; Phase-1 default is the env source) | soma-infra `crypto` |
| LLM (AI seam, Phase 1.5) | `soma_infra::llm::{LlmClient, LlmConfig, MessagesRequest, Message, Role}` | soma-infra `llm` |
| HTTP client (Rust SDK) | `soma_infra::http::client()` | soma-infra `http` |
| Migrations | `soma_schema::{Migrator, PostgresDriver, PostgresConfig}` → `Migrator::from_root(..).up(&driver)` | soma-schema (`default-features = false`) |
| Audit schema install | `soma_audit_pg::install(&pool)` | soma-audit-pg |
| Audit keys (embedded) | `soma_audit_pg::AuditKeys::from_env_local()` | soma-audit-pg |
| Audit sink | `soma_audit_pg::LocalSink::new(pool, Arc::new(keys), "soma-analytics")` | soma-audit-pg |
| Audit event | `AuditEvent::builder(tenant_id, action, Outcome::*)…build()` + `record_in_tx(&event, &mut tx)` / `record(&event)` | soma-audit-core (re-exported) |
| **Ontology** (DEFERRED) | `soma_infra::kg::{KgNode, KgEdge, upsert_node, upsert_edge, neighbors, vector_search_cosine}` + pgvector | soma-infra `kg` (§13) |
| Object storage / export (DEFERRED) | `soma_infra::storage::StorageClient` | soma-infra `storage-s3`/`storage-azure` (§13) |

**Per-crate soma-infra features:**
- `soma-storage`: `["db", "cache", "crypto", "errors"]`
- `soma-server`: `["db", "tracing", "signal", "config", "web", "crypto", "cache", "errors", "llm"]`
- `soma-api`: `["web", "crypto", "llm"]`
- `soma-semantic`: **none** (pure crate; depends only on `serde`, `uuid`, `thiserror` — no soma-infra)
- `soma-sdk`: `["http"]`

**Path deps** (from a `crates/<name>/` member): `soma-infra = { path = "../../../soma-infra", features = [...] }`, `soma-schema = { path = "../../../soma-schema", default-features = false }`, `soma-audit-pg = { path = "../../../soma-audit/crates/soma-audit-pg" }`, `soma-audit-core = { path = "../../../soma-audit/crates/soma-audit-core" }`. Intra-workspace: `soma-semantic = { path = "../soma-semantic" }`, etc.

---

## 3. Auth model & the IAM-deferral seam

soma-iam's cross-service SDK is a stub today, so Phase 1 ships **local placeholder auth** behind a thin trait, swappable for the real IAM SDK at **M11+**. The shape mirrors soma-vault's `Principal` (inserted into request extensions by middleware, extracted via `FromRequestParts`).

```rust
// crates/soma-api — the principal carried on every authenticated request
#[derive(Clone)]
pub struct Principal {
    pub tenant_id: Uuid,
    pub subject: String,        // token id (admin key) OR "embed:<user_id>"
    pub role: Role,             // Reader | Editor | Admin
    pub scope: AuthScope,       // Full (admin key) | Embed { allowed_cube: Option<String>, row_filters: Vec<RowFilter> }
}
pub enum Role { Reader, Editor, Admin }   // VARCHAR + CHECK in DB; rank Reader<Editor<Admin

// THE SEAM — Phase 1 has one impl; M11 adds IamTokenVerifier with identical signature.
#[async_trait]
pub trait TokenVerifier: Send + Sync {
    async fn verify(&self, bearer: &str) -> Option<Principal>;
}

pub struct LocalTokenVerifier { /* metadata pool, embed secret */ }
// Phase 1 impl resolves two token shapes:
//   1. Admin/API key  → sha256_hex(token) looked up in "soma_analytics"."01_fct_api_tokens" (role + tenant_id)
//   2. Embed token     → "<base64url(payload)>.<hmac>"; verify hmac via crypto::hmac_sha256_verify (constant-time; NEVER String == which short-circuits and leaks timing — see §14), check exp, build Embed Principal
// Future:  pub struct IamTokenVerifier { /* soma-iam SDK */ }  // drop-in replacement
```

`auth_middleware` calls `extract_bearer(...)`, then `verifier.verify(token)`, inserts `Principal` into extensions or returns 401. Authorization is `principal.require(min_role)` (Reader<Editor<Admin), exactly like vault. **Tenant is always sourced from the verified `Principal`, never from request input** (db-standards DB-701). `LocalTokenVerifier::verify()` enforces `expires_at IS NULL OR expires_at > now()` in the SQL lookup itself (not just app logic). Unit test: an expired token must be rejected.

---

## 4. Database schema + migration plan

### 4.1 Runner config (state in spec)

```rust
// crates/soma-storage — run at startup (soma-schema)
PostgresDriver::new(pool.clone(), PostgresConfig {
    schema: Some("soma_analytics".into()),
    advisory_lock_key: 0x50A1_A7C5_0001_i64,   // ★ NEW, unique. (taken: vault 0x50A1_7A01_7017,
    ..Default::default()                        //  audit 6020250626000001, iam 7318249506742315)
})?;
Migrator::from_root(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations")).up(&driver).await?;
```

- **Schema:** `soma_analytics` (matches soma-audit's `soma_audit` service-name convention; the platform's services use a service-name schema rather than the generic `NN_` prefix — tables inside still use `NN_<type>_` per DB-301).
- **Advisory lock key:** `0x50A1_A7C5_0001` — new and unique across the shared Postgres (confirm in §15).
- **Pool:** metadata pool must allow `max_connections >= 2` (soma-schema + soma-audit each hold one connection for their advisory lock). The default `PoolConfig` (max 10) satisfies this.

### 4.2 Migration files

`migration-order.yaml`:
```yaml
manifest_version: 1
versions:
  - version: 1
    description: "Initial soma-analytics semantic model"
    migrations:
      - file: "20260629_01_enable-pgcrypto.sql"
        created: "2026-06-29"
        author: "soma-analytics"
        why: "gen_random_uuid() + crypto primitives (db-standards DB-102)"
      - file: "20260629_02_init-analytics-schema.sql"
        created: "2026-06-29"
        author: "soma-analytics"
        why: "Model registry: api_tokens, data_sources, cubes, dimensions, measures, joins, segments"
      - file: "20260629_03_init-dashboards.sql"
        created: "2026-06-29"
        author: "soma-analytics"
        why: "Savable dashboards + panels (embedded reports layer)"
```

`00_setup/01_schema.sql` (idempotent, untracked, runs every `up()`):
```sql
CREATE SCHEMA IF NOT EXISTS "soma_analytics";
CREATE OR REPLACE FUNCTION "soma_analytics".fn_update_timestamp()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN NEW.updated_at = now(); RETURN NEW; END;
$$;
-- Default privileges (db-standards DB-206). Role names must match the other soma services on the shared instance.
ALTER DEFAULT PRIVILEGES IN SCHEMA "soma_analytics" GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO soma_write_user;
ALTER DEFAULT PRIVILEGES IN SCHEMA "soma_analytics" GRANT SELECT ON TABLES TO soma_read_user;
```

`20260629_01_enable-pgcrypto.sql`: `CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;` (+ `-- DOWN ==` no-op: `DROP EXTENSION pgcrypto` is omitted because the extension is shared across services in the public schema; dropping it would break other soma services — satisfies soma-schema invariant 3's documentation requirement for non-reversible migrations).

### 4.3 Model-metadata tables (`20260629_02_init-analytics-schema.sql`)

Conventions applied: `NN_<type>_<descriptor>` double-quoted names; `UUID DEFAULT gen_random_uuid()` PKs; `tenant_id UUID NOT NULL` (platform-consistent — see §15 deviation note); `TIMESTAMPTZ NOT NULL DEFAULT now()`; `VARCHAR+CHECK` instead of ENUM; soft-delete triplet + bidirectional CHECK on entity tables; explicit constraint names; partial unique indexes `WHERE is_deleted = false`; mandatory comments; **no RLS** (db-standards DB-108 — tenancy enforced in the query layer). Dependency order is encoded in the numeric prefix.

```sql
-- 01_fct_api_tokens — placeholder local auth (IAM seam). High-entropy tokens → sha256 lookup is sufficient.
CREATE TABLE "soma_analytics"."01_fct_api_tokens" (
    id           UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id    UUID         NOT NULL,
    token_sha256 VARCHAR(64)  NOT NULL,                       -- sha256_hex(token); the plaintext is shown once
    name         VARCHAR(120) NOT NULL,
    role         VARCHAR(20)  NOT NULL DEFAULT 'reader',
    expires_at   TIMESTAMPTZ,
    is_deleted   BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by   UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_01_fct_api_tokens PRIMARY KEY (id),
    CONSTRAINT ck_01_fct_api_tokens_sha256_len CHECK (length(token_sha256) = 64),
    CONSTRAINT ck_01_fct_api_tokens_role  CHECK (role = ANY (ARRAY['reader','editor','admin'])),
    CONSTRAINT ck_01_fct_api_tokens_deleted CHECK (
        (is_deleted = false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted = true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_01_fct_api_tokens_sha ON "soma_analytics"."01_fct_api_tokens" (token_sha256)
    WHERE is_deleted = false;
COMMENT ON TABLE "soma_analytics"."01_fct_api_tokens" IS 'Local placeholder API tokens per tenant (IAM seam).';
COMMENT ON COLUMN "soma_analytics"."01_fct_api_tokens"."token_sha256" IS 'PII: indirect — sha256_hex of the API token; plaintext shown once.';

-- 02_fct_data_sources — a configured Postgres source. DSN stored ENCRYPTED (never plaintext, db-standards DB-1201).
CREATE TABLE "soma_analytics"."02_fct_data_sources" (
    id             UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id      UUID         NOT NULL,
    name           VARCHAR(120) NOT NULL,                     -- business key (code)
    driver         VARCHAR(20)  NOT NULL DEFAULT 'postgres',
    dsn_ciphertext BYTEA,                                     -- NULL ⇒ use the single ANALYTICS_DB_URL env source (Phase-1 default). Non-NULL ⇒ crypto::encrypt(KEK, dsn, aad=tenant_id) for a per-source DSN (multi-source extension).
    is_deleted     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_02_fct_data_sources PRIMARY KEY (id),
    CONSTRAINT ck_02_fct_data_sources_driver CHECK (driver = ANY (ARRAY['postgres'])),
    CONSTRAINT ck_02_fct_data_sources_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_02_fct_data_sources_tenant_name
    ON "soma_analytics"."02_fct_data_sources" (tenant_id, name) WHERE is_deleted = false;

-- 03_fct_cubes — a cube over a base table OR SQL. model_version bumps on any change to the cube or its children.
CREATE TABLE "soma_analytics"."03_fct_cubes" (
    id             UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id      UUID         NOT NULL,
    data_source_id UUID         NOT NULL,
    name           VARCHAR(120) NOT NULL,
    title          VARCHAR(200),
    description    TEXT,
    sql_table      VARCHAR(300),                              -- exactly one of (sql_table, base_sql) is set
    base_sql       TEXT,
    primary_key    VARCHAR(120) NOT NULL,                     -- required for fan-out correctness (LookML idea)
    cache_ttl_secs INTEGER      NOT NULL DEFAULT 300,
    model_version  INTEGER      NOT NULL DEFAULT 1,           -- ★ cache-invalidation lever (see §6)
    is_deleted     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_03_fct_cubes PRIMARY KEY (id),
    CONSTRAINT fk_03_fct_cubes_data_source_id_02_fct_data_sources
        FOREIGN KEY (data_source_id) REFERENCES "soma_analytics"."02_fct_data_sources"(id) ON DELETE RESTRICT,
    CONSTRAINT ck_03_fct_cubes_source CHECK (
        (sql_table IS NOT NULL AND base_sql IS NULL) OR (sql_table IS NULL AND base_sql IS NOT NULL)),
    CONSTRAINT ck_03_fct_cubes_primary_key CHECK (length(trim(primary_key)) > 0),
    CONSTRAINT ck_03_fct_cubes_sql_table_nonempty CHECK (sql_table IS NULL OR length(trim(sql_table)) > 0),
    CONSTRAINT ck_03_fct_cubes_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_03_fct_cubes_tenant_name
    ON "soma_analytics"."03_fct_cubes" (tenant_id, name) WHERE is_deleted = false;
CREATE INDEX idx_soma_analytics_03_fct_cubes_data_source_id
    ON "soma_analytics"."03_fct_cubes" USING btree (data_source_id);

-- 04_fct_dimensions
CREATE TABLE "soma_analytics"."04_fct_dimensions" (
    id         UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id  UUID         NOT NULL,
    cube_id    UUID         NOT NULL,
    name       VARCHAR(120) NOT NULL,
    description TEXT,
    sql_expr   TEXT         NOT NULL,                          -- expression over {CUBE}, e.g. "status"
    data_type  VARCHAR(20)  NOT NULL,                          -- string|number|time|boolean
    is_deleted BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_04_fct_dimensions PRIMARY KEY (id),
    -- RESTRICT (not CASCADE): these are fct_ tables; the service only soft-deletes, so a hard DELETE must explicitly remove children first — preserving the audit trail.
    CONSTRAINT fk_04_fct_dimensions_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id) REFERENCES "soma_analytics"."03_fct_cubes"(id) ON DELETE RESTRICT,
    CONSTRAINT ck_04_fct_dimensions_data_type CHECK (data_type = ANY (ARRAY['string','number','time','boolean'])),
    CONSTRAINT ck_04_fct_dimensions_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_04_fct_dimensions_cube_name
    ON "soma_analytics"."04_fct_dimensions" (cube_id, name) WHERE is_deleted = false;
CREATE INDEX idx_soma_analytics_04_fct_dimensions_cube_id
    ON "soma_analytics"."04_fct_dimensions" USING btree (cube_id);

-- 05_fct_measures
CREATE TABLE "soma_analytics"."05_fct_measures" (
    id         UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id  UUID         NOT NULL,
    cube_id    UUID         NOT NULL,
    name       VARCHAR(120) NOT NULL,
    description TEXT,
    sql_expr   TEXT,                                           -- column/expr to aggregate; NULL for agg_type='count'
    agg_type   VARCHAR(20)  NOT NULL,                          -- count|count_distinct|sum|avg|min|max|number
    is_deleted BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_05_fct_measures PRIMARY KEY (id),
    CONSTRAINT fk_05_fct_measures_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id) REFERENCES "soma_analytics"."03_fct_cubes"(id) ON DELETE RESTRICT,
    CONSTRAINT ck_05_fct_measures_sql_expr CHECK (agg_type = 'count' OR sql_expr IS NOT NULL),
    CONSTRAINT ck_05_fct_measures_agg CHECK (agg_type = ANY (ARRAY['count','count_distinct','sum','avg','min','max','number'])),
    CONSTRAINT ck_05_fct_measures_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_05_fct_measures_cube_name
    ON "soma_analytics"."05_fct_measures" (cube_id, name) WHERE is_deleted = false;
CREATE INDEX idx_soma_analytics_05_fct_measures_cube_id
    ON "soma_analytics"."05_fct_measures" USING btree (cube_id);

-- 06_fct_joins — single-level joins between cubes; cardinality REQUIRED (fan-out gate).
CREATE TABLE "soma_analytics"."06_fct_joins" (
    id             UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id      UUID         NOT NULL,
    cube_id        UUID         NOT NULL,                      -- the "from" cube
    target_cube_id UUID         NOT NULL,
    name           VARCHAR(120) NOT NULL,
    relationship   VARCHAR(20)  NOT NULL,                      -- many_to_one|one_to_many|one_to_one
    sql_on         TEXT         NOT NULL,                      -- e.g. "{CUBE}.customer_id = {customers}.id"
    is_deleted     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_06_fct_joins PRIMARY KEY (id),
    CONSTRAINT fk_06_fct_joins_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id) REFERENCES "soma_analytics"."03_fct_cubes"(id) ON DELETE RESTRICT,
    CONSTRAINT fk_06_fct_joins_target_cube_id_03_fct_cubes
        FOREIGN KEY (target_cube_id) REFERENCES "soma_analytics"."03_fct_cubes"(id) ON DELETE RESTRICT,
    CONSTRAINT ck_06_fct_joins_relationship CHECK (relationship = ANY (ARRAY['many_to_one','one_to_many','one_to_one'])),
    CONSTRAINT ck_06_fct_joins_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_06_fct_joins_cube_name
    ON "soma_analytics"."06_fct_joins" (cube_id, name) WHERE is_deleted = false;
CREATE INDEX idx_soma_analytics_06_fct_joins_cube_id
    ON "soma_analytics"."06_fct_joins" USING btree (cube_id);
CREATE INDEX idx_soma_analytics_06_fct_joins_target_cube_id
    ON "soma_analytics"."06_fct_joins" USING btree (target_cube_id);

-- 07_fct_segments — named, reusable filter predicates.
CREATE TABLE "soma_analytics"."07_fct_segments" (
    id         UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id  UUID         NOT NULL,
    cube_id    UUID         NOT NULL,
    name       VARCHAR(120) NOT NULL,
    sql_expr   TEXT         NOT NULL,                          -- "{CUBE}.amount_cents > 100000"
    is_deleted BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_07_fct_segments PRIMARY KEY (id),
    CONSTRAINT fk_07_fct_segments_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id) REFERENCES "soma_analytics"."03_fct_cubes"(id) ON DELETE RESTRICT,
    CONSTRAINT ck_07_fct_segments_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_07_fct_segments_cube_name
    ON "soma_analytics"."07_fct_segments" (cube_id, name) WHERE is_deleted = false;
CREATE INDEX idx_soma_analytics_07_fct_segments_cube_id
    ON "soma_analytics"."07_fct_segments" USING btree (cube_id);
```

(A representative `COMMENT ON` block is shown above for `01_fct_api_tokens`; the actual migration carries `COMMENT ON TABLE/COLUMN` for all seven tables (DB-307), with PII classifications (DB-1206) on at least `tenant_id`, `token_sha256`, and `dsn_ciphertext` (`PII: sensitive — AES-256-GCM-encrypted DSN; plaintext never stored`). `fn_update_timestamp` triggers per table, and a `-- DOWN ==` section dropping tables in FK-safe reverse order — note: because child FKs are RESTRICT, the DOWN teardown must explicitly DELETE child rows (dimensions, measures, joins, segments) before dropping/deleting parent cubes. Mandatory in the actual file.)

The dashboard/panel tables live in a separate migration file (`20260629_03_init-dashboards.sql`):

```sql
-- 08_fct_dashboards — a savable dashboard/report (a named set of panels) per tenant.
CREATE TABLE "soma_analytics"."08_fct_dashboards" (
    id          UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id   UUID         NOT NULL,
    name        VARCHAR(120) NOT NULL,
    description TEXT,
    is_deleted  BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_08_fct_dashboards PRIMARY KEY (id),
    CONSTRAINT ck_08_fct_dashboards_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_08_fct_dashboards_tenant_name
    ON "soma_analytics"."08_fct_dashboards" (tenant_id, name) WHERE is_deleted = false;
COMMENT ON TABLE "soma_analytics"."08_fct_dashboards" IS 'A savable dashboard/report — a named, tenant-scoped set of panels.';
COMMENT ON COLUMN "soma_analytics"."08_fct_dashboards"."tenant_id" IS 'PII: indirect — identifies the owning tenant.';

-- 09_fct_panels — one panel = a saved semantic query rendered as a chart, positioned on a dashboard grid.
CREATE TABLE "soma_analytics"."09_fct_panels" (
    id           UUID         DEFAULT gen_random_uuid() NOT NULL,
    tenant_id    UUID         NOT NULL,
    dashboard_id UUID         NOT NULL,
    name         VARCHAR(120) NOT NULL,
    chart_type   VARCHAR(20)  NOT NULL,          -- area|bar|line|pie|radar|radial|table|number (1:1 soma-ui charts)
    query_json   JSONB        NOT NULL,          -- the saved SemanticQuery document (opaque; executed by the compiler — DB-104 permits JSONB here)
    grid_x       INTEGER      NOT NULL DEFAULT 0,
    grid_y       INTEGER      NOT NULL DEFAULT 0,
    grid_w       INTEGER      NOT NULL DEFAULT 6,
    grid_h       INTEGER      NOT NULL DEFAULT 4,
    is_deleted   BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID, updated_by UUID, deleted_at TIMESTAMPTZ, deleted_by UUID,
    CONSTRAINT pk_09_fct_panels PRIMARY KEY (id),
    CONSTRAINT fk_09_fct_panels_dashboard_id_08_fct_dashboards
        FOREIGN KEY (dashboard_id) REFERENCES "soma_analytics"."08_fct_dashboards"(id) ON DELETE RESTRICT,
    CONSTRAINT ck_09_fct_panels_chart_type CHECK (chart_type = ANY (ARRAY['area','bar','line','pie','radar','radial','table','number'])),
    CONSTRAINT ck_09_fct_panels_grid CHECK (grid_w > 0 AND grid_h > 0 AND grid_x >= 0 AND grid_y >= 0),
    CONSTRAINT ck_09_fct_panels_deleted CHECK (
        (is_deleted=false AND deleted_at IS NULL AND deleted_by IS NULL)
        OR (is_deleted=true AND deleted_at IS NOT NULL))
);
CREATE UNIQUE INDEX uq_09_fct_panels_dashboard_name
    ON "soma_analytics"."09_fct_panels" (dashboard_id, name) WHERE is_deleted = false;
CREATE INDEX idx_soma_analytics_09_fct_panels_dashboard_id
    ON "soma_analytics"."09_fct_panels" USING btree (dashboard_id);
COMMENT ON TABLE "soma_analytics"."09_fct_panels" IS 'One panel = a saved semantic query + chart type + grid position on a dashboard.';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."query_json" IS 'Opaque SemanticQuery document executed by the compiler. JSONB per DB-104 — stored as a whole document, never filtered by subfield, so no GIN index.';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."chart_type" IS '1:1 with soma-ui chart components: area|bar|line|pie|radar|radial|table|number.';
```

`query_json` uses JSONB per db-standards DB-104 — it is an opaque query document the compiler executes, never filtered by subfield, so no GIN index is warranted. Both `08_fct_dashboards` and `09_fct_panels` are `fct_` tables: soft-delete triplet, `ON DELETE RESTRICT` child FKs, and `COMMENT ON TABLE/COLUMN` (DB-307), consistent with the rest of §4.3. The `-- DOWN ==` section for this migration must drop `09_fct_panels` before `08_fct_dashboards` (FK-safe order).

**The query/result cache lives in Redis, not Postgres** — there is no cache table. Shape: §6.

> **No EAV here, deliberately.** The model schema is *known and structured* (cubes, measures, dimensions, joins, segments are first-class, individually CRUD'd and audited), so db-standards (DB-104) says normalize into columns — not JSONB blobs, not EAV. This is also why per-entity audit events are clean (§9).

---

## 5. The semantic query contract

### 5.1 Authoring format (declarative model — the on-disk source of truth)

YAML, applied via `POST /api/v1/model:apply` or `soma-cli apply`. 1:1 with the metadata tables.

```yaml
model_version: 1
data_sources:
  - name: warehouse           # driver: postgres → uses ANALYTICS_DB_URL (or a per-source encrypted DSN)
    driver: postgres
cubes:
  - name: orders
    data_source: warehouse
    sql_table: public.orders
    primary_key: id
    dimensions:
      - { name: status,     sql: status,     type: string }
      - { name: created_at, sql: created_at, type: time }
    measures:
      - { name: count,         type: count }
      - { name: total_revenue, sql: amount_cents, type: sum }
    segments:
      - { name: high_value, sql: "{CUBE}.amount_cents > 100000" }
    joins:
      - { name: customers, relationship: many_to_one, sql: "{CUBE}.customer_id = {customers}.id" }
  - name: customers
    data_source: warehouse
    sql_table: public.customers
    primary_key: id
    dimensions:
      - { name: region, sql: region, type: string }
```

### 5.2 Query in (JSON) — Cube-compatible member naming `cube.member`

```json
{
  "cube": "orders",
  "measures": ["orders.count", "orders.total_revenue"],
  "dimensions": ["orders.status"],
  "filters": [{ "member": "orders.status", "operator": "equals", "values": ["completed", "shipped"] }],
  "segments": ["orders.high_value"],
  "timeDimension": { "member": "orders.created_at", "granularity": "month", "dateRange": ["2026-01-01", "2026-06-30"] },
  "order": [["orders.total_revenue", "desc"]],
  "limit": 1000
}
```

### 5.3 Compiler contract (pure, in `soma-semantic`)

```rust
pub struct SemanticQuery {
    pub cube: String,
    pub measures: Vec<String>,            // "cube.measure"
    pub dimensions: Vec<String>,          // "cube.dimension"
    pub filters: Vec<Filter>,
    pub segments: Vec<String>,
    pub time_dimension: Option<TimeDimension>,
    pub order: Vec<(String, Order)>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}
pub struct Filter { pub member: String, pub operator: FilterOp, pub values: Vec<String> }
pub enum FilterOp { Equals, NotEquals, Contains, Gt, Gte, Lt, Lte, Set, NotSet, InDateRange }
pub struct TimeDimension { pub member: String, pub granularity: Granularity, pub date_range: Option<[String; 2]> }
// date_range: [0] is inclusive lower bound; [1] is inclusive upper bound supplied by the caller.
// The compiler converts [1] to an exclusive `< $N` bound internally (adds one grain/day).
// Granularity::Week = ISO week, Monday-start (date_trunc('week', ...) in Postgres).
pub enum Granularity { Day, Week, Month, Quarter, Year }

/// Security context — injected by the API layer from the verified Principal. NEVER from the caller.
pub struct QueryScope { pub tenant_id: Uuid, pub row_filters: Vec<RowFilter> }
pub struct RowFilter { pub member: String, pub value: String }

pub struct CompiledQuery { pub sql: String, pub binds: Vec<SqlValue>, pub columns: Vec<ColumnMeta> }
pub struct ColumnMeta { pub name: String, pub data_type: ColumnType }  // string|number|time|boolean

/// THE PURE ENTRY POINT — no I/O. Also the AI validation gate (§8).
pub fn compile(model: &Model, q: &SemanticQuery, scope: &QueryScope) -> Result<CompiledQuery, CompileError>;

pub enum CompileError {
    UnknownCube(String), UnknownMember(String), UnknownSegment(String),
    // Note: UndeclaredJoinCardinality was removed — join-cardinality presence is a type-level
    // invariant (`relationship` is a required field on `Join`) + DB CHECK constraint, not a
    // runtime compile error.
    UnreachableMember { member: String, from_cube: String },  // no join path
    UnsupportedFanout(String),                                 // beyond two-leg in Phase 1
    NonAdditiveMeasureInFanout { measure: String, cube: String, agg_type: String },
}
```

**Validation gate (the whole safety story):** every `cube`, `measures[]`, `dimensions[]`, `segments[]`, and `timeDimension.member` is resolved against `model` (the whitelist) **before any SQL string is built**. Unknown members → `CompileError`. User-supplied filter **values** become **bind parameters** (`sqlx`), never interpolated (db-standards DB-1101). Model-authored `sql_expr`/`sql_on`/segment SQL is trusted (it's governed model content, not request input) and is templated in. `scope.tenant_id` + `scope.row_filters` are appended as mandatory bound `AND` conditions. After resolving all members, collect the set of distinct cube names referenced; if more than 2, return `UnsupportedFanout` (Phase 1 supports at most a single one-hop join — see §0). This is a simple count, not a graph traversal.

**Test plan note:** a query referencing members from three cubes must return `UnsupportedFanout` (not `UnreachableMember`, not compiled SQL).

**Defense-in-depth on model-authored SQL:** `soma_semantic::validate_sql_fragment(&str) -> Result<(), ModelError>` rejects fragments containing `--`, `/*`, `*/`, `;`, or the keywords UNION/DROP/ALTER/CREATE/INSERT/UPDATE/DELETE/TRUNCATE/GRANT/REVOKE (case-insensitive, word-boundary). It runs at model-save time (both `POST /model:apply` and entity CRUD) on every `sql_expr`, segment `sql_expr`, `sql_on`, and `base_sql`. For `sql_on`, every `{token}` must be `{CUBE}` or a declared cube name. The compiler's `{CUBE}`/`{name}` substitution emits **double-quoted SQL identifiers** (`"name"`, internal `"` escaped), not raw string substitution. 'Editor role' ≠ 'trusted SQL author', so this validation augments the 'governed content' reasoning.

**Row-filter scope validation:** `scope.row_filters` from an embed token are NOT pre-trusted: each `RowFilter.member` must resolve through the SAME dimension whitelist as `SemanticQuery` members; an unresolvable member is `CompileError::UnknownMember`. Only the model-resolved, quoted column identifier is used in the bound `AND` condition.

### 5.4 Worked example — query → SQL

The §5.2 query compiles to (single cube, no join needed):

```sql
SELECT
  "orders".status                          AS "orders.status",
  date_trunc('month', "orders".created_at) AS "orders.created_at.month",
  count(*)                                 AS "orders.count",
  sum("orders".amount_cents)               AS "orders.total_revenue"
FROM public.orders AS "orders"
WHERE "orders".status = ANY($1)            -- filter values   (bind: {completed,shipped}::text[])
  AND "orders".amount_cents > 100000       -- segment high_value (model-authored, trusted)
  AND "orders".created_at >= $2            -- dateRange lower  (bind: 2026-01-01)
  AND "orders".created_at <  $3            -- dateRange upper, exclusive (bind: 2026-07-01)
  AND "orders".tenant_col = $4             -- scope.row_filters / tenant (bind), if the cube is tenant-scoped
GROUP BY 1, 2
ORDER BY "orders.total_revenue" DESC
LIMIT $5;                                  -- bind: 1000
```

Returned:
```json
{
  "columns": [
    {"name": "orders.status", "type": "string"},
    {"name": "orders.created_at.month", "type": "time"},
    {"name": "orders.count", "type": "number"},
    {"name": "orders.total_revenue", "type": "number"}
  ],
  "rows": [["completed", "2026-01-01T00:00:00Z", 1280, 4210000]],
  "meta": { "cache": "miss", "query_fingerprint": "sha256:1f3a…", "row_count": 1, "duration_ms": 34 }
}
```

**Multi-fact (fan-out) case — the two-leg pattern:** if a query selects measures from two cubes joined `one_to_many` (e.g. `orders` ↔ `line_items`), naïvely joining then aggregating double-counts the `one` side. Phase 1 emits **two sub-SELECTs**, each aggregating its own cube at the shared-dimension grain, then `FULL JOIN`s them on the shared dimension keys:

```sql
SELECT d.region, o.order_count, li.units
FROM   (SELECT c.region, count(*) AS order_count FROM orders o JOIN customers c ON … GROUP BY c.region) o
FULL JOIN (SELECT c.region, sum(li.qty) AS units    FROM line_items li JOIN … GROUP BY c.region) li USING (region)
…
```

This is correct without symmetric-aggregate (MD5 dedup) math, which is deferred (§13). The compiler **selects** this strategy from the declared `relationship`; an undeclared cardinality is a hard `CompileError`, never a silent default.

In two-leg mode the compiler iterates the resolved measures of the joined (non-root) cube and rejects any with `agg_type ∈ {avg, count_distinct, number}` via `NonAdditiveMeasureInFanout` — these are not safe to combine per-leg. `sum`/`count`/`min`/`max` remain valid (min/max per-leg equals the global min/max; sum/count are additive). Note: time-grain `date_trunc` runs under a UTC session (data-source pool sets `timezone='UTC'`), so month/week buckets are environment-independent.

---

## 6. Caching design (Phase-1 stand-in for pre-aggregations)

`soma-infra` cache is raw-bytes (`get`/`set_ex` over a `ConnectionManager`; **no `get_or_set`**), so the read-through is implemented inline in `soma-storage`:

```
key = "saq:v1:" + sha256_hex(canonical_json{ tenant_id, cube, measures, dimensions, filters,
                                              segments, time_dimension, order, limit, offset,
                                              row_filters, cube.model_version })
1. let hit = cache::get(&cm, &key).await?      // Some(bytes) → deserialize → return {cache:"hit"}
2. miss → compile + execute against the data-source pool
3. cache::set_ex(&cm, &key, &bytes, cube.cache_ttl_secs).await   // TTL = per-cube 03_fct_cubes.cache_ttl_secs
4. return {cache:"miss"}
```

- **Key correctness:** the key **must** include `tenant_id` and `row_filters` (so scoped/embedded results never leak across tenants) and `cube.model_version`. Canonical JSON = object keys sorted at every level; the string arrays (`measures`, `dimensions`, `segments`) sorted lexicographically; `row_filters` sorted by `(member, value)` before hashing. Unit test: two scopes with the same `row_filters` in different order must produce the same cache key.
- **Invalidation = model_version bump, not key scanning.** Any model mutation to a cube or its children bumps `03_fct_cubes.model_version` in the same write transaction. New queries use the new version → old cache entries become unreachable and expire by TTL. This avoids Redis key-pattern deletion (not exposed by the soma-infra API) entirely. ✅ ponytail.
- **Data-freshness:** Phase 1 relies on TTL for data changes in the *source* tables. A LookML-style **datagroup sentinel** (`MAX(updated_at)` probe folded into the key) is the named Phase-2 upgrade for instant freshness. A data-event-driven freshness upgrade (refreshKey/datagroup probe) replacing TTL-only is planned for Phase 2.5 — see docs/03 §3.
- DB is the source of truth; cache is a pure accelerator. A Redis outage degrades to direct execution, never to wrong data.

---

## 7. `main.rs` boot sequence (`crates/soma-server`) — matches the soma-vault skeleton

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Telemetry
    soma_infra::telemetry::init();

    // 2. Config (env)
    let bind_addr      = soma_infra::config::env_or("SOMA_BIND", "0.0.0.0:8080");
    let analytics_url  = soma_infra::config::require_env("ANALYTICS_DB_URL")?;   // the queried source
    let embed_secret   = soma_infra::config::require_env("ANALYTICS_EMBED_SECRET")?;
    // Only required if any data source stores a per-source DSN; Phase-1 default uses ANALYTICS_DB_URL.
    let dsn_kek        = soma_infra::crypto::CryptoKey::from_env_optional("ANALYTICS_DSN_KEK_HEX");
    // CORS: empty/absent ANALYTICS_CORS_ORIGINS → same-origin only (no cross-origin). Fail-closed.
    // If no origins are set and embedding is likely intended, a startup warning is logged.
    // cors_layer enumerates allowed methods (POST, GET, OPTIONS) and headers (Authorization, Content-Type) — never wildcard.
    let cors_origins   = soma_infra::config::env_or("ANALYTICS_CORS_ORIGINS", "");
    if cors_origins.trim().is_empty() {
        tracing::warn!("ANALYTICS_CORS_ORIGINS is not set — CORS disabled (same-origin only). Set this env var if embedding into external products.");
    }

    // 3. Metadata pool (DATABASE_URL)
    let pool = soma_infra::connect_from_env().await?;

    // 4. Migrate (soma-schema; schema soma_analytics, advisory key 0x50A1_A7C5_0001)
    soma_storage::migrate(&pool).await?;

    // 5. Audit: install schema + sink (embedded LocalSink pattern)
    soma_audit_pg::install(&pool).await?;
    let audit_keys = std::sync::Arc::new(soma_audit_pg::AuditKeys::from_env_local()?);
    let audit_sink = std::sync::Arc::new(
        soma_audit_pg::LocalSink::new(pool.clone(), audit_keys, "soma-analytics"));

    // 6. Data-source pool (separate; the thing we query)
    // NOTE: after_connect hook sets `SET timezone = 'UTC'` on every connection so date_trunc month/week
    //       buckets are environment-independent regardless of server locale.
    let ds_pool = soma_infra::db::connect(&soma_infra::db::PoolConfig::new(analytics_url)).await?;

    // 7. Cache (REDIS_URL)
    let cache = soma_infra::cache::connect_from_env().await?;

    // 8. App state + verifier (LocalTokenVerifier = the IAM seam)
    let state = soma_api::AppState::new(pool, ds_pool, cache, audit_sink, embed_secret, dsn_kek);

    // 9. Router (+ CORS for embedding, + SPA fallback for the deferred dashboard)
    let app = soma_api::router(state)
        .layer(soma_api::cors_layer(&cors_origins))
        .fallback(|uri: axum::http::Uri| async move { soma_infra::web::serve_spa::<Dashboard>(&uri) })
        .layer(tower_http::trace::TraceLayer::new_for_http());

    // 10. Serve with graceful shutdown
    soma_infra::web::serve_with_shutdown(&bind_addr, app).await?;
    Ok(())
}
```

(`AuditKeys::from_env_local()` — not `from_env()` — is the embedded path: it reads only `SOMA_AUDIT_MASTER_SECRET` and generates an ephemeral signing key, which is what an embedded service sink wants.)

---

## 8. The AI seam (NL → semantic query) — Phase 1.5, behind a flag

A single endpoint, gated by `env_parse::<bool>("ANALYTICS_AI_ENABLED")` (default off). The LLM emits a **semantic query**, never SQL, and it passes through the **same `soma_semantic::compile` validation gate** as `/query`.

```
POST /api/v1/ai/query   { "question": "monthly revenue by status for completed orders this year" }

1. principal.require(Reader); build QueryScope from Principal (tenant + row_filters)
2. meta = governed model for tenant (same payload as GET /meta) — measures/dimensions + descriptions
3. let client = soma_infra::llm::LlmClient::from_env()?;     // Anthropic, feature llm
   client.messages(&MessagesRequest {
       model, max_tokens,
       system: Some(SEMANTIC_QUERY_SYSTEM_PROMPT),           // "emit ONLY a SemanticQuery JSON for this model"
       messages: vec![Message{ role: Role::User, content: format!("{meta}\n\nQ: {question}") }],
       tools: Some(semantic_query_tool_schema()),            // forces structured SemanticQuery output
   }).await?
4. parse → SemanticQuery
5. soma_semantic::compile(&model, &q, &scope)  →  on Err(UnknownMember/…) return 422 {"error":"out_of_scope", member}
6. execute via the SAME /query path (cache + data-source pool); audit "query.executed" with metadata.source="ai"
```

- **No raw text-to-SQL fallback** (would void the governance guarantee). Out-of-scope → structured `422 out_of_scope`. A supervised, disclosed escape hatch is Phase 2.
- `LlmClient`, `LlmConfig`, `MessagesRequest`, `Message`, `Role` are all from `soma_infra::llm` — **no hand-rolled Anthropic client.**

**AI report/panel generation:** the same seam authors dashboards — NL → a validated `SemanticQuery` (via the §5.3 gate) → a suggested `chart_type` inferred from the selected dimensions/measures (e.g. time dimension ⇒ line/area, single category ⇒ bar/pie, single measure no dimension ⇒ number) → a draft `09_fct_panels` row the user reviews before save. The AI emits governed panels, never raw SQL — this is how "AI dashboards/reports" are produced.

---

## 9. Audit taxonomy

Mutations are audited **in the write transaction** (`record_in_tx(&event, &mut tx)`); reads/best-effort use `record(&event)`. Action names use `{resource}.{verb}`:

| Action | When | Path | Outcome |
|---|---|---|---|
| `datasource.connected` / `.updated` / `.deleted` | data-source CRUD | `record_in_tx` | Success / Error |
| `cube.created` / `.updated` / `.deleted` | cube CRUD (bumps `model_version`) | `record_in_tx` | Success |
| `dimension.*` / `measure.*` / `join.*` / `segment.*` | child CRUD (bumps parent `model_version`) | `record_in_tx` | Success |
| `model.applied` | bulk YAML apply | `record_in_tx` (one per affected entity) | Success |
| `query.executed` | every `/query` and `/ai/query` | `record` (best-effort) | Success / Error |
| `embed_token.minted` | `/embed/token` | `record` | Success |
| `dashboard.created` / `.updated` / `.deleted` | dashboard CRUD | `record_in_tx` | Success |
| `panel.created` / `.updated` / `.deleted` | panel CRUD (nested under dashboard) | `record_in_tx` | Success |
| `auth.denied` | authz failure on any route | `record` | **Denied** |

```rust
let ev = AuditEvent::builder(principal.tenant_id, "cube.updated", Outcome::Success)
    .source_service("soma-analytics")
    .actor_id(principal_subject_uuid)
    .resource("cube", cube_id.to_string())         // NOTE: .resource(type, id) — TWO args (no name)
    .metadata(serde_json::json!({ "name": cube_name, "model_version": new_version }))
    .build();
sink.record_in_tx(&ev, &mut tx).await?;            // NOTE: (event, tx) order
tx.commit().await?;
// NOTE: actor_id is a Uuid: for API-key principals use the token row's `id`; for embed principals
// parse the <user_id> UUID out of `subject` ("embed:<user_id>") — do not unwrap the whole subject string.
```

**Read-query auditing decision:** Phase 1 audits **every** `query.executed` **best-effort** (`record()`, own txn, after auth) — governance cares about *access*, and best-effort keeps it off the hot write-transaction path. The metadata carries cube + members + `cache` hit/miss + `source` (api|ai|embed). If volume becomes a problem, the documented lever is sampling/disable via an env flag — **not built in Phase 1** (YAGNI). Cache **hits** are still audited (an access occurred); they just don't touch the data source.

---

## 10. API surface

All routes under `/api/v1`. Auth: `Authorization: Bearer <token>` (admin API key or embed token) → `Principal`. Errors are redacted via `soma_infra::errors::redact_db_error` (never raw Postgres text).

**Query & introspection**
- `POST /query` — `{SemanticQuery}` → `{columns, rows, meta}`. Reader+. Cache-served. CORS-enabled (embedding).
- `GET  /meta` — governed, tenant-scoped measures/dimensions (+ descriptions). Reader+. Powers AI + host query-builders.
- `POST /ai/query` — `{question}` → result set (Phase 1.5, flag). Reader+.

  **Embed scope enforcement:** if `principal.scope` is `Embed { allowed_cube: Some(c), .. }` and the request's `cube != c`, return **403** before calling `compile()`. `GET /meta` with a scoped embed token returns only that cube's members.

**Model / admin (audited, Editor+ to write, Admin for data sources)**
- `POST /model:apply` — apply a full YAML/JSON model (upserts; per-entity audit). Editor+.
- `GET  /model` — export the tenant's model as YAML/JSON.
- `…/datasources` — `GET` list, `POST` create (`datasource.connected`), `PATCH /{id}`, `DELETE /{id}`. Admin.
- `…/cubes` — `GET`, `POST`, `PATCH /{id}`, `DELETE /{id}`. Editor+.
- `…/cubes/{id}/dimensions` | `/measures` | `/joins` | `/segments` — nested CRUD. Editor+.
- `…/tokens` — admin API-key CRUD (returns plaintext once on create). Admin.
- `…/dashboards` — `GET` list, `POST` create (`dashboard.created`), `PATCH /{id}` (`dashboard.updated`), `DELETE /{id}` (`dashboard.deleted`). Editor+. All writes audited.
- `…/dashboards/{id}/panels` — nested panel CRUD (`panel.created` / `.updated` / `.deleted`). Editor+. All writes audited.
- `GET /dashboards/{id}` — returns the dashboard definition + its panels for rendering (Reader+; the client then runs each panel's saved `query_json` through `POST /query`, which is cache-served).

**Embedding**

- `POST /embed/token` — `{user_id, row_filters:[{column,value}]}` → `{token, expires_at}`. Authenticated by an admin/Editor key (the host product's backend calls this). Mints a ≤10-min HMAC-signed token (§11). The minted token's `tenant_id` is always `principal.tenant_id` from the verified caller — the caller cannot choose which tenant the token scopes to.

Request/response are JSON; the query/result shapes are §5. List endpoints use keyset pagination (`(created_at, id)` cursor, db-standards DB-1109) — though Phase-1 model sizes are small.

---

## 11. Embedding & embedded dashboards

### 11.1 Data access to business data (read-only; one source first)

soma-analytics reads the data it reports on **directly from the target soma service's Postgres as a READ-ONLY user** (db-standards DB-1202 `READ_USER`) — the Grafana data-source model, requiring **no changes to the other services**.

- **Phase 1 source = soma-audit (confirmed).** `ANALYTICS_DB_URL` points at the shared Postgres instance's read-only user — the same single pool that reaches all service schemas. `02_fct_data_sources.dsn_ciphertext` stays NULL ⇒ resolves to `ANALYTICS_DB_URL`.
- **All soma services share ONE Postgres instance (confirmed), with separate schemas (`soma_audit`, `soma_iam`, …).** A single READ_USER pool already reaches all schemas cross-schema — no pool-per-source registry is needed for cross-service soma reports. `02_fct_data_sources` is a routing label (which schema/cube a model targets), not a DSN store. The pool-per-source registry path only returns if a **non-soma external warehouse** is added later (Phase 3 if ever).
- A source's tables become cubes exactly as §5 — e.g. a cube `audit_events` over `soma_audit."…_aud_events"`, with measures (counts, rates) and dimensions (event_type, source_service, time). Cross-schema cubes (e.g. joining `soma_audit` + `soma_iam` tables) work in the same single pool with no extra wiring.

### 11.2 The dashboard / report layer (new — exists nowhere on the platform)

A **panel** = a saved semantic query + chart type + grid position; a **dashboard** = a named set of panels. Stored in `08_fct_dashboards` + `09_fct_panels` (§4.3), CRUD'd + audited like the model. `chart_type ∈ {area,bar,line,pie,radar,radial,table,number}` maps **1:1 to the soma-ui charts that already exist**. `GET /dashboards/{id}` returns the definition; the client runs each panel's saved query through `POST /query` (cache-served) and feeds rows to the matching chart.

### 11.3 The shared soma-ui widget (platform rule: reusable UI → soma-ui)

soma-ui **already ships the chart primitives** (`AreaChart`, `BarChart`, `LineChart`, `PieChart`, `RadarChart`, `RadialChart` — pure-SVG Leptos, used in soma-observe). The only NEW UI is a thin composition layer added **to soma-ui**, not soma-analytics:

- `<AnalyticsPanel def=… token=…>` — binds a saved panel (query + chart_type) → calls soma-analytics → renders the matching soma-ui chart.
- `<Dashboard id=… token=…>` — a responsive grid of `<AnalyticsPanel>`s from a dashboard definition.

Each soma service's existing `dashboard/` Leptos crate then **drops in `<Dashboard>`** to get an embedded "Reports" page (e.g. soma-audit "Reports", soma-iam "Activity") over governed, AI-authorable metrics — no per-service chart/layout reinvention. (Internal embedding uses the platform's normal service auth / the §3 token seam; the §11.4 scoped-token path is for true external products.)

### 11.4 External headless embedding (3rd-party products) — secondary

Headless, no iframes (Part A §6). The **embed token** is a compact HMAC-signed token (JWT-shaped, but no JWT dependency — uses `soma_infra::crypto::hmac_sha256_hex`):

```
token   = base64url(payload_json) + "." + hmac_sha256_hex(ANALYTICS_EMBED_SECRET, base64url(payload_json))
payload = { "tenant_id": "...", "sub": "embed:<user_id>", "cube": "orders" | null,
            "row_filters": [{"column":"region","value":"EU"}], "exp": 1750000000 }
```

- Minted **server-side only** by the host's backend via `POST /embed/token` (never in a browser). Secret is separate from `DATABASE_URL`/internal auth.
- Verify: split on `.`, recompute HMAC, **constant-time compare** (see §14), check `exp`. Build an `Embed` `Principal` whose `scope` carries `allowed_cube` + `row_filters`. The `tenant_id` in the payload is always sourced from the minting caller's `Principal`, not the request body.
- `row_filters` are **structured equality objects** → injected as bound `AND` conditions by the API layer into `QueryScope`. **No arbitrary SQL in the token** (injection-safe; the Cube approach, not Superset's raw clause).
- Embed tokens are **query-only** and locked to the named **cube** — raw cube SQL is never reachable.
- CORS allow-list from `ANALYTICS_CORS_ORIGINS` (explicit origins; **never wildcard in prod**). Empty/absent `ANALYTICS_CORS_ORIGINS` ⇒ same-origin only (fail-closed). `cors_layer` enumerates allowed methods (`POST, GET, OPTIONS`) and headers (`Authorization, Content-Type`) — never wildcard. A startup warning is logged if no origins are configured (embedding likely intended).

**`sdk/js` — `@soma-analytics/client`** (the only non-Rust artifact; a ~100-line fetch wrapper, no runtime to operate, runs in the host's browser — justified because embedding into web products needs a browser client; explicitly **not** a Cube-style Node server):
```js
const soma = new SomaClient(fetchToken, { apiUrl });   // fetchToken: host calls its own /embed/token backend
const rs   = await soma.query({ cube: "orders", measures: ["orders.count"], dimensions: ["orders.status"] });
rs.tableData();  rs.series();   // adapters → feed the host's existing chart lib. soma ships ZERO chart code.
```
`fetchToken` is called transparently before expiry (no page reload). Charts are the host's concern — soma-ui already ships the primitives; the `<Dashboard>`/`<AnalyticsPanel>` composition widget (§11.3) is what ties them to saved panels.

---

## 12. SDK (Rust) — `crates/soma-sdk`

A `reqwest` client built on `soma_infra::http::client()`:
```rust
let c = SomaAnalytics::new(base_url, api_key);     // uses soma_infra::http::client()
c.apply_model(&yaml).await?;                        // POST /model:apply
c.list_cubes(tenant).await?;                        // GET  /cubes
c.meta().await?;                                    // GET  /meta
c.query(&SemanticQuery{ .. }).await? -> ResultSet;  // POST /query
c.mint_embed_token(tenant, user, row_filters).await?;
```
Non-Rust SDKs (beyond the JS embed client) are deferred.

---

## 13. Phase 2+ deferral list — what ponytail says NOT to build yet (named future home)

| Deferred | Named future home |
|---|---|
| **Pre-aggregations** (rollups) | Postgres `MATERIALIZED VIEW` rollups + a Tokio refresh scheduler + aggregate-awareness routing in `soma-semantic` (Phase 2); a pure-Rust embedded columnar engine (Apache DataFusion / Polars) as an in-process snapshot store (Phase 3, only if MV latency is a measured bottleneck); DuckDB (C++ FFI) is a fallback only. NOT `cubestored` (needs the Node orchestrator). See docs/03 §2. |
| **Multi-warehouse** (Snowflake/BigQuery/DuckDB) | A query-dispatch trait in **soma-infra** (mirrors `storage::StorageClient`) + per-dialect emitters in `soma-semantic` |
| **Postgres-wire SQL API** (Tableau/Superset/DBeaver) | New `crates/soma-pgwire` (the `pgwire` crate) exposing the same query contract — Phase 2 via a new `crates/soma-pgwire` (the `pgwire` crate) — unlocks Grafana/Superset/Metabase/Tableau with zero client changes; far simpler than Cube's `cubesql`. See docs/03 §5. |
| **Symmetric aggregates** (MD5 dedup) | `soma-semantic` — only where the two-leg pattern proves insufficient |
| **Dashboard definition layer + `<Dashboard>`/`<AnalyticsPanel>` soma-ui widget** | **Phase 1** — `08_fct_dashboards` + `09_fct_panels` (§4.3) + composition widget in soma-ui (§11.2/§11.3) |
| **AI panel-generation** (NL → governed panel → draft `09_fct_panels` row) | **Phase 1.5** — same `/ai/query` seam (§8); a full drag-drop self-service builder stays Phase 2 |
| **Raw text-to-SQL escape hatch** | `soma-api` `/ai/query`, supervised + disclosed (Phase 2) |
| **First-party chart components** | Already exist in soma-ui (`AreaChart`/`BarChart`/`LineChart`/`PieChart`/`RadarChart`/`RadialChart`). Only the `<Dashboard>`/`<AnalyticsPanel>` composition widget is new (Phase 1, §11.3). |
| **Multi-source connection registry** | Only for a non-soma external warehouse; the shared Postgres covers cross-service reports cross-schema (Phase 3 if ever). |
| **Real soma-iam integration** | Swap `LocalTokenVerifier` → `IamTokenVerifier` (identical `TokenVerifier` trait) at **M11+** |
| **Object storage / result export / data-lake** | `soma_infra::storage::StorageClient` (`storage-s3`/`storage-azure`) |
| SCD2 dims, HyperLogLog approx-distinct, WebSocket subscriptions, per-tenant compiled schemas | `soma-semantic` / `soma-api`, as scale demands |
| Cube-style **Views** (member-curated governance facade over cubes) | `soma-semantic` + a view-membership table (Phase 2) |
| **Filtered measures** (Malloy-style per-measure predicate) | `soma-semantic` + `filter_sql` column on `05_fct_measures` (Phase 2) |
| **Phase-2 embed hardening** | add `jti` (UUIDv4) to embed token + Redis revocation set (`saq:revoked:<jti>`, TTL=exp) for emergency revocation; add `kek_version INTEGER` to `02_fct_data_sources` for KEK rotation |
| **Data-freshness cache invalidation** (refreshKey/datagroup probe) | `refresh_key_sql` on `03_fct_cubes` + a ~30s probe; stale-while-revalidate via Tokio; optional `cache_mode` on the query (Phase 2.5). See docs/03 §3. |
| **Lightweight MCP server** (`list_cubes` + `run_query`) for external agents | Phase 2. |
| **NL→query eval harness** (execution-based, BIRD-style) | Phase 2. |
| **The Ontology layer (the Palantir trajectory)** | **`soma_infra::kg` + pgvector** — mapped below |

### The ontology, mapped onto soma-infra `kg` (so the path is named, not invented later)

| Foundry concept | soma-infra `kg` mapping |
|---|---|
| Object Type / object instance | `KgNode { id, kind, props }` via `upsert_node` (`kind` = object type; `props` = attributes) |
| Link Type (named, cardinal edge) | `KgEdge { id, src, dst, rel, props }` via `upsert_edge`; traverse with `neighbors(node, rel, dir, limit)` |
| Semantic / similarity search over objects | `vector_search_cosine(embedding, k)` (pgvector) |
| **Action Type (write-back)** | A new service-owned `Action` trait that validates + mutates objects/source rows (the *operational* leap; Phase 3) |
| Scenarios (what-if) | Deferred — branch object sets; out of scope |

A cube/measure can later be *projected over* an object set instead of a raw table, fusing the metrics layer with the ontology — but **not in Phase 1**. The substrate exists; we don't build on it yet.

---

## 14. Small soma-infra additions Phase 1 needs (THE LINE: generic primitive → infra)

Two genuinely-generic primitives soma-infra lacks; per the platform rule, **add them to soma-infra** rather than hand-roll in the service:

1. **`crypto::hmac_sha256_verify(key, msg, tag_hex) -> bool`** (or `crypto::ct_eq`) — a **constant-time** HMAC/string comparison for embed-token verification. Token verification is a trust boundary; a non-constant-time `==` on the HMAC is a (small) timing oracle. This is a decision-free crypto operation → it belongs in `soma_infra::crypto`.
2. *(Optional, nice-to-have)* `cache::get_or_set(cm, key, ttl, async fn)` — a read-through helper. Phase 1 inlines the get/compute/`set_ex` (3 lines), so this is **not** required; note it only if a second service wants the same pattern.

Everything else is consumed as-is. No Postgres pool, `tracing` init, shutdown future, AEAD, HMAC, SHA-256, or `reqwest` client is re-implemented in the service (CLAUDE.md "do not re-duplicate").

---

## 15. Open questions / decisions needing your call

1. **Advisory lock key** — proposed `0x50A1_A7C5_0001`. Confirm it's free on the shared Postgres (taken: vault `0x50A1_7A01_7017`, audit `6020250626000001`/outbox `…002`, iam `7318249506742315`).
2. **Tenant identity** — spec uses `tenant_id UUID` (consistent with soma-audit/soma-vault/iam, which all use UUID tenants and which the audit builder *requires*). This **deviates from db-standards DB-701** (`tenant_key VARCHAR(100)`). Recommend keeping UUID for platform consistency — confirm.
3. **Schema name** — `soma_analytics` (no `NN_` prefix), matching soma-audit's `soma_audit` convention rather than db-standards DB-201's `NN_name`. Confirm.
4. **Embed token** — HMAC-signed compact token now (no new dep, swap to RS256 JWT when soma-iam lands), vs. pulling in `jsonwebtoken` now. Recommend HMAC.
5. **Data-source credentials** — Phase-1 default = single `ANALYTICS_DB_URL` (read-only pool over the recommended soma-audit DB); per-source encrypted DSN in `02_fct_data_sources.dsn_ciphertext` is the path to multi-source (Phase 1.5). Confirm scope.
6. **Query auditing** — full best-effort (recommended, specced) vs. sampled vs. off. Confirm.
7. **JS embed client** — ship `@soma-analytics/client` in Phase 1 (the only non-Rust artifact; required to actually embed into web products). Confirm it's acceptable.
8. **Chart components** — soma-ui already ships `AreaChart`/`BarChart`/`LineChart`/`PieChart`/`RadarChart`/`RadialChart`. Phase 1 adds only the `<Dashboard>`/`<AnalyticsPanel>` composition widget to soma-ui (§11.3). Confirm scope of widget.
9. **soma-cli + dashboard scope** — Phase 1 ships `soma-cli` (apply model, run query); the Leptos `dashboard/` admin/authoring UI is **deferred**. Confirm, or pull a thin authoring UI into Phase 1.
10. **NL→query (AI seam)** — include as a flagged Phase-1.5 thin slice (recommended, specced in §8) or defer entirely to Phase 2.
11. **Phase-1 first source = soma-audit** — **RESOLVED: soma-audit.**
12. **Topology: shared Postgres or separate databases?** — **RESOLVED: one shared Postgres instance, separate schemas → single cross-schema READ_USER pool; no multi-source registry needed.**

Further design decisions surfaced by the Cube evaluation (pgwire timing, the embedded-engine trigger + choice, refreshKey probe shape, cheap-seam ponytail call, MCP appetite) are listed in `docs/03-cube-deep-evaluation.md` §9.

---

*End of Phase-1 spec. Pairs with Part A (competitor analysis). Next step: engineering review.*
