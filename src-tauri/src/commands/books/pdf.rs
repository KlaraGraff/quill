use super::*;

pub(super) struct PdfExtracted {
    pub(super) title: String,
    pub(super) author: String,
    pub(super) description: Option<String>,
    pub(super) pages: i32,
    pub(super) cover: Option<Vec<u8>>,
}

/// Single pdfium pass: load the doc once (streaming via `Read + Seek`
/// internally — bounded memory regardless of PDF size), pull
/// title/author/page-count from its metadata, and render page 1 to JPEG.
/// Avoids the multi-second `lopdf::Document::load_mem` parse on large
/// PDFs (lopdf walks the full document up front; pdfium is lazy).
///
/// Best-effort throughout: any failure falls back to the filename-derived
/// title + "Unknown Author" + 0 pages + no cover, so the import still
/// succeeds. The cover sub-step has its own fallback inside.
pub(super) fn extract_pdf(path: &Path, fallback_title: &str) -> PdfExtracted {
    let fallback = || PdfExtracted {
        title: fallback_title.to_string(),
        author: "Unknown Author".into(),
        description: None,
        pages: 0,
        cover: None,
    };

    let Ok(pdfium) =
        pdfium::pdfium().inspect_err(|e| log::warn!("extract_pdf: pdfium unavailable: {e}"))
    else {
        return fallback();
    };
    let Ok(doc) = pdfium
        .load_pdf_from_file(path, None)
        .inspect_err(|e| log::warn!("extract_pdf: load_pdf_from_file({}): {e}", path.display()))
    else {
        return fallback();
    };

    // Read from the PDF Info dictionary via pdfium (`FPDF_GetMetaText`).
    // The old frontend pdf.js path also tried the XMP `/Metadata` stream
    // (dc:title / dc:creator / dc:description) before falling back to
    // Info, but in practice almost every PDF that has XMP also fills in
    // Info with matching values — the XMP-only population is dominated
    // by PDF/A archival and PDF/X print-prep files, which aren't typical
    // reading material. Falling back to filename for that tail keeps
    // this code path simple; revisit if a real-world bug surfaces.
    let metadata = doc.metadata();
    let info = |tag: PdfDocumentMetadataTagType| {
        metadata
            .get(tag)
            .map(|t| t.value().to_string())
            .filter(|s| !s.trim().is_empty())
    };

    let title =
        info(PdfDocumentMetadataTagType::Title).unwrap_or_else(|| fallback_title.to_string());
    let author =
        info(PdfDocumentMetadataTagType::Author).unwrap_or_else(|| "Unknown Author".into());
    let description = info(PdfDocumentMetadataTagType::Subject);
    let pages = doc.pages().len();
    let cover = render_first_page(&doc);

    PdfExtracted {
        title,
        author,
        description,
        pages,
        cover,
    }
}

/// Render page 1 of an already-loaded PDF to JPEG bytes.
/// Returns `None` on any rendering or encoding failure.
fn render_first_page(doc: &PdfDocument) -> Option<Vec<u8>> {
    let page = doc.pages().first().ok()?;
    let bitmap = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(600)
                .render_form_data(false)
                .render_annotations(false),
        )
        .ok()?;
    let mut buf = Vec::new();
    bitmap
        .as_image()
        .ok()?
        .into_rgb8()
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .ok()?;
    Some(buf)
}
