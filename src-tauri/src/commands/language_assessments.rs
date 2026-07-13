use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult};

const MAPPING_VERSION: &str = "cefr-estimate-2026-07-v1";
const CEFR_LEVELS: [&str; 6] = ["A1", "A2", "B1", "B2", "C1", "C2"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CefrEstimate {
    pub estimated_cefr: String,
    pub lower_cefr: Option<String>,
    pub upper_cefr: Option<String>,
    pub confidence: String,
    pub mapping_version: String,
    pub needs_confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageAssessment {
    pub id: String,
    pub exam_type: String,
    pub overall_score: f64,
    pub reading_score: Option<f64>,
    pub exam_date: Option<String>,
    pub mapping_version: String,
    pub estimated_cefr: String,
    pub lower_cefr: Option<String>,
    pub upper_cefr: Option<String>,
    pub confidence: String,
    pub needs_confirmation: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguageAssessmentSummary {
    pub estimated_cefr: String,
    pub lower_cefr: Option<String>,
    pub upper_cefr: Option<String>,
    pub confidence: String,
    pub needs_confirmation: bool,
    pub assessment_count: usize,
    pub reading_assessment_count: usize,
    pub official_assessment_count: usize,
    pub latest_exam_date: Option<String>,
    pub primary_assessment_id: String,
}

#[derive(Debug, Clone)]
struct StoredLanguageAssessment {
    id: String,
    exam_type: String,
    overall_score: f64,
    reading_score: Option<f64>,
    exam_date: Option<String>,
    mapping_version: String,
    estimated_cefr: String,
    confidence: String,
    created_at: i64,
    updated_at: i64,
}

fn level_index(level: &str) -> usize {
    CEFR_LEVELS
        .iter()
        .position(|candidate| *candidate == level)
        .unwrap_or(0)
}

fn confidence_rank(confidence: &str) -> usize {
    match confidence {
        "official_band_approximation" => 3,
        "approximate" => 2,
        _ => 1,
    }
}

fn normalized_exam_date(exam_date: Option<String>) -> AppResult<Option<String>> {
    let Some(exam_date) = exam_date else {
        return Ok(None);
    };
    let exam_date = exam_date.trim();
    if exam_date.is_empty() {
        return Ok(None);
    }
    let parsed = chrono::NaiveDate::parse_from_str(exam_date, "%Y-%m-%d")
        .map_err(|_| AppError::Other("LANGUAGE_EXAM_DATE_INVALID".to_string()))?;
    if parsed > chrono::Local::now().date_naive() {
        return Err(AppError::Other("LANGUAGE_EXAM_DATE_IN_FUTURE".to_string()));
    }
    Ok(Some(parsed.format("%Y-%m-%d").to_string()))
}

fn estimate_from_thresholds(score: f64, thresholds: &[(f64, &str)]) -> String {
    thresholds
        .iter()
        .rev()
        .find(|(minimum, _)| score >= *minimum)
        .map(|(_, level)| (*level).to_string())
        .unwrap_or_else(|| "A1".to_string())
}

fn score_range(exam_type: &str, reading: bool) -> Option<(f64, f64)> {
    match (exam_type, reading) {
        ("ielts", _) => Some((0.0, 9.0)),
        ("toefl_ibt", false) => Some((0.0, 120.0)),
        ("toefl_ibt", true) => Some((0.0, 30.0)),
        ("toeic_lr", false) => Some((10.0, 990.0)),
        ("toeic_lr", true) => Some((5.0, 495.0)),
        ("cambridge", _) => Some((80.0, 230.0)),
        ("det", _) => Some((10.0, 160.0)),
        ("cet4" | "cet6", false) => Some((0.0, 710.0)),
        ("cet4" | "cet6", true) => Some((0.0, 249.0)),
        _ => None,
    }
}

fn estimate_one(exam_type: &str, score: f64, reading: bool) -> AppResult<(String, String)> {
    let (minimum, maximum) = score_range(exam_type, reading)
        .ok_or_else(|| AppError::Other("LANGUAGE_EXAM_UNSUPPORTED".to_string()))?;
    if !score.is_finite() || score < minimum || score > maximum {
        return Err(AppError::Other("LANGUAGE_SCORE_INVALID".to_string()));
    }
    let thresholds: &[(f64, &str)] = match (exam_type, reading) {
        ("ielts", _) => &[
            (0.0, "A1"),
            (3.0, "A2"),
            (4.0, "B1"),
            (5.5, "B2"),
            (7.0, "C1"),
            (8.5, "C2"),
        ],
        ("toefl_ibt", false) => &[
            (0.0, "A1"),
            (32.0, "A2"),
            (42.0, "B1"),
            (72.0, "B2"),
            (95.0, "C1"),
        ],
        ("toefl_ibt", true) => &[
            (0.0, "A1"),
            (4.0, "A2"),
            (9.0, "B1"),
            (17.0, "B2"),
            (24.0, "C1"),
        ],
        ("toeic_lr", false) => &[
            (10.0, "A1"),
            (225.0, "A2"),
            (550.0, "B1"),
            (785.0, "B2"),
            (945.0, "C1"),
        ],
        ("toeic_lr", true) => &[
            (5.0, "A1"),
            (115.0, "A2"),
            (275.0, "B1"),
            (395.0, "B2"),
            (455.0, "C1"),
        ],
        ("cambridge", _) => &[
            (80.0, "A1"),
            (120.0, "A2"),
            (140.0, "B1"),
            (160.0, "B2"),
            (180.0, "C1"),
            (200.0, "C2"),
        ],
        ("det", _) => &[
            (10.0, "A1"),
            (55.0, "A2"),
            (75.0, "B1"),
            (100.0, "B2"),
            (130.0, "C1"),
            (155.0, "C2"),
        ],
        ("cet4", false) => &[
            (0.0, "A1"),
            (350.0, "A2"),
            (425.0, "B1"),
            (550.0, "B2"),
            (650.0, "C1"),
        ],
        ("cet6", false) => &[
            (0.0, "A1"),
            (350.0, "A2"),
            (425.0, "B1"),
            (520.0, "B2"),
            (620.0, "C1"),
        ],
        ("cet4" | "cet6", true) => &[
            (0.0, "A1"),
            (100.0, "A2"),
            (150.0, "B1"),
            (195.0, "B2"),
            (230.0, "C1"),
        ],
        _ => return Err(AppError::Other("LANGUAGE_EXAM_UNSUPPORTED".to_string())),
    };
    let confidence = if matches!(exam_type, "cet4" | "cet6") {
        "low"
    } else if matches!(exam_type, "ielts" | "toefl_ibt" | "toeic_lr" | "cambridge") {
        "official_band_approximation"
    } else {
        "approximate"
    };
    Ok((
        estimate_from_thresholds(score, thresholds),
        confidence.to_string(),
    ))
}

#[tauri::command]
pub fn estimate_cefr(
    exam_type: String,
    overall_score: f64,
    reading_score: Option<f64>,
) -> AppResult<CefrEstimate> {
    let (overall, overall_confidence) = estimate_one(&exam_type, overall_score, false)?;
    let reading = reading_score
        .map(|score| estimate_one(&exam_type, score, true))
        .transpose()?;
    let estimated_cefr = reading
        .as_ref()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| overall.clone());
    let confidence = reading
        .as_ref()
        .map(|value| value.1.clone())
        .unwrap_or(overall_confidence);
    let (lower, upper, needs_confirmation) = match reading.as_ref() {
        Some((reading, _)) if level_index(&overall).abs_diff(level_index(reading)) >= 2 => {
            let mut values = [overall.clone(), reading.clone()];
            values.sort_by_key(|level| level_index(level));
            (Some(values[0].clone()), Some(values[1].clone()), true)
        }
        _ => (None, None, false),
    };
    Ok(CefrEstimate {
        estimated_cefr,
        lower_cefr: lower,
        upper_cefr: upper,
        confidence,
        mapping_version: MAPPING_VERSION.to_string(),
        needs_confirmation,
    })
}

fn row_to_assessment(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredLanguageAssessment> {
    Ok(StoredLanguageAssessment {
        id: row.get(0)?,
        exam_type: row.get(1)?,
        overall_score: row.get(2)?,
        reading_score: row.get(3)?,
        exam_date: row.get(4)?,
        mapping_version: row.get(5)?,
        estimated_cefr: row.get(6)?,
        confidence: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn enrich_assessment(stored: StoredLanguageAssessment) -> AppResult<LanguageAssessment> {
    let estimate = if stored.mapping_version == MAPPING_VERSION {
        Some(estimate_cefr(
            stored.exam_type.clone(),
            stored.overall_score,
            stored.reading_score,
        )?)
    } else {
        None
    };
    Ok(LanguageAssessment {
        id: stored.id,
        exam_type: stored.exam_type,
        overall_score: stored.overall_score,
        reading_score: stored.reading_score,
        exam_date: stored.exam_date,
        mapping_version: stored.mapping_version,
        estimated_cefr: stored.estimated_cefr,
        lower_cefr: estimate
            .as_ref()
            .and_then(|estimate| estimate.lower_cefr.clone()),
        upper_cefr: estimate
            .as_ref()
            .and_then(|estimate| estimate.upper_cefr.clone()),
        confidence: stored.confidence,
        needs_confirmation: estimate.is_some_and(|estimate| estimate.needs_confirmation),
        created_at: stored.created_at,
        updated_at: stored.updated_at,
    })
}

fn load_language_assessments(db: &Db) -> AppResult<Vec<LanguageAssessment>> {
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT id, exam_type, overall_score, reading_score, exam_date, mapping_version,
                estimated_cefr, confidence, created_at, updated_at
         FROM language_assessments ORDER BY updated_at DESC, id ASC",
    )?;
    let stored = statement
        .query_map([], row_to_assessment)?
        .collect::<Result<Vec<_>, _>>()?;
    stored.into_iter().map(enrich_assessment).collect()
}

fn summarize_assessments(assessments: &[LanguageAssessment]) -> Option<LanguageAssessmentSummary> {
    if assessments.is_empty() {
        return None;
    }

    // Prefer evidence that directly measures reading, then the confidence of
    // the published mapping, then the newest dated result. The remaining
    // records still determine whether the evidence is too contradictory to
    // apply silently.
    let primary = assessments.iter().max_by(|left, right| {
        left.reading_score
            .is_some()
            .cmp(&right.reading_score.is_some())
            .then_with(|| {
                confidence_rank(&left.confidence).cmp(&confidence_rank(&right.confidence))
            })
            .then_with(|| left.exam_date.cmp(&right.exam_date))
            .then_with(|| left.updated_at.cmp(&right.updated_at))
            .then_with(|| right.id.cmp(&left.id))
    })?;

    let lowest = assessments
        .iter()
        .map(|assessment| {
            assessment
                .lower_cefr
                .as_ref()
                .unwrap_or(&assessment.estimated_cefr)
        })
        .min_by_key(|level| level_index(level))?
        .to_string();
    let highest = assessments
        .iter()
        .map(|assessment| {
            assessment
                .upper_cefr
                .as_ref()
                .unwrap_or(&assessment.estimated_cefr)
        })
        .max_by_key(|level| level_index(level))?
        .to_string();
    let needs_confirmation = level_index(&lowest).abs_diff(level_index(&highest)) >= 2;

    let latest_date = assessments
        .iter()
        .filter_map(|assessment| assessment.exam_date.as_deref())
        .filter_map(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .max();
    let (weighted_level_sum, total_weight) =
        assessments
            .iter()
            .fold((0usize, 0usize), |(level_sum, weight_sum), assessment| {
                let date_weight = assessment
                    .exam_date
                    .as_deref()
                    .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                    .zip(latest_date)
                    .map(|(date, latest)| match (latest - date).num_days() {
                        days if days <= 730 => 3,
                        days if days <= 1_825 => 2,
                        _ => 1,
                    })
                    .unwrap_or(1);
                let reading_weight = if assessment.reading_score.is_some() {
                    4
                } else {
                    1
                };
                let weight = reading_weight * confidence_rank(&assessment.confidence) * date_weight;
                (
                    level_sum + level_index(&assessment.estimated_cefr) * weight,
                    weight_sum + weight,
                )
            });
    let combined_level_index = ((weighted_level_sum as f64 / total_weight as f64).round() as usize)
        .min(CEFR_LEVELS.len() - 1);

    Some(LanguageAssessmentSummary {
        estimated_cefr: CEFR_LEVELS[combined_level_index].to_string(),
        lower_cefr: needs_confirmation.then_some(lowest),
        upper_cefr: needs_confirmation.then_some(highest),
        confidence: primary.confidence.clone(),
        needs_confirmation,
        assessment_count: assessments.len(),
        reading_assessment_count: assessments
            .iter()
            .filter(|assessment| assessment.reading_score.is_some())
            .count(),
        official_assessment_count: assessments
            .iter()
            .filter(|assessment| assessment.confidence == "official_band_approximation")
            .count(),
        latest_exam_date: latest_date.map(|date| date.format("%Y-%m-%d").to_string()),
        primary_assessment_id: primary.id.clone(),
    })
}

#[tauri::command]
pub fn save_language_assessment(
    exam_type: String,
    overall_score: f64,
    reading_score: Option<f64>,
    exam_date: Option<String>,
    db: State<'_, Db>,
) -> AppResult<LanguageAssessment> {
    let exam_date = normalized_exam_date(exam_date)?;
    let estimate = estimate_cefr(exam_type.clone(), overall_score, reading_score)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let conn = db.reader();
    conn.execute(
        "INSERT INTO language_assessments
         (id, exam_type, overall_score, reading_score, exam_date, mapping_version,
          estimated_cefr, confidence, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            id,
            exam_type,
            overall_score,
            reading_score,
            exam_date,
            estimate.mapping_version,
            estimate.estimated_cefr,
            estimate.confidence,
            now
        ],
    )?;
    let stored = conn.query_row(
        "SELECT id, exam_type, overall_score, reading_score, exam_date, mapping_version,
                estimated_cefr, confidence, created_at, updated_at
         FROM language_assessments WHERE id = ?1",
        params![id],
        row_to_assessment,
    )?;
    enrich_assessment(stored)
}

#[tauri::command]
pub fn list_language_assessments(db: State<'_, Db>) -> AppResult<Vec<LanguageAssessment>> {
    load_language_assessments(&db)
}

#[tauri::command]
pub fn summarize_language_assessments(
    db: State<'_, Db>,
) -> AppResult<Option<LanguageAssessmentSummary>> {
    let assessments = load_language_assessments(&db)?;
    Ok(summarize_assessments(&assessments))
}

#[tauri::command]
pub fn delete_language_assessment(id: String, db: State<'_, Db>) -> AppResult<()> {
    let deleted = db.reader().execute(
        "DELETE FROM language_assessments WHERE id = ?1",
        params![id],
    )?;
    if deleted == 0 {
        return Err(AppError::Other("LANGUAGE_ASSESSMENT_NOT_FOUND".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_disagreement_returns_a_range() {
        let estimate = estimate_cefr("toefl_ibt".to_string(), 100.0, Some(10.0)).unwrap();
        assert!(estimate.needs_confirmation);
        assert_eq!(estimate.estimated_cefr, "B1");
        assert_eq!(estimate.lower_cefr.as_deref(), Some("B1"));
        assert_eq!(estimate.upper_cefr.as_deref(), Some("C1"));
    }

    #[test]
    fn reading_score_is_primary_when_present() {
        let estimate = estimate_cefr("ielts".to_string(), 6.5, Some(7.0)).unwrap();
        assert_eq!(estimate.estimated_cefr, "C1");
        assert!(!estimate.needs_confirmation);
    }

    #[test]
    fn reading_confidence_is_primary_when_present() {
        let estimate = estimate_cefr("det".to_string(), 105.0, Some(105.0)).unwrap();
        assert_eq!(estimate.estimated_cefr, "B2");
        assert_eq!(estimate.confidence, "approximate");
    }

    #[test]
    fn cet_is_always_low_confidence() {
        let estimate = estimate_cefr("cet6".to_string(), 520.0, None).unwrap();
        assert_eq!(estimate.estimated_cefr, "B2");
        assert_eq!(estimate.confidence, "low");
    }

    fn assessment(
        id: &str,
        level: &str,
        reading: bool,
        confidence: &str,
        date: Option<&str>,
    ) -> LanguageAssessment {
        LanguageAssessment {
            id: id.to_string(),
            exam_type: "ielts".to_string(),
            overall_score: 6.0,
            reading_score: reading.then_some(6.0),
            exam_date: date.map(str::to_string),
            mapping_version: MAPPING_VERSION.to_string(),
            estimated_cefr: level.to_string(),
            lower_cefr: None,
            upper_cefr: None,
            confidence: confidence.to_string(),
            needs_confirmation: false,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn summary_prefers_reading_then_confidence_then_date() {
        let assessments = vec![
            assessment(
                "overall",
                "B1",
                false,
                "official_band_approximation",
                Some("2026-06-01"),
            ),
            assessment(
                "reading-old",
                "B2",
                true,
                "official_band_approximation",
                Some("2025-01-01"),
            ),
            assessment(
                "reading-new",
                "B2",
                true,
                "official_band_approximation",
                Some("2026-01-01"),
            ),
        ];
        let summary = summarize_assessments(&assessments).unwrap();
        assert_eq!(summary.estimated_cefr, "B2");
        assert_eq!(summary.primary_assessment_id, "reading-new");
        assert!(!summary.needs_confirmation);
    }

    #[test]
    fn primary_evidence_prefers_mapping_confidence_before_date() {
        let assessments = vec![
            assessment(
                "official-old",
                "B2",
                true,
                "official_band_approximation",
                Some("2025-01-01"),
            ),
            assessment(
                "approximate-new",
                "B2",
                true,
                "approximate",
                Some("2026-01-01"),
            ),
        ];
        let summary = summarize_assessments(&assessments).unwrap();
        assert_eq!(summary.primary_assessment_id, "official-old");
        assert_eq!(summary.confidence, "official_band_approximation");
    }

    #[test]
    fn summary_combines_weighted_evidence_instead_of_only_copying_primary() {
        let assessments = vec![
            assessment(
                "reading",
                "B1",
                true,
                "official_band_approximation",
                Some("2026-01-01"),
            ),
            assessment(
                "overall-1",
                "B2",
                false,
                "official_band_approximation",
                Some("2026-01-01"),
            ),
            assessment(
                "overall-2",
                "B2",
                false,
                "official_band_approximation",
                Some("2026-01-01"),
            ),
        ];
        let summary = summarize_assessments(&assessments).unwrap();
        assert_eq!(summary.primary_assessment_id, "reading");
        assert_eq!(summary.estimated_cefr, "B1");
        assert!(!summary.needs_confirmation);
    }

    #[test]
    fn summary_requires_confirmation_for_two_level_conflict() {
        let assessments = vec![
            assessment("a", "B1", true, "official_band_approximation", None),
            assessment("b", "C1", false, "official_band_approximation", None),
        ];
        let summary = summarize_assessments(&assessments).unwrap();
        assert!(summary.needs_confirmation);
        assert_eq!(summary.lower_cefr.as_deref(), Some("B1"));
        assert_eq!(summary.upper_cefr.as_deref(), Some("C1"));
        assert_eq!(summary.estimated_cefr, "B1");
    }

    #[test]
    fn validates_assessment_dates() {
        assert_eq!(
            normalized_exam_date(Some("2025-03-09".to_string())).unwrap(),
            Some("2025-03-09".to_string())
        );
        assert!(normalized_exam_date(Some("March 9".to_string())).is_err());
        assert!(normalized_exam_date(Some("2999-01-01".to_string())).is_err());
    }
}
