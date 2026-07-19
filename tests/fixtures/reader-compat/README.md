# Reader compatibility fixtures

These fixtures are synthetic, contain no user book data, and are intentionally
small enough to inspect directly.

## `minimal-deflated.epub`

The archive is built from `epub-source/`. `mimetype` is the first entry and is
stored; every XML/XHTML/SVG payload is deflated with ZIP method 8. The fixture
covers:

- `META-INF/container.xml` package discovery;
- EPUB 3 OPF metadata and spine;
- EPUB 2 NCX and EPUB 3 navigation;
- one text chapter; and
- one referenced SVG image.

Rebuild from this directory with Info-ZIP:

```sh
cd epub-source
TZ=UTC touch -t 202607190000.00 \
  mimetype META-INF/container.xml EPUB/package.opf EPUB/toc.ncx \
  EPUB/nav.xhtml EPUB/chapter.xhtml EPUB/image.svg
zip -X0 ../minimal-deflated.epub mimetype
zip -X9 ../minimal-deflated.epub \
  META-INF/container.xml EPUB/package.opf EPUB/toc.ncx \
  EPUB/nav.xhtml EPUB/chapter.xhtml EPUB/image.svg
```

Delete or move the previous archive before rebuilding. `reader-compat.test.ts`
checks entry order, compression methods, paths, and payloads.

## `minimal-text.pdf`

This is a hand-auditable PDF 1.4 file with one page, one built-in Helvetica
font, and one text operation containing `Lantern reader compatibility`.
