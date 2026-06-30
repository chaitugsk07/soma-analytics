-- Dashboard and panel tables for the soma-analytics embedded reports layer.
-- These are fct_ tables: same conventions as 02 (soft-delete triplet, RESTRICT FKs,
-- COMMENT ON all tables and columns, updated_at trigger, partial unique index).
-- query_json uses JSONB per DB-104 — the query document is opaque, stored whole,
-- and never filtered by subfield (no GIN index warranted).

-- ── 08_fct_dashboards ────────────────────────────────────────────────────────
-- A savable dashboard: a named, tenant-scoped set of panels.

CREATE TABLE "soma_analytics"."08_fct_dashboards" (
    id          UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id   UUID         NOT NULL,
    name        VARCHAR(120) NOT NULL,
    description TEXT,
    is_deleted  BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by  UUID,
    updated_by  UUID,
    deleted_at  TIMESTAMPTZ,
    deleted_by  UUID,
    CONSTRAINT pk_08_fct_dashboards
        PRIMARY KEY (id),
    CONSTRAINT ck_08_fct_dashboards_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_08_fct_dashboards_tenant_name
    ON "soma_analytics"."08_fct_dashboards" (tenant_id, name)
    WHERE is_deleted = false;

CREATE TRIGGER tg_08_fct_dashboards_updated_at
    BEFORE UPDATE ON "soma_analytics"."08_fct_dashboards"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."08_fct_dashboards"
    IS 'A savable dashboard/report — a named, tenant-scoped set of panels.';
COMMENT ON COLUMN "soma_analytics"."08_fct_dashboards"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."08_fct_dashboards"."name"
    IS 'Unique dashboard name per tenant (partial unique index WHERE is_deleted = false).';

-- ── 09_fct_panels ────────────────────────────────────────────────────────────
-- One panel = a saved semantic query + chart type + grid position on a dashboard.
-- chart_type values map 1:1 to soma-ui chart components.

CREATE TABLE "soma_analytics"."09_fct_panels" (
    id           UUID         NOT NULL DEFAULT gen_random_uuid(),
    tenant_id    UUID         NOT NULL,
    dashboard_id UUID         NOT NULL,
    name         VARCHAR(120) NOT NULL,
    chart_type   VARCHAR(20)  NOT NULL,  -- area | bar | line | pie | radar | radial | table | number
    query_json   JSONB        NOT NULL,  -- opaque SemanticQuery document; executed by the compiler
    grid_x       INTEGER      NOT NULL DEFAULT 0,
    grid_y       INTEGER      NOT NULL DEFAULT 0,
    grid_w       INTEGER      NOT NULL DEFAULT 6,
    grid_h       INTEGER      NOT NULL DEFAULT 4,
    is_deleted   BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    created_by   UUID,
    updated_by   UUID,
    deleted_at   TIMESTAMPTZ,
    deleted_by   UUID,
    CONSTRAINT pk_09_fct_panels
        PRIMARY KEY (id),
    CONSTRAINT fk_09_fct_panels_dashboard_id_08_fct_dashboards
        FOREIGN KEY (dashboard_id)
        REFERENCES "soma_analytics"."08_fct_dashboards" (id)
        ON DELETE RESTRICT,
    CONSTRAINT ck_09_fct_panels_chart_type
        CHECK (chart_type = ANY (ARRAY['area', 'bar', 'line', 'pie', 'radar', 'radial', 'table', 'number'])),
    CONSTRAINT ck_09_fct_panels_grid
        CHECK (grid_w > 0 AND grid_h > 0 AND grid_x >= 0 AND grid_y >= 0),
    CONSTRAINT ck_09_fct_panels_deleted
        CHECK (
            (is_deleted = false AND deleted_at IS NULL     AND deleted_by IS NULL)
            OR
            (is_deleted = true  AND deleted_at IS NOT NULL)
        )
);

CREATE UNIQUE INDEX uq_09_fct_panels_dashboard_name
    ON "soma_analytics"."09_fct_panels" (dashboard_id, name)
    WHERE is_deleted = false;

CREATE INDEX idx_soma_analytics_09_fct_panels_dashboard_id
    ON "soma_analytics"."09_fct_panels" USING btree (dashboard_id);

CREATE TRIGGER tg_09_fct_panels_updated_at
    BEFORE UPDATE ON "soma_analytics"."09_fct_panels"
    FOR EACH ROW EXECUTE FUNCTION "soma_analytics".fn_update_timestamp();

COMMENT ON TABLE  "soma_analytics"."09_fct_panels"
    IS 'One panel = a saved semantic query + chart type + grid position on a dashboard.';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."tenant_id"
    IS 'PII: indirect — identifies the owning tenant.';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."query_json"
    IS 'Opaque SemanticQuery document executed by the compiler. JSONB per DB-104 — stored as a whole document, never filtered by subfield, so no GIN index.';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."chart_type"
    IS '1:1 with soma-ui chart components: area | bar | line | pie | radar | radial | table | number.';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."grid_x"
    IS 'Column offset on the dashboard grid (0-indexed).';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."grid_y"
    IS 'Row offset on the dashboard grid (0-indexed).';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."grid_w"
    IS 'Width in grid columns (must be > 0).';
COMMENT ON COLUMN "soma_analytics"."09_fct_panels"."grid_h"
    IS 'Height in grid rows (must be > 0).';

-- DOWN ==
-- Teardown in FK-safe reverse-dependency order (panel before dashboard).

DROP TABLE IF EXISTS "soma_analytics"."09_fct_panels";
DROP TABLE IF EXISTS "soma_analytics"."08_fct_dashboards";
