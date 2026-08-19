-- Serves the deduplication lookup on POST /tasks.
--
-- A client that retries after a timeout submits the same logical request twice. An in-flight
-- (`queued`/`processing`) or `ready` task sharing a submission's
-- (key_id, target_address, transition_index) is that same work, so the submit path collapses onto
-- it rather than creating a duplicate that would race a doomed second transaction onto the chain
-- (only one can consume a given transition_index). A `failed` or `expired` task is not a duplicate,
-- so re-submission after one proceeds normally.
--
-- The index is partial on `transition_index IS NOT NULL`: an "auto" request (NULL index) leaves the
-- slot for the server to resolve at dequeue time, so two auto submissions are distinct requests
-- that each take their own slot (safe parallel submissions) and are never deduplicated. The lookup
-- therefore only ever probes non-NULL index rows, which this index covers.
CREATE INDEX idx_tasks_dedup
    ON tasks (key_id, target_address, transition_index)
    WHERE transition_index IS NOT NULL;
