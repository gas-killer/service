-- Structured record of a completed aggregation round, stored as JSON alongside the rendered
-- transaction-request `payload`.
--
-- The `payload` column holds the ready-to-sign transaction request the user submits; `bundle`
-- holds the underlying `verifyAndUpdate` argument components (msg hash, quorum/proof material,
-- reference block, storage updates, transition index, chain id, validity bound). Keeping the
-- bundle as the durable unit lets the read path check freshness without re-deriving anything and
-- lets the retained broadcast path (future auto-execute / AA tier) submit the same round directly.
-- NULL until the task settles `ready`.
ALTER TABLE tasks ADD COLUMN bundle TEXT;
