ALTER TABLE books ADD COLUMN source_format TEXT;
ALTER TABLE books ADD COLUMN render_format TEXT;
ALTER TABLE books ADD COLUMN conversion_version INTEGER;

UPDATE books
SET source_format = COALESCE(source_format, format),
    render_format = COALESCE(render_format, format),
    conversion_version = COALESCE(conversion_version, 1);
