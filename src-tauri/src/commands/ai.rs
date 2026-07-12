use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiStreamChunk {
    pub delta: String,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn public_stream_error_code(error: &AppError) -> &'static str {
    const CONFIGURATION_ERRORS: [&str; 5] = [
        "AI_NOT_CONFIGURED",
        "AI_KEYS_DISABLED",
        "AI_ALL_KEYS_INVALID",
        "AI_KEYS_COOLING_DOWN",
        "AI_NO_USABLE_KEYS",
    ];
    let message = error.to_string();
    CONFIGURATION_ERRORS
        .into_iter()
        .find(|code| message.contains(code))
        .unwrap_or("AI_STREAM_FAILED")
}

pub(crate) fn emit_stream_failure(app: &AppHandle, event_name: &str, error: &AppError) {
    if error.to_string().contains("AI_REQUEST_CANCELLED") {
        return;
    }
    log::error!("AI stream failed on {event_name}: {error}");
    let _ = app.emit(
        event_name,
        AiStreamChunk {
            delta: String::new(),
            done: true,
            error: Some(public_stream_error_code(error).to_string()),
        },
    );
}

fn spawn_routed_stream(
    app: AppHandle,
    db: Db,
    secrets: Secrets,
    messages: Vec<ChatMessage>,
    event_name: String,
    max_tokens: Option<u32>,
    request_id: String,
) {
    // Register before spawning so an immediate Stop click can never race the
    // task's first poll of the cancellation registry.
    crate::ai::router::register_request(&request_id);
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::ai::router::stream_with_failover(
            &app,
            &db,
            &secrets,
            &messages,
            &event_name,
            max_tokens,
            Some(&request_id),
        )
        .await
        {
            emit_stream_failure(&app, &event_name, &error);
        }
    });
}

#[tauri::command]
pub fn ai_cancel(request_id: String) -> bool {
    crate::ai::router::cancel_request(&request_id)
}

const LOOKUP_TRANSLATION_MARKER: &str = "[[QUILL_TRANSLATION]]";

/// Sentinel `lookup_language` value: respond in whatever language the
/// selection is in, rather than a pinned target language.
const LOOKUP_LANGUAGE_SELECTION: &str = "selection";

fn language_name(code: &str) -> String {
    match code {
        "en" => "English",
        "zh" => "Chinese (Simplified)",
        "ja" => "Japanese",
        "ko" => "Korean",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        _ => code,
    }
    .to_string()
}

/// The "respond in X" clause for the main definition/context output. For the
/// `selection` sentinel this instructs the model to mirror the selection's
/// language; otherwise it resolves to a concrete language name.
fn main_language_clause(language: &str) -> String {
    if language == LOOKUP_LANGUAGE_SELECTION {
        "the same language as the selected word/phrase".to_string()
    } else {
        language_name(language)
    }
}

fn lookup_system_prompt(
    kind: &str,
    language: &str,
    lookup_translation_language: &str,
    show_translation: bool,
) -> String {
    let should_show_translation = show_translation
        && !lookup_translation_language.is_empty()
        && lookup_translation_language != language;
    let translation_prefix = if should_show_translation {
        format!(
            "Before the definition, provide a brief translation of the word/phrase in {}. The first line MUST be exactly `{}` followed immediately by the brief translation, then a newline. This marker is required machine-readable metadata, not a header. Keep the translation to a few words — no explanation, just the meaning. After that first line, proceed with the definition as usual. Do not put the marker anywhere except the first line.\n\n",
            language_name(lookup_translation_language),
            LOOKUP_TRANSLATION_MARKER,
        )
    } else {
        String::new()
    };
    let clause = main_language_clause(language);
    let definition_language_prefix = if should_show_translation {
        format!("After that first line, respond entirely in {}.\n\n", clause)
    } else {
        format!("Respond entirely in {}.\n\n", clause)
    };
    let context_language_prefix = format!("Respond entirely in {}.\n\n", clause);

    let def_prefix = format!("{translation_prefix}{definition_language_prefix}");
    let ctx_prefix = &context_language_prefix;

    match kind {
        "definition" => format!("{}You are a reading assistant embedded in an ebook reader. The user selected a word or phrase and wants a dictionary-style definition.\n\nGive: pronunciation in IPA (if English), part of speech, and a concise definition in 1–2 sentences.\n\nIf the selection is a proper noun (person, place, historical event), give a brief factual identification instead.\n\nBe concise. No headers or labels.", def_prefix),
        "context" => format!("{}You are a reading assistant embedded in an ebook reader. The user selected a word or phrase and wants to understand how it's used in the surrounding passage.\n\nExplain how the word is used in context. Consider the author's intent, tone, or any literary/idiomatic significance. Keep it to 2–3 sentences.\n\nBe concise. No headers or labels.", ctx_prefix),
        _ => format!("{}You are a reading assistant embedded in an ebook reader. The user selected a word or phrase and wants to understand it.\n\nRespond in two parts:\n\n1. **Definition** — Give a dictionary-style entry: the word, pronunciation in IPA (if it's an English word), part of speech, and a concise definition in one sentence.\n\n2. **In context** — Explain how the word is used in the given passage. Consider the author's intent, tone, or any literary/idiomatic significance. Keep it to 2–3 sentences.\n\nIf the selection is a proper noun (person, place, historical event), replace the dictionary definition with a brief factual identification, then explain its relevance in context.\n\nDo not use headers or labels like \"Definition:\" or \"In context:\". Separate the two parts with a line break. Be concise.", def_prefix),
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ai_lookup(
    word: String,
    sentence: String,
    book_title: Option<String>,
    chapter: Option<String>,
    request_id: String,
    kind: Option<String>,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    let (language, lookup_translation_language, show_translation) = {
        let conn = db.reader();
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok()
        };
        let sys_language = get("language").unwrap_or_else(|| "en".to_string());
        // lookup_language drives the main definition/context language; unset
        // falls back to the selection's own language.
        let lookup_language =
            get("lookup_language").unwrap_or_else(|| LOOKUP_LANGUAGE_SELECTION.to_string());
        let lookup_translation_language = get("lookup_translation_language")
            .map(|lang| lang.trim().to_string())
            .filter(|lang| !lang.is_empty())
            .unwrap_or_else(|| sys_language.clone());
        (
            lookup_language,
            lookup_translation_language,
            get("show_translation").unwrap_or_else(|| "false".to_string()),
        )
    };

    let mut user_content = format!(
        "Word/phrase: \"{}\"\nSurrounding text: \"{}\"",
        word, sentence
    );
    if let Some(ref title) = book_title {
        user_content.push_str(&format!("\nBook: \"{}\"", title));
    }
    if let Some(ref ch) = chapter {
        user_content.push_str(&format!("\nChapter: \"{}\"", ch));
    }

    let kind = kind.unwrap_or_else(|| "full".to_string());

    let system_prompt = lookup_system_prompt(
        kind.as_str(),
        &language,
        lookup_translation_language.trim(),
        show_translation == "true",
    );

    let max_tokens = match kind.as_str() {
        "definition" => Some(128),
        "context" => Some(192),
        _ => Some(256),
    };

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_content,
        },
    ];

    let event_name = format!("ai-lookup-chunk-{}", request_id);

    spawn_routed_stream(
        app,
        db.inner().clone(),
        secrets.inner().clone(),
        messages,
        event_name,
        max_tokens,
        request_id,
    );

    Ok(())
}

/// Build the system prompt for sentence/passage explanation.
///
/// Brevity is enforced by the prompt itself (2–3 sentences, no preamble) —
/// deliberately no `max_tokens` cap, which would truncate mid-sentence.
/// Unlike `ai_lookup`, there is no translation-gloss branch: that is a
/// word-level concept and makes no sense for a whole passage. The only
/// language handling is the response-language directive.
fn explain_system_prompt(language: &str) -> String {
    let target = if language == LOOKUP_LANGUAGE_SELECTION {
        "the same language as the selected passage".to_string()
    } else {
        language_name(language)
    };
    let language_prefix = format!("Respond entirely in {}.\n\n", target);

    format!(
        "{}You are a reading assistant embedded in an ebook reader. The user selected a sentence or passage and wants to understand it in context.\n\nIn 2–3 sentences, explain what it means and why it matters here — clarify any difficult phrasing, allusion, or tone. Be direct and concise. Do not restate the passage, add headers or labels, or pad with preamble. Plain prose only.",
        language_prefix
    )
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ai_explain(
    passage: String,
    surrounding: Option<String>,
    book_title: Option<String>,
    chapter: Option<String>,
    request_id: String,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    let language = {
        let conn = db.reader();
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok()
        };
        let lookup_language =
            get("lookup_language").unwrap_or_else(|| LOOKUP_LANGUAGE_SELECTION.to_string());
        let explain_language = get("explain_language")
            .map(|lang| lang.trim().to_string())
            .filter(|lang| !lang.is_empty());
        let language = match explain_language.as_deref() {
            Some("lookup") | None => lookup_language,
            Some(lang) => lang.to_string(),
        };
        language
    };

    let mut user_content = format!("Passage: \"{}\"", passage);
    if let Some(ref ctx) = surrounding {
        if !ctx.is_empty() && ctx != &passage {
            user_content.push_str(&format!("\nSurrounding text: \"{}\"", ctx));
        }
    }
    if let Some(ref title) = book_title {
        user_content.push_str(&format!("\nBook: \"{}\"", title));
    }
    if let Some(ref ch) = chapter {
        user_content.push_str(&format!("\nChapter: \"{}\"", ch));
    }

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: explain_system_prompt(&language),
        },
        ChatMessage {
            role: "user".to_string(),
            content: user_content,
        },
    ];

    let event_name = format!("ai-lookup-chunk-{}", request_id);

    spawn_routed_stream(
        app,
        db.inner().clone(),
        secrets.inner().clone(),
        messages,
        event_name,
        None,
        request_id,
    );

    Ok(())
}

#[tauri::command]
pub async fn ai_generate_title(
    user_message: String,
    assistant_message: String,
    request_id: String,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    let language = {
        let conn = db.reader();
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok()
        };
        get("language").unwrap_or_else(|| "en".to_string())
    };

    let user_snippet = truncate_utf8(&user_message, 200);
    let ai_snippet = truncate_utf8(&assistant_message, 200);

    let title_lang_hint = if language == "zh" {
        " Generate the title in Chinese."
    } else {
        ""
    };

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: format!("Generate a very short title (3-6 words) for the following chat exchange.{} Respond with ONLY the title, no quotes, no punctuation at the end.", title_lang_hint),
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!("User: {}\nAssistant: {}", user_snippet, ai_snippet),
        },
    ];

    let event_name = format!("ai-title-chunk-{}", request_id);
    spawn_routed_stream(
        app,
        db.inner().clone(),
        secrets.inner().clone(),
        messages,
        event_name,
        Some(32),
        request_id,
    );

    Ok(())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }

    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ai_chat(
    messages: Vec<ChatMessage>,
    book_title: Option<String>,
    book_author: Option<String>,
    current_chapter: Option<String>,
    request_id: String,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    let language = {
        let conn = db.reader();
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok()
        };
        get("language").unwrap_or_else(|| "en".to_string())
    };

    // Build messages: system prompt + conversation history (context is inlined by frontend)
    let mut system_content = "You are a helpful reading assistant. Help the user understand and discuss the book they are reading.".to_string();
    if let Some(ref title) = book_title {
        system_content.push_str(&format!("\n\nThe user is currently reading \"{}\"", title));
        if let Some(ref author) = book_author {
            system_content.push_str(&format!(" by {}", author));
        }
        system_content.push('.');
        if let Some(ref chapter) = current_chapter {
            system_content.push_str(&format!(" They are on: {}.", chapter));
        }
    }
    if language == "zh" {
        system_content.push_str(" Always respond in Chinese (Simplified).");
    }

    let mut api_messages = Vec::new();
    api_messages.push(ChatMessage {
        role: "system".to_string(),
        content: system_content,
    });
    api_messages.extend(messages);

    let event_name = format!("ai-stream-chunk-{request_id}");
    spawn_routed_stream(
        app,
        db.inner().clone(),
        secrets.inner().clone(),
        api_messages,
        event_name,
        None,
        request_id,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explain_prompt_asks_for_brevity_and_no_headers() {
        let p = explain_system_prompt("en");
        assert!(p.contains("2–3 sentences"), "must request a short answer");
        assert!(
            p.contains("headers or labels"),
            "must forbid headers/labels"
        );
        assert!(p.contains("in context"), "must be context-aware");
    }

    #[test]
    fn explain_prompt_english_emits_english_directive() {
        let p = explain_system_prompt("en");
        assert!(p.contains("Respond entirely in English."));
    }

    #[test]
    fn explain_prompt_non_english_prepends_response_language() {
        let zh = explain_system_prompt("zh");
        assert!(zh.starts_with("Respond entirely in Chinese (Simplified)."));

        let fr = explain_system_prompt("fr");
        assert!(fr.starts_with("Respond entirely in French."));
    }

    #[test]
    fn explain_selection_uses_source_language() {
        let p = explain_system_prompt("selection");
        assert!(p.contains("the same language as the selected passage"));
    }

    #[test]
    fn explain_prompt_never_has_translation_gloss() {
        // The word-level "brief translation of the word/phrase" preamble from
        // ai_lookup must not leak into the passage-level explain prompt.
        for lang in ["en", "zh", "fr"] {
            let p = explain_system_prompt(lang);
            assert!(
                !p.to_lowercase().contains("translation of the"),
                "explain must not carry ai_lookup's word-gloss logic (lang={lang})"
            );
        }
    }

    #[test]
    fn lookup_definition_prompt_marks_translation_when_target_differs() {
        let p = lookup_system_prompt("definition", "en", "zh", true);
        assert!(p.contains(LOOKUP_TRANSLATION_MARKER));
        assert!(p.contains("Chinese (Simplified)"));

        let non_english_lookup = lookup_system_prompt("definition", "zh", "en", true);
        assert!(non_english_lookup.contains(LOOKUP_TRANSLATION_MARKER));
        assert!(non_english_lookup.contains("brief translation of the word/phrase in English"));
        assert!(non_english_lookup
            .contains("After that first line, respond entirely in Chinese (Simplified)."));

        let same_language = lookup_system_prompt("definition", "en", "en", true);
        assert!(!same_language.contains(LOOKUP_TRANSLATION_MARKER));

        let disabled = lookup_system_prompt("definition", "en", "zh", false);
        assert!(!disabled.contains(LOOKUP_TRANSLATION_MARKER));
    }

    #[test]
    fn lookup_context_prompt_never_marks_english_translation() {
        let p = lookup_system_prompt("context", "en", "zh", true);
        assert!(!p.contains(LOOKUP_TRANSLATION_MARKER));
        assert!(!p.to_lowercase().contains("brief translation"));
    }

    #[test]
    fn lookup_prompt_uses_lookup_language_names() {
        let zh = lookup_system_prompt("definition", "zh", "en", true);
        assert!(zh.contains("respond entirely in Chinese (Simplified)."));
    }

    #[test]
    fn lookup_english_emits_explicit_english_directive() {
        let p = lookup_system_prompt("definition", "en", "", false);
        assert!(p.contains("Respond entirely in English."));
    }

    #[test]
    fn lookup_selection_uses_source_language() {
        let p = lookup_system_prompt("definition", "selection", "", false);
        assert!(p.contains("the same language as the selected word/phrase"));
        assert!(!p.contains("Respond entirely in selection."));
    }

    #[test]
    fn lookup_selection_allows_gloss() {
        let p = lookup_system_prompt("definition", "selection", "en", true);
        assert!(p.contains(LOOKUP_TRANSLATION_MARKER));
        assert!(p.contains(
            "After that first line, respond entirely in the same language as the selected word/phrase."
        ));
    }

    #[test]
    fn truncate_utf8_respects_multibyte_boundaries() {
        assert_eq!(truncate_utf8("short", 200), "short");
        assert_eq!(truncate_utf8(&"a".repeat(201), 200).len(), 200);

        let chinese = "中".repeat(100);
        let truncated = truncate_utf8(&chinese, 200);
        assert_eq!(truncated.len(), 198);
        assert_eq!(truncated.chars().count(), 66);

        let emoji = format!("{}🙂tail", "a".repeat(199));
        assert_eq!(truncate_utf8(&emoji, 200), "a".repeat(199));
    }

    #[test]
    fn stream_failures_preserve_public_key_pool_states() {
        for code in [
            "AI_NOT_CONFIGURED",
            "AI_KEYS_DISABLED",
            "AI_ALL_KEYS_INVALID",
            "AI_KEYS_COOLING_DOWN",
            "AI_NO_USABLE_KEYS",
        ] {
            assert_eq!(
                public_stream_error_code(&AppError::Other(code.to_string())),
                code
            );
        }
        assert_eq!(
            public_stream_error_code(&AppError::Ai("provider request failed".to_string())),
            "AI_STREAM_FAILED"
        );
    }
}
