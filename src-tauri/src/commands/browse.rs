use std::sync::Arc;
use tauri::State;

use crate::core::{
    error::AppError,
    skill_metadata,
    skill_store::SkillStore,
    skillssh_api::{self, LeaderboardType, SkillsShSkill},
};

const LEADERBOARD_CACHE_TTL: i64 = 300; // 5 minutes

/// Description + full SKILL.md content for a marketplace skill, fetched from
/// raw.githubusercontent without cloning. Candidates mirror the on-disk
/// lookup order of `git_fetcher::find_skill_dir`.
#[derive(Debug, serde::Serialize)]
pub struct MarketplaceSkillDetails {
    pub description: Option<String>,
    pub content: Option<String>,
}

/// Raw candidates in the same order `find_skill_dir` probes on disk.
fn marketplace_raw_candidates(source: &str, skill_id: &str) -> Vec<String> {
    let source = source.trim().trim_matches('/');
    [
        format!("skills/{skill_id}/SKILL.md"),
        format!("{skill_id}/SKILL.md"),
        format!(".agents/skills/{skill_id}/SKILL.md"),
    ]
    .into_iter()
    .map(|rel| format!("https://raw.githubusercontent.com/{}/HEAD/{rel}", source))
    .collect()
}

#[tauri::command]
pub async fn fetch_marketplace_skill_details(
    store: State<'_, Arc<SkillStore>>,
    source: String,
    skill_id: String,
) -> Result<MarketplaceSkillDetails, AppError> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Site skills (source without "/") have no GitHub repo behind them.
        if !source.contains('/') || skill_id.trim().is_empty() {
            return Ok(MarketplaceSkillDetails {
                description: None,
                content: None,
            });
        }
        let proxy_url = store.proxy_url();
        let client = skillssh_api::build_http_client(proxy_url.as_deref(), 20);
        for url in marketplace_raw_candidates(&source, &skill_id) {
            let response = match client.get(&url).send() {
                Ok(response) => response,
                Err(e) => return Err(AppError::network(format!("Network error: {e}"))),
            };
            if !response.status().is_success() {
                // 404 on one candidate is normal — try the next layout.
                continue;
            }
            let content = response
                .text()
                .map_err(|e| AppError::network(format!("Failed to read response: {e}")))?;
            let meta = skill_metadata::parse_frontmatter(&content);
            return Ok(MarketplaceSkillDetails {
                description: meta.description.filter(|s| !s.trim().is_empty()),
                content: Some(content),
            });
        }
        Ok(MarketplaceSkillDetails {
            description: None,
            content: None,
        })
    })
    .await?
}

#[tauri::command]
pub async fn fetch_leaderboard(
    board: String,
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<SkillsShSkill>, AppError> {
    let cache_key = format!("leaderboard_{}", board);

    // Check cache
    if let Ok(Some(cached)) = store.get_cache(&cache_key, LEADERBOARD_CACHE_TTL) {
        if let Ok(skills) = serde_json::from_str::<Vec<SkillsShSkill>>(&cached) {
            return Ok(skills);
        }
    }

    let proxy_url = store.proxy_url();
    let board_type = LeaderboardType::from_str(&board);
    let skills = tauri::async_runtime::spawn_blocking(move || {
        skillssh_api::fetch_leaderboard(board_type, proxy_url.as_deref())
            .map_err(|e| AppError::network(format!("{e:#}")))
    })
    .await??;

    // Update cache
    if let Ok(json) = serde_json::to_string(&skills) {
        store.set_cache(&cache_key, &json).ok();
    }

    Ok(skills)
}

#[tauri::command]
pub async fn search_skillssh(
    query: String,
    limit: Option<usize>,
    store: State<'_, Arc<SkillStore>>,
) -> Result<Vec<SkillsShSkill>, AppError> {
    let proxy_url = store.proxy_url();
    let requested = limit.unwrap_or(60);
    let bounded = requested.clamp(1, 300);
    tauri::async_runtime::spawn_blocking(move || {
        skillssh_api::search_skills(&query, bounded, proxy_url.as_deref())
            .map_err(|e| AppError::network(format!("{e:#}")))
    })
    .await?
}

