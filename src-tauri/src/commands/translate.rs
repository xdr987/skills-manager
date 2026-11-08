use std::sync::Arc;
use std::time::Duration;
use tauri::State;

use crate::commands::agent_workspace::{
    collect_installed_local_skills, find_center_match,
};
use crate::core::{
    error::AppError, llm_api, skill_metadata, skill_store::SkillStore, sync_metadata,
    translate_api,
};

/// LLM endpoints tolerate higher rates; stay polite without slowing small
/// batches down.
const SUMMARY_PACING: Duration = Duration::from_millis(400);
/// The instruction behind every default note: a few-word Chinese summary.
const SUMMARY_SYSTEM_PROMPT: &str = "你是中文编辑。把用户给出的 AI 编程技能简介概括成不超过 12 个字的中文短语，直接输出概括内容：不要句号或结尾标点、不要引号、不要任何解释或前缀。";

/// Fetch the Baidu credentials from settings, or fail with the sentinel the
/// frontend maps to a "not configured" hint.
fn baidu_credentials(store: &SkillStore) -> Result<(String, String), AppError> {
    let app_id = store
        .get_setting("baidu_app_id")
        .map_err(AppError::db)?
        .unwrap_or_default();
    let api_key = store
        .get_setting("baidu_api_key")
        .map_err(AppError::db)?
        .unwrap_or_default();
    if app_id.trim().is_empty() || api_key.trim().is_empty() {
        return Err(AppError::invalid_input(
            "Baidu Translate API is not configured. Set the APP ID and key in Settings.",
        ));
    }
    Ok((app_id.trim().to_string(), api_key.trim().to_string()))
}

/// (base_url, api_key, model) for the summarization LLM, or the invalid-input
/// sentinel the frontend maps to a "configure the LLM" hint.
fn llm_credentials(store: &SkillStore) -> Result<(String, String, String), AppError> {
    let base_url = store
        .get_setting("llm_base_url")
        .map_err(AppError::db)?
        .unwrap_or_default();
    let api_key = store
        .get_setting("llm_api_key")
        .map_err(AppError::db)?
        .unwrap_or_default();
    let model = store
        .get_setting("llm_model")
        .map_err(AppError::db)?
        .unwrap_or_default();
    if base_url.trim().is_empty() || api_key.trim().is_empty() || model.trim().is_empty() {
        return Err(AppError::invalid_input(
            "LLM API is not configured. Set the base URL, API key and model in Settings.",
        ));
    }
    Ok((
        base_url.trim().to_string(),
        api_key.trim().to_string(),
        model.trim().to_string(),
    ))
}

/// The description a card would show: SKILL.md frontmatter wins, DB fallback.
fn display_description(central_path: &str, fallback: Option<&str>) -> Option<String> {
    skill_metadata::parse_skill_md(std::path::Path::new(central_path))
        .description
        .or_else(|| fallback.map(|s| s.to_string()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Translate a skill's English description / document into Chinese through the
/// Baidu Translate API configured in Settings. The free tier is paced at one
/// request per second inside [`translate_api`], so this runs on the blocking
/// pool like every other network command.
#[tauri::command]
pub async fn translate_text(
    text: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<String, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (app_id, api_key) = baidu_credentials(&store)?;
        let proxy_url = store.proxy_url();
        translate_api::translate_text(
            &text,
            "auto",
            "zh",
            &app_id,
            &api_key,
            proxy_url.as_deref(),
        )
        .map_err(AppError::network)
    })
    .await?
}

/// Default notes for every skill that has none, across the whole app:
/// the LLM compresses each skill's description into a short Chinese phrase
/// the user can edit afterwards. Covers the central library AND every
/// enabled+installed agent's local skills; returns how many got a note.
#[tauri::command]
pub async fn generate_missing_notes(
    store: State<'_, Arc<SkillStore>>,
) -> Result<usize, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (base_url, api_key, model) = llm_credentials(&store)?;
        let proxy_url = store.proxy_url();
        let summarize = |description: &str| -> Result<String, AppError> {
            llm_api::chat_completion(
                &base_url,
                &api_key,
                &model,
                SUMMARY_SYSTEM_PROMPT,
                description,
                60,
                proxy_url.as_deref(),
            )
            .map_err(AppError::network)
        };

        let mut generated = 0usize;
        let mut first_request = true;
        let mut pace = |first: &mut bool| {
            if !*first {
                std::thread::sleep(SUMMARY_PACING);
            }
            *first = false;
        };

        // ── Library skills ──
        let existing = store.get_notes_map().map_err(AppError::db)?;
        for skill in store.get_all_skills().map_err(AppError::db)? {
            if existing.contains_key(&skill.id) {
                continue;
            }
            let Some(description) = display_description(&skill.central_path, skill.description.as_deref())
            else {
                continue;
            };
            pace(&mut first_request);
            let summary = summarize(&description)?;
            sync_metadata::with_repo_lock("set skill notes", || {
                store.set_notes_for_skill(&skill.id, Some(&summary))?;
                sync_metadata::ensure_skill_metadata_unlocked(&store, &skill.id)
            })
            .map_err(AppError::db)?;
            generated += 1;
        }

        // ── Agent-local skills (center-matched ones were just covered) ──
        let all_managed = store.get_all_skills().map_err(AppError::db)?;
        let all_targets = store.get_all_targets().map_err(AppError::db)?;
        let central_notes = store.get_notes_map().map_err(AppError::db)?;
        for (adapter, skills) in collect_installed_local_skills(&store) {
            let local_notes = store.get_local_notes_map(&adapter.key).map_err(AppError::db)?;
            for skill in skills {
                if local_notes.contains_key(&skill.relative_path) {
                    continue;
                }
                // A synced copy shows the central note — the library pass
                // above just generated it if it was missing.
                if find_center_match(&skill, &all_managed, &all_targets).is_some() {
                    continue;
                }
                let Some(description) = skill.description.clone().filter(|s| !s.trim().is_empty())
                else {
                    continue;
                };
                pace(&mut first_request);
                let summary = summarize(&description)?;
                store
                    .set_local_skill_notes(&adapter.key, &skill.relative_path, Some(&summary))
                    .map_err(AppError::db)?;
                generated += 1;
            }
        }

        Ok(generated)
    })
    .await?
}
