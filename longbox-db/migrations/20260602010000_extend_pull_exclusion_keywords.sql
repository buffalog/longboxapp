-- Extend pull_exclusion_keywords with collected-edition formats.
--
-- The 20260602 migration seeded the row with `Infinity Comic,Infinite
-- Comic`. This migration appends `Trade Paperback,Hardcover,Omnibus,
-- Compendium` so NZBs for collected editions of subscribed series
-- (TPBs, HCs, omnibuses) are silently dropped before the pull engine
-- considers them. Complement to the 5f4d8bc cv-enrichment penalty —
-- enrichment keeps the catalog row pointed at the original series;
-- this keeps the pull engine from grabbing the collected reissue
-- under that subscription.
--
-- Idempotency: the `NOT LIKE '%Trade Paperback%'` guard makes this
-- safe to re-run on a DB that already carries the extended value
-- (hand-edited, or a future re-application). On a fresh deploy where
-- the prior migration just seeded the row, the UPDATE matches and
-- appends. On a deploy where the user already extended the value
-- manually, the UPDATE no-ops.
UPDATE settings
   SET value = value || ',Trade Paperback,Hardcover,Omnibus,Compendium'
 WHERE key = 'pull_exclusion_keywords'
   AND value NOT LIKE '%Trade Paperback%';
