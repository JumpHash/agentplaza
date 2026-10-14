-- The cached answer for one reference, if any (unfurl/mod.rs). $1 = key (text)
SELECT data, ok, fetched_ms FROM unfurl_cache WHERE key = $1
