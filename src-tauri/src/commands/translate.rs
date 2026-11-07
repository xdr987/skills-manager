use std::sync::Arc;
use tauri::State;

use crate::core::{error::AppError, skill_store::SkillStore, translate_api};

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
        let proxy_url = store.proxy_url();
        translate_api::translate_text(
            &text,
            "auto",
            "zh",
            app_id.trim(),
            api_key.trim(),
            proxy_url.as_deref(),
        )
        .map_err(AppError::network)
    })
    .await?
}
