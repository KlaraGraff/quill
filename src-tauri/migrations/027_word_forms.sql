CREATE TABLE IF NOT EXISTS word_forms (
    normalized_word TEXT PRIMARY KEY,
    forms TEXT NOT NULL DEFAULT '[]',
    source TEXT NOT NULL DEFAULT 'user' CHECK(source IN ('model', 'user')),
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_word_mark_rules_global_created
ON word_mark_rules(enabled, created_at DESC);
