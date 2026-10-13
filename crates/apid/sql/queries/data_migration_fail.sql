-- Marks one domain failed, with the reason. When the domain has no row yet its source is unknown,
-- so the row records source none.
-- $1 = domain (text), $2 = detail (text)
INSERT INTO data_migrations (domain, status, source, detail, finished_at)
VALUES ($1, 'failed', 'none', $2, now())
ON CONFLICT (domain) DO UPDATE SET status = 'failed', detail = EXCLUDED.detail, finished_at = now()
