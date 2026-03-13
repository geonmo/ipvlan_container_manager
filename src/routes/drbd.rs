use axum::{
    extract::State,
    response::Html,
    Form,
};
use serde::{Deserialize, Serialize};
use tera::Context;
use crate::AppState;
use crate::models::drbd::{
    AnsibleInventory, AnsibleNode, DrbdDiskOptions, DrbdNetOptions, DrbdNode, DrbdResource,
    DrbdStartupOptions,
};
use crate::generators::drbd::{
    generate_res_file, generate_ansible_inventory, generate_drbd_init_commands,
};

pub async fn index(State(state): State<AppState>) -> Html<String> {
    let ctx = Context::new();
    let rendered = state.tera.render("drbd/index.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

#[derive(Debug, Deserialize)]
pub struct DrbdFormData {
    pub resource_name: String,
    pub protocol: String,
    pub minor: u32,

    // 노드 1
    pub node1_hostname: String,
    pub node1_ip: String,
    pub node1_disk: String,
    pub node1_meta: String,
    pub node1_port: u16,

    // 노드 2
    pub node2_hostname: String,
    pub node2_ip: String,
    pub node2_disk: String,
    pub node2_meta: String,
    pub node2_port: u16,

    // net options
    pub allow_two_primaries: Option<String>,
    pub after_sb_0pri: String,
    pub after_sb_1pri: String,
    pub after_sb_2pri: String,

    // disk options
    pub on_io_error: String,
    pub fencing: String,

    // startup
    pub wfc_timeout: u32,
    pub degr_wfc_timeout: u32,
    pub become_primary_on: Option<String>,

    // ansible inventory
    pub ansible_user: String,
    pub ansible_ssh_key: String,
    pub ansible_become: Option<String>,
}

#[derive(Serialize)]
pub struct DrbdResult {
    pub res_file: String,
    pub inventory_yaml: String,
    pub init_commands: Vec<String>,
    pub res_filename: String,
}

pub async fn generate(
    State(state): State<AppState>,
    Form(form): Form<DrbdFormData>,
) -> Html<String> {
    let resource = DrbdResource {
        resource_name: form.resource_name.clone(),
        protocol: form.protocol.clone(),
        minor: form.minor,
        nodes: vec![
            DrbdNode {
                hostname: form.node1_hostname.clone(),
                ip: form.node1_ip.clone(),
                disk_device: form.node1_disk.clone(),
                meta_disk: form.node1_meta.clone(),
                port: form.node1_port,
            },
            DrbdNode {
                hostname: form.node2_hostname.clone(),
                ip: form.node2_ip.clone(),
                disk_device: form.node2_disk.clone(),
                meta_disk: form.node2_meta.clone(),
                port: form.node2_port,
            },
        ],
        net_options: DrbdNetOptions {
            allow_two_primaries: form.allow_two_primaries.as_deref() == Some("on"),
            after_sb_0pri: form.after_sb_0pri.clone(),
            after_sb_1pri: form.after_sb_1pri.clone(),
            after_sb_2pri: form.after_sb_2pri.clone(),
        },
        disk_options: DrbdDiskOptions {
            on_io_error: form.on_io_error.clone(),
            fencing: form.fencing.clone(),
        },
        startup_options: DrbdStartupOptions {
            wfc_timeout: form.wfc_timeout,
            degr_wfc_timeout: form.degr_wfc_timeout,
            become_primary_on: form.become_primary_on.unwrap_or_default(),
        },
    };

    let inventory = AnsibleInventory {
        nodes: vec![
            AnsibleNode {
                hostname: form.node1_hostname.clone(),
                ip: form.node1_ip.clone(),
            },
            AnsibleNode {
                hostname: form.node2_hostname.clone(),
                ip: form.node2_ip.clone(),
            },
        ],
        ansible_user: form.ansible_user.clone(),
        ansible_ssh_private_key_file: form.ansible_ssh_key.clone(),
        r#become: form.ansible_become.as_deref() == Some("on"),
    };

    let res_file = generate_res_file(&resource);
    let inventory_yaml = generate_ansible_inventory(&inventory)
        .unwrap_or_else(|e| format!("# Error: {}", e));
    let init_commands = generate_drbd_init_commands(&resource);
    let res_filename = format!("{}.res", resource.resource_name);

    let result = DrbdResult {
        res_file,
        inventory_yaml,
        init_commands,
        res_filename,
    };

    let mut ctx = Context::new();
    ctx.insert("result", &result);
    ctx.insert("form", &serde_json::json!({
        "resource_name": form.resource_name,
        "node1_hostname": form.node1_hostname,
        "node2_hostname": form.node2_hostname,
    }));

    let rendered = state.tera.render("drbd/result.html", &ctx)
        .unwrap_or_else(|e| format!("<pre>Template error: {}</pre>", e));
    Html(rendered)
}

/// DRBD res 파일 다운로드 (plain text)
pub async fn download_res(
    State(_state): State<AppState>,
    Form(form): Form<DrbdFormData>,
) -> axum::response::Response<String> {
    let resource = form_to_resource(&form);
    let content = generate_res_file(&resource);
    let filename = format!("{}.res", resource.resource_name);

    axum::response::Response::builder()
        .header("Content-Type", "text/plain; charset=utf-8")
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(content)
        .unwrap()
}

fn form_to_resource(form: &DrbdFormData) -> DrbdResource {
    DrbdResource {
        resource_name: form.resource_name.clone(),
        protocol: form.protocol.clone(),
        minor: form.minor,
        nodes: vec![
            DrbdNode {
                hostname: form.node1_hostname.clone(),
                ip: form.node1_ip.clone(),
                disk_device: form.node1_disk.clone(),
                meta_disk: form.node1_meta.clone(),
                port: form.node1_port,
            },
            DrbdNode {
                hostname: form.node2_hostname.clone(),
                ip: form.node2_ip.clone(),
                disk_device: form.node2_disk.clone(),
                meta_disk: form.node2_meta.clone(),
                port: form.node2_port,
            },
        ],
        net_options: DrbdNetOptions {
            allow_two_primaries: form.allow_two_primaries.as_deref() == Some("on"),
            after_sb_0pri: form.after_sb_0pri.clone(),
            after_sb_1pri: form.after_sb_1pri.clone(),
            after_sb_2pri: form.after_sb_2pri.clone(),
        },
        disk_options: DrbdDiskOptions {
            on_io_error: form.on_io_error.clone(),
            fencing: form.fencing.clone(),
        },
        startup_options: DrbdStartupOptions {
            wfc_timeout: form.wfc_timeout,
            degr_wfc_timeout: form.degr_wfc_timeout,
            become_primary_on: form.become_primary_on.clone().unwrap_or_default(),
        },
    }
}
