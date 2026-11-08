use std::sync::Arc;
use std::time::Duration;
use tauri::State;

use crate::core::{
    error::AppError, skill_metadata, skill_store::SkillStore, sync_metadata, translate_api,
};

/// Baidu's free tier allows 1 QPS; pace batch generation the same way the
/// translate API client paces multi-chunk documents.
const BATCH_PACING: Duration = Duration::from_millis(1100);

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

/// Default notes for every library skill that has none: the Chinese
/// translation of its description, saved as an editable note. Paced at 1 QPS
/// for the free tier; returns how many skills got a note.
#[tauri::command]
pub async fn generate_missing_notes(
    store: State<'_, Arc<SkillStore>>,
) -> Result<usize, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (app_id, api_key) = baidu_credentials(&store)?;
        let proxy_url = store.proxy_url();
        let existing = store.get_notes_map().map_err(AppError::db)?;
        let mut generated = 0usize;
        let mut first_request = true;

        for skill in store.get_all_skills().map_err(AppError::db)? {
            if existing.contains_key(&skill.id) {
                continue;
            }
            // Prefer the description shown in the list view (SKILL.md
            // frontmatter wins, exactly like managed_skill_to_dto).
            let description = skill_metadata::parse_skill_md(std::path::Path::new(
                &skill.central_path,
            ))
            .description
            .or(skill.description)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
            let Some(description) = description else {
                continue;
            };

            if !first_request {
                std::thread::sleep(BATCH_PACING);
            }
            first_request = false;

            let translated =
                translate_api::translate_text(&description, "auto", "zh", &app_id, &api_key, proxy_url.as_deref())
                    .map_err(AppError::network)?;
            sync_metadata::with_repo_lock("set skill notes", || {
                store.set_notes_for_skill(&skill.id, Some(&translated))?;
                sync_metadata::ensure_skill_metadata_unlocked(&store, &skill.id)
            })
            .map_err(AppError::db)?;
            generated += 1;
        }

        Ok(generated)
    })
    .await?
}
