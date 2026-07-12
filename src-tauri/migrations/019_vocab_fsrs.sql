ALTER TABLE vocab_words ADD COLUMN fsrs_stability REAL;
ALTER TABLE vocab_words ADD COLUMN fsrs_difficulty REAL;
ALTER TABLE vocab_words ADD COLUMN fsrs_version INTEGER NOT NULL DEFAULT 1;

