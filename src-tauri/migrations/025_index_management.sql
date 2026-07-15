ALTER TABLE book_summaries
  ADD COLUMN user_edited INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_book_summaries_edited
  ON book_summaries(book_id, user_edited);
