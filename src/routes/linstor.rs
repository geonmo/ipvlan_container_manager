use axum::{
    extract::State,
    response::Html,
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::linstor::{
    LinstorConfig, LinstorResourceGroup, LinstorResourceSpawn, LinstorStoragePool,
};
use crate::generators::linstor::{
    generate_linstor_ansible_playbook, generate_linstor_inventory, generate_linstor_requirements_yml,
};

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("linstor/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

#[derive(Debug, Deserialize)]
pub struct LinstorFormData {
    /// JSON 배열: ["node1", ...]
    pub controller_nodes_json: Option<String>,
    /// JSON 배열: ["node1", "node2", ...]
    pub satellite_nodes_json: Option<String>,
    pub rpm_dir: String,
    /// JSON 배열: [{name, pool_type, vg, vg_thinpool, zpool, file_path, physical_devices, nodes}]
    pub storage_pools_json: Option<String>,
    /// JSON 배열: [{name, storage_pool, place_count}]
    pub resource_groups_json: Option<String>,
    /// JSON 배열: [{name, resource_group, size}]
    pub resources_json: Option<String>,
    pub deploy_storage: Option<String>,
    pub ha_database: Option<String>,
    pub token_auth: Option<String>,
    pub ansible_user: Option<String>,
    pub ansible_ssh_key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LinstorResult {
    pub requirements_yml: String,
    pub inventory: String,
    pub ansible_playbook: String,
}

fn parse_json_vec<T: for<'de> Deserialize<'de> + Default>(json: &Option<String>) -> Vec<T> {
    json.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<Vec<T>>(s).ok())
        .unwrap_or_default()
}

pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<LinstorFormData>,
) -> Html<String> {
    let controller_nodes: Vec<String> = parse_json_vec(&form.controller_nodes_json);
    let satellite_nodes: Vec<String> = parse_json_vec(&form.satellite_nodes_json);
    let storage_pools: Vec<LinstorStoragePool> = parse_json_vec(&form.storage_pools_json);
    let resource_groups: Vec<LinstorResourceGroup> = parse_json_vec(&form.resource_groups_json);
    let resources: Vec<LinstorResourceSpawn> = parse_json_vec(&form.resources_json);

    let config = LinstorConfig {
        controller_nodes,
        satellite_nodes,
        rpm_dir: form.rpm_dir.clone(),
        storage_pools,
        resource_groups,
        resources,
        deploy_storage: form.deploy_storage.as_deref() == Some("on"),
        ha_database: form.ha_database.as_deref() == Some("on"),
        token_auth: form.token_auth.as_deref() != Some("off"),
    };

    let ansible_user = form.ansible_user.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "root".to_string());
    let ansible_ssh_key = form.ansible_ssh_key.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "~/.ssh/id_rsa".to_string());

    // 노드 풀에 등록된 IP 조회 (없으면 hostname을 그대로 ansible_host로 사용)
    let node_ips = {
        let conn = state.db.lock().unwrap();
        crate::db::list_nodes(&conn)
            .unwrap_or_default()
            .into_iter()
            .map(|n| (n.hostname, n.ip))
            .collect::<std::collections::HashMap<_, _>>()
    };

    let requirements_yml = generate_linstor_requirements_yml();
    let inventory = generate_linstor_inventory(&config, &node_ips, &ansible_user, &ansible_ssh_key);
    let ansible_playbook = generate_linstor_ansible_playbook(&config, &ansible_user, &ansible_ssh_key);

    let result = LinstorResult {
        requirements_yml,
        inventory,
        ansible_playbook,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);
    let rendered = state.tera.render("linstor/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}
