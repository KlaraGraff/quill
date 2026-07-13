-- Text sources use a locally derived reading document. The source file and
-- its sync metadata remain authoritative; these fields only describe whether
-- this device has prepared its local rendering cache.
ALTER TABLE books ADD COLUMN preparation_state TEXT NOT NULL DEFAULT 'ready';
ALTER TABLE books ADD COLUMN preparation_error TEXT;

-- Versions before the native text reader stored TXT/Markdown/HTML as a
-- generated EPUB. Preserve their source files and re-prepare them locally.
UPDATE books
SET file_path = source_file_path,
    render_format = 'text',
    preparation_state = 'pending',
    preparation_error = NULL
WHERE source_format IN ('txt', 'markdown', 'html')
  AND source_file_path IS NOT NULL;
