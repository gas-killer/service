-- Serves the startup re-queue's keyset walk over tasks a restart left in flight.
--
-- The walk pages by `(created_at, id)` and settles rows as it goes, so the set it is reading
-- changes underneath it. A keyset cursor stays correct across that mutation where an OFFSET
-- would skip rows, but it is only cheap if the index supplies the ordering: without one, every
-- page sorts the whole matching set. This index is partial so it covers only the rows the walk
-- can act on, terminal rows being the ones that dominate the table over time, and it is keyed on
-- the cursor's own columns so each page is a seek rather than a scan and sort.
--
-- The predicate names the terminal statuses rather than the two in-flight ones, which are
-- equivalent under the table's CHECK constraint. SQLite only uses a partial index when it can
-- prove the query's WHERE implies the index's own, and of the equivalent forms its prover accepts
-- only this one: `status IN ('queued','processing')` and the `OR` spelling both fall back to
-- `idx_tasks_status` plus a temporary B-tree sort. `the_paged_walk_uses_the_incomplete_index`
-- pins that, so a future status must be added to both this list and the walk's together.
CREATE INDEX idx_tasks_incomplete
    ON tasks (created_at, id)
    WHERE status NOT IN ('ready', 'failed', 'expired');
