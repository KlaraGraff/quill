-- Structured learning cards, complete AI service health, whole-book word
-- markers, user notes, and optional language-assessment inputs.

ALTER TABLE lookup_records ADD COLUMN result_json TEXT;
ALTER TABLE lookup_records ADD COLUMN provider_profile_id TEXT;
ALTER TABLE lookup_records ADD COLUMN model TEXT;
ALTER TABLE lookup_records ADD COLUMN updated_at INTEGER;
UPDATE lookup_records SET updated_at = last_looked_up_at WHERE updated_at IS NULL;

ALTER TABLE ai_profiles ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
ALTER TABLE ai_profiles ADD COLUMN cooldown_until INTEGER;
ALTER TABLE ai_profiles ADD COLUMN last_error_kind TEXT;
ALTER TABLE ai_profiles ADD COLUMN last_used_at INTEGER;
ALTER TABLE ai_profiles ADD COLUMN last_latency_ms INTEGER;

CREATE TABLE word_mark_rules (
  id TEXT PRIMARY KEY,
  book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  normalized_word TEXT NOT NULL,
  display_word TEXT NOT NULL,
  match_mode TEXT NOT NULL DEFAULT 'exact',
  color TEXT NOT NULL DEFAULT 'lookup',
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  updated_by_device TEXT NOT NULL DEFAULT 'migration'
);

CREATE UNIQUE INDEX idx_word_mark_rules_book_word
  ON word_mark_rules(book_id, normalized_word, match_mode);
CREATE INDEX idx_word_mark_rules_updated
  ON word_mark_rules(updated_at DESC);

CREATE TABLE notes (
  id TEXT PRIMARY KEY,
  book_id TEXT REFERENCES books(id) ON DELETE SET NULL,
  anchor_kind TEXT NOT NULL,
  normalized_word TEXT,
  scope TEXT NOT NULL DEFAULT 'book',
  location TEXT,
  selected_text TEXT,
  content TEXT NOT NULL DEFAULT '',
  content_format TEXT NOT NULL DEFAULT 'plain_text',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  updated_by_device TEXT NOT NULL DEFAULT 'migration'
);

CREATE INDEX idx_notes_book_updated ON notes(book_id, updated_at DESC);
CREATE INDEX idx_notes_word_updated ON notes(normalized_word, updated_at DESC);
CREATE INDEX idx_notes_updated ON notes(updated_at DESC);

-- Preserve legacy highlight notes as first-class selection notes. The source
-- column intentionally remains populated so older logs/snapshots and existing
-- highlight UI continue to round-trip without data loss.
INSERT OR IGNORE INTO notes (
  id, book_id, anchor_kind, normalized_word, scope, location, selected_text,
  content, content_format, created_at, updated_at, updated_by_device
)
SELECT
  'legacy-highlight-note-' || id,
  book_id,
  'selection',
  NULL,
  'book',
  cfi_range,
  text_content,
  note,
  'plain_text',
  created_at,
  updated_at,
  updated_by_device
FROM highlights
WHERE note IS NOT NULL AND LENGTH(TRIM(note)) > 0;

CREATE TABLE language_assessments (
  id TEXT PRIMARY KEY,
  exam_type TEXT NOT NULL,
  overall_score REAL NOT NULL,
  reading_score REAL,
  exam_date TEXT,
  mapping_version TEXT NOT NULL,
  estimated_cefr TEXT NOT NULL,
  confidence TEXT NOT NULL DEFAULT 'official',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_language_assessments_recent
  ON language_assessments(updated_at DESC);
