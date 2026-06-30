-- Enable pgcrypto in the shared public schema so gen_random_uuid() and other
-- crypto primitives are available to all service schemas on this instance.
-- db-standards DB-102.

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;

-- DOWN ==
-- Intentionally a no-op: pgcrypto lives in the public schema and is shared
-- across all soma services (soma-audit, soma-vault, soma-iam use it too).
-- Dropping it would break other services — this satisfies the soma-schema
-- invariant that non-reversible migrations must document why the DOWN is absent.
-- To remove pgcrypto: coordinate across ALL services, run DROP EXTENSION pgcrypto
-- manually after all dependent objects are removed from every service schema.
