ALTER TABLE chat_messages ADD COLUMN updated_by_device TEXT NOT NULL DEFAULT '';

INSERT INTO settings (key, value) VALUES ('ai_spoiler_guard', 'true')
ON CONFLICT(key) DO NOTHING;
