-- Migration 0003 — pending_sync cascade payload (T0.2.4 sync ship-gate)
--
-- migration 0002's pending_sync carried only (memory_id, operation,
-- queued_at) — not enough for the divergence detector's sweep to re-enqueue
-- the cascade when retry_queue capacity returns. Re-enqueueing a retry_queue
-- row needs the same two fields retry_queue itself stores: the audit-chain
-- `sequence_id` (the cascade-ordering anchor — plan Q1) and the operation
-- `payload` (the CascadePayloadV1 bytes — embedding + boundary). Without them
-- the sweep could only DROP the entry, a silent cross-device data-recovery
-- gap. These columns let the sweep hand the stored bytes straight to the
-- retry_queue insert, closing the V0.2 sync ship-gate.
--
-- Columns are added nullable / defaulted so the ALTER succeeds on the table
-- 0002 already created. No legacy rows are expected (cap-overflow needs a
-- 10k-deep retry_queue, never reached in V0.1 dogfood); any that somehow
-- exist read back with NULL payload and the sweep skips them rather than
-- re-enqueueing a broken cascade.

ALTER TABLE pending_sync ADD COLUMN sequence_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pending_sync ADD COLUMN payload BLOB;
