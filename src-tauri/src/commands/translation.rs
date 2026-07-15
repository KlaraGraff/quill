use tauri::{AppHandle, State};

use crate::commands::ai::{book_reference_block, emit_stream_failure, ChatMessage};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

fn lang_display_name(code: &str) -> &str {
    match code {
        "en" => "English",
        "zh" => "Chinese (Simplified)",
        "ja" => "Japanese",
        "ko" => "Korean",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "ar" => "Arabic",
        "it" => "Italian",
        _ => code,
    }
}

fn configured_translation_language(
    target_language: Option<String>,
    saved_language: Option<String>,
    ui_language: Option<String>,
) -> AppResult<String> {
    [target_language, saved_language, ui_language]
        .into_iter()
        .flatten()
        .map(|lang| lang.trim().to_string())
        .find(|lang| !lang.is_empty())
        .ok_or_else(|| AppError::Other("TRANSLATION_LANGUAGE_NOT_CONFIGURED".to_string()))
}

/// Stream a translation from the AI provider. Translation results are ephemeral
/// and are discarded when the frontend popover closes.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ai_translate_passage(
    text: String,
    context: Option<String>,
    #[allow(unused_variables)] book_id: String,
    book_title: Option<String>,
    book_author: Option<String>,
    chapter: Option<String>,
    target_language: Option<String>,
    request_id: String,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    let target_lang = {
        let conn = db.reader();
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok()
        };
        configured_translation_language(
            target_language,
            get("translation_language"),
            get("language").or_else(|| Some("en".to_string())),
        )?
    };

    let target_name = lang_display_name(&target_lang);

    // Context-aware prompt: if selection is shorter than surrounding paragraph,
    // provide the paragraph as context but only translate the selection
    let has_context = context
        .as_ref()
        .is_some_and(|c| c != &text && c.len() > text.len());
    let mut system_prompt = if has_context {
        format!(
            "You are a translator embedded in an ebook reader. The user selected a portion of text they want translated into {}.\n\n\
            Full paragraph for context:\n\"{}\"\n\n\
            Translate ONLY the selected portion below — not the full paragraph. Use the surrounding context to ensure accuracy of meaning, tone, and any pronouns or references.\n\n\
            Produce only the translation. No commentary, no labels, no original text.",
            target_name,
            context.as_deref().unwrap_or("")
        )
    } else {
        format!(
            "You are a translator embedded in an ebook reader. Translate the following passage into {}.\n\n\
            Produce only the translation. No commentary, no labels, no original text. Preserve paragraph structure and tone.",
            target_name
        )
    };
    if let Some(reference) = book_reference_block(
        book_title.as_deref(),
        book_author.as_deref(),
        chapter.as_deref(),
    ) {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&reference);
    }

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
        },
        ChatMessage {
            role: "user".to_string(),
            content: text.clone(),
        },
    ];

    let event_name = format!("ai-translate-chunk-{}", request_id);
    crate::ai::router::ensure_stream_credentials_accessible(&db, &secrets)?;
    let db = db.inner().clone();
    let secrets = secrets.inner().clone();
    // Keep the cancellation token available before the detached task starts.
    crate::ai::router::register_request(&request_id);
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::ai::router::stream_with_failover(
            &app,
            &db,
            &secrets,
            &messages,
            &event_name,
            None,
            Some(&request_id),
        )
        .await
        {
            emit_stream_failure(&app, &event_name, &error);
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_translation_language_defaults_to_ui_language() {
        let lang = configured_translation_language(None, None, Some("zh".to_string())).unwrap();
        assert_eq!(lang, "zh");

        let err = configured_translation_language(None, None, None).unwrap_err();
        assert_eq!(err.to_string(), "TRANSLATION_LANGUAGE_NOT_CONFIGURED");

        let blank =
            configured_translation_language(None, Some("  ".to_string()), Some("  ".to_string()))
                .unwrap_err();
        assert_eq!(blank.to_string(), "TRANSLATION_LANGUAGE_NOT_CONFIGURED");

        let blank_saved =
            configured_translation_language(None, Some("  ".to_string()), Some("zh".to_string()))
                .unwrap();
        assert_eq!(blank_saved, "zh");
    }

    #[test]
    fn configured_translation_language_prefers_command_target_then_saved_target() {
        let lang = configured_translation_language(
            Some("zh".to_string()),
            Some("en".to_string()),
            Some("en".to_string()),
        )
        .unwrap();
        assert_eq!(lang, "zh");

        let saved =
            configured_translation_language(None, Some(" en ".to_string()), Some("zh".to_string()))
                .unwrap();
        assert_eq!(saved, "en");
    }

    #[test]
    fn translation_uses_shared_untrusted_book_reference() {
        let block =
            book_reference_block(Some("Book"), Some("Unknown Author"), Some("One")).unwrap();
        assert!(block.contains("untrusted reference data"));
        assert!(block.contains("\"title\":\"Book\""));
        assert!(!block.contains("Unknown Author"));
    }
}
