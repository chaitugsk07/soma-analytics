-- Model-metadata tables for the soma-analytics semantic layer.
--
-- Conventions applied:
--   - Double-quoted "NN_type_descriptor" names (DB-301)
--   - UUID PKs: DEFAULT gen_random_uuid() (DB-102)
--   - tenant_id UUID NOT NULL (matches soma-vault / soma-audit UUID tenants; see §15)
--   - Soft-delete triplet (is_deleted, deleted_at, deleted_by) + bidirectional CHECK
--   - ON DELETE RESTRICT on child FKs (service only soft-deletes; hard DELETE must
--     explicitly remove children first to preserve the audit trail)
--   - Partial unique indexes WHERE is_deleted = false
--   - COMMENT ON TABLE/COLUMN for every table (DB-307), PII tags where applicable (DB-1206)
--   - fn_update_timestamp() trigger on every entity table
--   - No RLS (DB-108 — tenancy enforced in the query layer)
--   - No EAV (DB-104 — model is structured, normalised into first-class tables)

-- ── 01_fct_api_tokens ────────────────────────────────────────────────────────
-- Local placeholder API tokens per tenant (the IAM-seam; swapped for soma-iam at M11+).
-- High-entropy tokens → sha256 lookup is sufficient (db-standards DB-1101).

CREATE TABLE "soma_analytics"."01_fct_api_tokens" (
    id           UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id    UUID         NOT NULL,
    token_sha256 VARCHAR(64)  NOT NULL,  -- sha256_hex(token); plaintext shown once at creation
    name         VARCHAR(120) NOT NULL,
    role         VARCHAR(20)  NOT NULL DEFAULT 'reader',
    expires_at   TIMESTAMPTZ,
    is_deleted   BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by   UUID,
    updated_by   UUID,
    deleted_at   TIMESTAMPTZ,
    deleted_by   UUID,
    CONSTRAINT pk_01_fct_api_tokens
        PRIMARY KEY (id),
    CONSTRAINT ck_01_fct_api_tokens_sha256_len
        CHECK (length(token_sha256) = 64),
    CONSTRAINT ck_01_fct_api_tokens_role
        CHECK (role = ANY (ARRAY['reader', 'editor', 'admin'])),
    CONSTRAINT ck_01_fct_api_tokens_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_01_fct_api_tokens_sha
    ON "soma_analytics"."01_fct_api_tokens" (token_sha256)
    WHERE is_deleted = false;

CREATE TRIGGER tg_01_fct_api_tokens_updated_at
    BEFORE UPDATE ON "soma_analytics"."01_fct_api_tokens"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."01_fct_api_tokens"
    IS 'Local placeholder API tokens per tenant (IAM seam — swap for soma-iam at M11+).';
COMMENT ON COLUMN "soma_analytics"."01_fct_api_tokens"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."01_fct_api_tokens"."token_sha256"
    IS 'PII: indirect — sha256_hex of the API token; plaintext is shown only once at creation.';
COMMENT ON COLUMN "soma_analytics"."01_fct_api_tokens"."role"
    IS 'Caller role: reader | editor | admin. Enforced by CHECK constraint.';
COMMENT ON COLUMN "soma_analytics"."01_fct_api_tokens"."expires_at"
    IS 'Optional expiry. NULL = never expires. Enforced in the query layer (not RLS).';

-- ── 02_fct_data_sources ──────────────────────────────────────────────────────
-- A configured Postgres data source per tenant.
-- Phase-1 default: dsn_ciphertext IS NULL → use ANALYTICS_DB_URL env pool.
-- Non-NULL: crypto::encrypt(KEK, dsn, aad=tenant_id bytes) for a per-source DSN.

CREATE TABLE "soma_analytics"."02_fct_data_sources" (
    id             UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id      UUID         NOT NULL,
    name           VARCHAR(120) NOT NULL,   -- business key (human name / code)
    driver         VARCHAR(20)  NOT NULL DEFAULT 'postgres',
    dsn_ciphertext BYTEA,                   -- NULL ⇒ env pool. Non-NULL ⇒ AES-256-GCM encrypted DSN.
    is_deleted     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by     UUID,
    updated_by     UUID,
    deleted_at     TIMESTAMPTZ,
    deleted_by     UUID,
    CONSTRAINT pk_02_fct_data_sources
        PRIMARY KEY (id),
    CONSTRAINT ck_02_fct_data_sources_driver
        CHECK (driver = ANY (ARRAY['postgres'])),
    CONSTRAINT ck_02_fct_data_sources_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_02_fct_data_sources_tenant_name
    ON "soma_analytics"."02_fct_data_sources" (tenant_id, name)
    WHERE is_deleted = false;

CREATE TRIGGER tg_02_fct_data_sources_updated_at
    BEFORE UPDATE ON "soma_analytics"."02_fct_data_sources"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."02_fct_data_sources"
    IS 'Configured Postgres data source per tenant. Phase-1 default: NULL dsn_ciphertext = ANALYTICS_DB_URL.';
COMMENT ON COLUMN "soma_analytics"."02_fct_data_sources"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."02_fct_data_sources"."dsn_ciphertext"
    IS 'PII: sensitive — AES-256-GCM-encrypted DSN; plaintext never stored. NULL = use the env data-source pool.';

-- ── 03_fct_cubes ─────────────────────────────────────────────────────────────
-- A cube over a base table OR SQL. model_version bumps on any mutation to the
-- cube or any of its child entities (cache-invalidation lever; see §6).

CREATE TABLE "soma_analytics"."03_fct_cubes" (
    id             UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id      UUID         NOT NULL,
    data_source_id UUID         NOT NULL,
    name           VARCHAR(120) NOT NULL,
    title          VARCHAR(200),
    description    TEXT,
    sql_table      VARCHAR(300),  -- exactly one of (sql_table, base_sql) must be set (CHECK below)
    base_sql       TEXT,
    primary_key    VARCHAR(120) NOT NULL,  -- required for fan-out correctness
    cache_ttl_secs INTEGER      NOT NULL DEFAULT 300,
    model_version  INTEGER      NOT NULL DEFAULT 1,   -- bumped on every mutation; drives cache key
    tenant_column  VARCHAR(120) NOT NULL DEFAULT 'tenant_id',  -- column used for structural tenant isolation in compiled queries
    is_deleted     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by     UUID,
    updated_by     UUID,
    deleted_at     TIMESTAMPTZ,
    deleted_by     UUID,
    CONSTRAINT pk_03_fct_cubes
        PRIMARY KEY (id),
    CONSTRAINT fk_03_fct_cubes_data_source_id_02_fct_data_sources
        FOREIGN KEY (data_source_id)
        REFERENCES "soma_analytics"."02_fct_data_sources" (id)
        ON DELETE RESTRICT,
    CONSTRAINT ck_03_fct_cubes_source
        CHECK (
            (sql_table IS NOT NULL AND base_sql IS NULL)
            OR
            (sql_table IS NULL     AND base_sql IS NOT NULL)
        ),
    CONSTRAINT ck_03_fct_cubes_primary_key
        CHECK (length(trim(primary_key)) > 0),
    CONSTRAINT ck_03_fct_cubes_sql_table_nonempty
        CHECK (sql_table IS NULL OR length(trim(sql_table)) > 0),
    CONSTRAINT ck_03_fct_cubes_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_03_fct_cubes_tenant_name
    ON "soma_analytics"."03_fct_cubes" (tenant_id, name)
    WHERE is_deleted = false;

CREATE INDEX idx_soma_analytics_03_fct_cubes_data_source_id
    ON "soma_analytics"."03_fct_cubes" USING btree (data_source_id);

CREATE TRIGGER tg_03_fct_cubes_updated_at
    BEFORE UPDATE ON "soma_analytics"."03_fct_cubes"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."03_fct_cubes"
    IS 'A cube: a logical table/view with typed dimensions, measures, joins, and segments. Exactly one of sql_table or base_sql must be set.';
COMMENT ON COLUMN "soma_analytics"."03_fct_cubes"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."03_fct_cubes"."model_version"
    IS 'Incremented on every mutation to this cube or its child entities (dimensions, measures, joins, segments). Used as a cache-key component for result-cache invalidation (§6).';
COMMENT ON COLUMN "soma_analytics"."03_fct_cubes"."sql_table"
    IS 'Fully-qualified table reference, e.g. public.orders. Mutually exclusive with base_sql.';
COMMENT ON COLUMN "soma_analytics"."03_fct_cubes"."base_sql"
    IS 'Raw SQL SELECT used as a subquery. Mutually exclusive with sql_table. Validated with validate_sql_fragment() at save time.';
COMMENT ON COLUMN "soma_analytics"."03_fct_cubes"."cache_ttl_secs"
    IS 'TTL in seconds for result-cache entries (Redis SETEX). Default 300 (5 min).';
COMMENT ON COLUMN "soma_analytics"."03_fct_cubes"."tenant_column"
    IS 'The column name in the base table used for structural tenant isolation. Emitted as a mandatory WHERE predicate by the compiler for every query.';

-- ── 04_fct_dimensions ────────────────────────────────────────────────────────

CREATE TABLE "soma_analytics"."04_fct_dimensions" (
    id          UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id   UUID         NOT NULL,
    cube_id     UUID         NOT NULL,
    name        VARCHAR(120) NOT NULL,
    description TEXT,
    sql_expr    TEXT         NOT NULL,  -- expression over {CUBE}, e.g. "{CUBE}.status"
    data_type   VARCHAR(20)  NOT NULL,  -- string | number | time | boolean
    is_deleted  BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by  UUID,
    updated_by  UUID,
    deleted_at  TIMESTAMPTZ,
    deleted_by  UUID,
    CONSTRAINT pk_04_fct_dimensions
        PRIMARY KEY (id),
    -- RESTRICT (not CASCADE): fct_ tables; service only soft-deletes. Hard DELETE must
    -- explicitly remove children to preserve the audit trail.
    CONSTRAINT fk_04_fct_dimensions_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id)
        REFERENCES "soma_analytics"."03_fct_cubes" (id)
        ON DELETE RESTRICT,
    CONSTRAINT ck_04_fct_dimensions_data_type
        CHECK (data_type = ANY (ARRAY['string', 'number', 'time', 'boolean'])),
    CONSTRAINT ck_04_fct_dimensions_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_04_fct_dimensions_cube_name
    ON "soma_analytics"."04_fct_dimensions" (cube_id, name)
    WHERE is_deleted = false;

CREATE INDEX idx_soma_analytics_04_fct_dimensions_cube_id
    ON "soma_analytics"."04_fct_dimensions" USING btree (cube_id);

CREATE TRIGGER tg_04_fct_dimensions_updated_at
    BEFORE UPDATE ON "soma_analytics"."04_fct_dimensions"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."04_fct_dimensions"
    IS 'A named, typed dimension on a cube. sql_expr may use {CUBE} token; validated at save time.';
COMMENT ON COLUMN "soma_analytics"."04_fct_dimensions"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."04_fct_dimensions"."sql_expr"
    IS 'Column or expression over {CUBE}, e.g. "{CUBE}.status". Validated with validate_sql_fragment() at save time.';
COMMENT ON COLUMN "soma_analytics"."04_fct_dimensions"."data_type"
    IS 'Semantic type: string | number | time | boolean. Enforced by CHECK constraint.';

-- ── 05_fct_measures ──────────────────────────────────────────────────────────

CREATE TABLE "soma_analytics"."05_fct_measures" (
    id          UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id   UUID         NOT NULL,
    cube_id     UUID         NOT NULL,
    name        VARCHAR(120) NOT NULL,
    description TEXT,
    sql_expr    TEXT,          -- column/expr to aggregate; NULL is valid only for agg_type = 'count'
    agg_type    VARCHAR(20)   NOT NULL,  -- count | count_distinct | sum | avg | min | max | number
    is_deleted  BOOLEAN       NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ   NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ   NOT NULL DEFAULT now(),
    created_by  UUID,
    updated_by  UUID,
    deleted_at  TIMESTAMPTZ,
    deleted_by  UUID,
    CONSTRAINT pk_05_fct_measures
        PRIMARY KEY (id),
    CONSTRAINT fk_05_fct_measures_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id)
        REFERENCES "soma_analytics"."03_fct_cubes" (id)
        ON DELETE RESTRICT,
    CONSTRAINT ck_05_fct_measures_sql_expr
        CHECK (agg_type = 'count' OR sql_expr IS NOT NULL),
    CONSTRAINT ck_05_fct_measures_agg
        CHECK (agg_type = ANY (ARRAY['count', 'count_distinct', 'sum', 'avg', 'min', 'max', 'number'])),
    CONSTRAINT ck_05_fct_measures_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_05_fct_measures_cube_name
    ON "soma_analytics"."05_fct_measures" (cube_id, name)
    WHERE is_deleted = false;

CREATE INDEX idx_soma_analytics_05_fct_measures_cube_id
    ON "soma_analytics"."05_fct_measures" USING btree (cube_id);

CREATE TRIGGER tg_05_fct_measures_updated_at
    BEFORE UPDATE ON "soma_analytics"."05_fct_measures"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."05_fct_measures"
    IS 'A named, aggregated measure on a cube. agg_type = count allows NULL sql_expr (count(*)); all others require it.';
COMMENT ON COLUMN "soma_analytics"."05_fct_measures"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."05_fct_measures"."sql_expr"
    IS 'Column/expression to aggregate. NULL only valid for agg_type = count. Validated with validate_sql_fragment().';
COMMENT ON COLUMN "soma_analytics"."05_fct_measures"."agg_type"
    IS 'Aggregation type: count | count_distinct | sum | avg | min | max | number. Enforced by CHECK.';

-- ── 06_fct_joins ─────────────────────────────────────────────────────────────
-- Single-level joins between cubes. Cardinality REQUIRED (fan-out gate in compiler).

CREATE TABLE "soma_analytics"."06_fct_joins" (
    id             UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id      UUID         NOT NULL,
    cube_id        UUID         NOT NULL,   -- the "from" cube
    target_cube_id UUID         NOT NULL,   -- the "to" cube
    name           VARCHAR(120) NOT NULL,
    relationship   VARCHAR(20)  NOT NULL,   -- many_to_one | one_to_many | one_to_one
    sql_on         TEXT         NOT NULL,   -- e.g. "{CUBE}.customer_id = {customers}.id"
    is_deleted     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by     UUID,
    updated_by     UUID,
    deleted_at     TIMESTAMPTZ,
    deleted_by     UUID,
    CONSTRAINT pk_06_fct_joins
        PRIMARY KEY (id),
    CONSTRAINT fk_06_fct_joins_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id)
        REFERENCES "soma_analytics"."03_fct_cubes" (id)
        ON DELETE RESTRICT,
    CONSTRAINT fk_06_fct_joins_target_cube_id_03_fct_cubes
        FOREIGN KEY (target_cube_id)
        REFERENCES "soma_analytics"."03_fct_cubes" (id)
        ON DELETE RESTRICT,
    CONSTRAINT ck_06_fct_joins_relationship
        CHECK (relationship = ANY (ARRAY['many_to_one', 'one_to_many', 'one_to_one'])),
    CONSTRAINT ck_06_fct_joins_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_06_fct_joins_cube_name
    ON "soma_analytics"."06_fct_joins" (cube_id, name)
    WHERE is_deleted = false;

CREATE INDEX idx_soma_analytics_06_fct_joins_cube_id
    ON "soma_analytics"."06_fct_joins" USING btree (cube_id);

CREATE INDEX idx_soma_analytics_06_fct_joins_target_cube_id
    ON "soma_analytics"."06_fct_joins" USING btree (target_cube_id);

CREATE TRIGGER tg_06_fct_joins_updated_at
    BEFORE UPDATE ON "soma_analytics"."06_fct_joins"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."06_fct_joins"
    IS 'Single-level join between two cubes. relationship is required (fan-out gate in the compiler).';
COMMENT ON COLUMN "soma_analytics"."06_fct_joins"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."06_fct_joins"."cube_id"
    IS 'The "from" (root) cube.';
COMMENT ON COLUMN "soma_analytics"."06_fct_joins"."target_cube_id"
    IS 'The "to" (joined) cube.';
COMMENT ON COLUMN "soma_analytics"."06_fct_joins"."sql_on"
    IS 'ON expression; may use {CUBE} and {target_cube_name} tokens. Validated at save time.';
COMMENT ON COLUMN "soma_analytics"."06_fct_joins"."relationship"
    IS 'Cardinality: many_to_one | one_to_many | one_to_one. Required — no default. Enforced by CHECK.';

-- ── 07_fct_segments ──────────────────────────────────────────────────────────
-- Named, reusable SQL predicate fragments per cube.

CREATE TABLE "soma_analytics"."07_fct_segments" (
    id          UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id   UUID         NOT NULL,
    cube_id     UUID         NOT NULL,
    name        VARCHAR(120) NOT NULL,
    sql_expr    TEXT         NOT NULL,  -- "{CUBE}.amount_cents > 100000" — model-authored, trusted
    is_deleted  BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by  UUID,
    updated_by  UUID,
    deleted_at  TIMESTAMPTZ,
    deleted_by  UUID,
    CONSTRAINT pk_07_fct_segments
        PRIMARY KEY (id),
    CONSTRAINT fk_07_fct_segments_cube_id_03_fct_cubes
        FOREIGN KEY (cube_id)
        REFERENCES "soma_analytics"."03_fct_cubes" (id)
        ON DELETE RESTRICT,
    CONSTRAINT ck_07_fct_segments_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_07_fct_segments_cube_name
    ON "soma_analytics"."07_fct_segments" (cube_id, name)
    WHERE is_deleted = false;

CREATE INDEX idx_soma_analytics_07_fct_segments_cube_id
    ON "soma_analytics"."07_fct_segments" USING btree (cube_id);

CREATE TRIGGER tg_07_fct_segments_updated_at
    BEFORE UPDATE ON "soma_analytics"."07_fct_segments"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."07_fct_segments"
    IS 'Named, reusable SQL predicate per cube. sql_expr is model-authored (trusted) and may use {CUBE} token.';
COMMENT ON COLUMN "soma_analytics"."07_fct_segments"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."07_fct_segments"."sql_expr"
    IS 'SQL predicate, e.g. "{CUBE}.amount_cents > 100000". Validated with validate_sql_fragment() at save time.';

-- DOWN ==
-- Teardown in FK-safe reverse-dependency order.
-- Child FKs are RESTRICT, so children must be removed before parents.
-- The service only soft-deletes at runtime; this DOWN is for dev/test teardown.

DROP TABLE IF EXISTS "soma_analytics"."07_fct_segments";
DROP TABLE IF EXISTS "soma_analytics"."06_fct_joins";
DROP TABLE IF EXISTS "soma_analytics"."05_fct_measures";
DROP TABLE IF EXISTS "soma_analytics"."04_fct_dimensions";
DROP TABLE IF EXISTS "soma_analytics"."03_fct_cubes";
DROP TABLE IF EXISTS "soma_analytics"."02_fct_data_sources";
DROP TABLE IF EXISTS "soma_analytics"."01_fct_api_tokens";
