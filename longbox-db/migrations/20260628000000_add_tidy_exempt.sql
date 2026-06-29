-- Library Tidy "Keep" needs a durable exemption flag. Without it, keeping a
-- phantom only reset its counters, and the next scan ticked them straight back
-- up — the series re-marked and re-surfaced. `tidy_exempt = 1` makes Keep stick:
-- the tick skips the series, auto-tidy never marks it, and it drops out of the
-- phantom list entirely (it shows in the "Kept series" section instead).
ALTER TABLE series ADD COLUMN tidy_exempt INTEGER NOT NULL DEFAULT 0;
