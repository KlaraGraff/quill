use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

use super::backend::OcrOutput;

#[derive(Debug, Clone)]
pub(crate) struct VerifiedOutput {
    pub path: PathBuf,
    pub content_sha256: String,
    pub byte_size: i64,
    pub page_count: i32,
    pub recognized_pages: i32,
    pub skipped_pages: i32,
    pub timed_out_pages: i32,
    pub failed_pages: i32,
}

fn validation_error(code: &str) -> AppError {
    AppError::Other(code.to_string())
}

pub(crate) fn reject_signed_pdf(path: &Path) -> AppResult<()> {
    let pdfium = crate::pdfium::pdfium().map_err(|_| validation_error("OCR_PDF_OPEN_FAILED"))?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|_| validation_error("OCR_PDF_OPEN_FAILED"))?;
    if !document.signatures().is_empty() {
        return Err(validation_error("OCR_PDF_SIGNED_UNSUPPORTED"));
    }
    Ok(())
}

pub(crate) fn validate_output(source: &Path, output: OcrOutput) -> AppResult<VerifiedOutput> {
    if output.page_count < 1
        || output.recognized_pages < 0
        || output.skipped_pages < 0
        || output.timed_out_pages < 0
        || output.failed_pages < 0
    {
        return Err(validation_error("OCR_OUTPUT_STATS_INVALID"));
    }

    let pdfium = crate::pdfium::pdfium().map_err(|_| validation_error("OCR_PDF_OPEN_FAILED"))?;
    let source_doc = pdfium
        .load_pdf_from_file(source, None)
        .map_err(|_| validation_error("OCR_SOURCE_PDF_INVALID"))?;
    let output_doc = pdfium
        .load_pdf_from_file(&output.output_path, None)
        .map_err(|_| validation_error("OCR_OUTPUT_PDF_INVALID"))?;
    if !source_doc.signatures().is_empty() {
        return Err(validation_error("OCR_PDF_SIGNED_UNSUPPORTED"));
    }

    let source_pages = source_doc.pages().len();
    let output_pages = output_doc.pages().len();
    if source_pages != output_pages || output_pages != output.page_count {
        return Err(validation_error("OCR_OUTPUT_PAGE_COUNT_MISMATCH"));
    }

    let sample_indexes = sampled_page_indexes(source_pages);
    let mut sampled_text = 0usize;
    for index in sample_indexes {
        let source_page = source_doc
            .pages()
            .get(index)
            .map_err(|_| validation_error("OCR_SOURCE_PDF_INVALID"))?;
        let output_page = output_doc
            .pages()
            .get(index)
            .map_err(|_| validation_error("OCR_OUTPUT_PDF_INVALID"))?;
        if (source_page.width().value - output_page.width().value).abs() > 0.5
            || (source_page.height().value - output_page.height().value).abs() > 0.5
            || source_page.rotation().ok() != output_page.rotation().ok()
        {
            return Err(validation_error("OCR_OUTPUT_GEOMETRY_MISMATCH"));
        }
        source_page
            .render_with_config(&PdfRenderConfig::new().set_target_width(256))
            .map_err(|_| validation_error("OCR_SOURCE_RENDER_FAILED"))?;
        output_page
            .render_with_config(&PdfRenderConfig::new().set_target_width(256))
            .map_err(|_| validation_error("OCR_OUTPUT_RENDER_FAILED"))?;
        sampled_text += output_page
            .text()
            .map_err(|_| validation_error("OCR_OUTPUT_TEXT_INVALID"))?
            .all()
            .trim()
            .chars()
            .count();
    }
    if output.recognized_pages > 0 && sampled_text == 0 {
        return Err(validation_error("OCR_OUTPUT_TEXT_LAYER_MISSING"));
    }

    let metadata = output
        .output_path
        .metadata()
        .map_err(|_| validation_error("OCR_OUTPUT_MISSING"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > i64::MAX as u64 {
        return Err(validation_error("OCR_OUTPUT_MISSING"));
    }
    Ok(VerifiedOutput {
        content_sha256: file_sha256(&output.output_path)?,
        byte_size: metadata.len() as i64,
        path: output.output_path,
        page_count: output.page_count,
        recognized_pages: output.recognized_pages,
        skipped_pages: output.skipped_pages,
        timed_out_pages: output.timed_out_pages,
        failed_pages: output.failed_pages,
    })
}

fn sampled_page_indexes(page_count: i32) -> Vec<i32> {
    let mut indexes = vec![0, page_count / 2, page_count - 1];
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

fn file_sha256(path: &Path) -> AppResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_covers_first_middle_and_last_without_duplicates() {
        assert_eq!(sampled_page_indexes(1), vec![0]);
        assert_eq!(sampled_page_indexes(2), vec![0, 1]);
        assert_eq!(sampled_page_indexes(9), vec![0, 4, 8]);
    }
}
