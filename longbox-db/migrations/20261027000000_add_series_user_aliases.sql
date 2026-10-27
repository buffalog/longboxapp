-- Alternate titles the user set for a series (newline-separated), e.g.
-- "Teenage Mutant Ninja Turtles: The Last Ronin" for CV's "TMNT: The Last
-- Ronin". Kept apart from `aliases`, which CV enrichment overwrites.
ALTER TABLE series ADD COLUMN user_aliases TEXT;
