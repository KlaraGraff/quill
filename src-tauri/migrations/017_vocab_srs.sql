-- Explicit spaced-repetition state. Existing vocabulary rows preserve their
-- current reminder; review metadata starts empty until the user rates a card.
ALTER TABLE vocab_words ADD COLUMN review_interval_days INTEGER NOT NULL DEFAULT 0;
ALTER TABLE vocab_words ADD COLUMN last_reviewed_at INTEGER;
ALTER TABLE vocab_words ADD COLUMN last_review_rating TEXT;

CREATE INDEX IF NOT EXISTS idx_vocab_due_review
  ON vocab_words(next_review_at, mastery);
