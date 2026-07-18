//! Local scanned-PDF OCR data plane.
//!
//! Phase D deliberately contains no sync events, outbox writes, package
//! downloads, or production worker startup. Those remain gated by later
//! phases so a default build cannot publish schema-v7 state accidentally.

#![allow(dead_code)]

pub(crate) mod assets;
pub(crate) mod backend;
pub(crate) mod jobs;
pub(crate) mod manager;
pub(crate) mod package;
pub(crate) mod publish;
pub(crate) mod resolver;
pub(crate) mod validate;

/// The production worker must remain disabled in default builds until the
/// Phase C release gates and the Phase G sync protocol land.
pub(crate) const PIPELINE_ENABLED: bool = cfg!(feature = "ocr-pipeline");

pub(crate) fn pipeline_enabled() -> bool {
    PIPELINE_ENABLED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(feature = "ocr-pipeline"))]
    fn default_build_is_fail_closed() {
        assert!(!pipeline_enabled());
    }

    #[test]
    #[cfg(feature = "ocr-pipeline")]
    fn opt_in_build_reports_pipeline_capability() {
        assert!(pipeline_enabled());
    }
}
