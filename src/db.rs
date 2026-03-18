use rusqlite::{Connection, Result, params};
use serde::{Deserialize, Serialize};

/// DB에 저장되는 DRBD 리소스 레코드
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbDrbdResource {
    pub id: i64,
    pub resource_name: String,
    pub protocol: String,
    pub minor: u32,
    pub nodes_json: String,
    pub net_options_json: String,
    pub disk_options_json: String,
    pub startup_options_json: String,
    /// "manual" | "imported"
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 클러스터 노드 레코드
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbNode {
    pub id: i64,
    pub hostname: String,
    pub ip: String,
    pub ssh_user: String,
    /// "manual" | "scanned"
    pub source: String,
    pub created_at: String,
}

/// 네트워크 풀 레코드
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbNetwork {
    pub id: i64,
    pub name: String,
    pub driver: String,
    pub interface: String,
    /// IPv4 서브넷 (예: 192.168.1.0/24)
    pub subnet: String,
    /// IPv4 게이트웨이
    pub gateway: String,
    /// IPv6 서브넷 (예: 2001:db8::/64), 없으면 빈 문자열
    pub subnet6: String,
    /// IPv6 게이트웨이, 없으면 빈 문자열
    pub gateway6: String,
    /// IPv6 활성화 여부 (IPv6=true/false)
    pub ipv6: bool,
    pub ipvlan_mode: String,
    /// "manual" | "scanned"
    pub source: String,
    pub created_at: String,
}

/// Ansible 인증 프로파일
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbAnsibleProfile {
    pub id: i64,
    pub name: String,
    /// "key" | "password"
    pub auth_method: String,
    pub ssh_user: String,
    pub ssh_key: String,
    pub ssh_password: String,
    #[serde(rename = "become")]
    pub do_become: bool,
    pub become_method: String,
    pub become_password: String,
    pub created_at: String,
    pub updated_at: String,
}

/// DB 초기화 (테이블 생성)
pub fn init_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         -- 기존 networks 테이블에 새 컬럼 추가 (이미 있으면 무시)
         ALTER TABLE networks ADD COLUMN subnet6  TEXT NOT NULL DEFAULT '' ;
         ALTER TABLE networks ADD COLUMN gateway6 TEXT NOT NULL DEFAULT '' ;
         ALTER TABLE networks ADD COLUMN ipv6     INTEGER NOT NULL DEFAULT 0 ;
        "
    ).ok(); // 이미 존재하는 컬럼 오류는 무시

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;

         CREATE TABLE IF NOT EXISTS drbd_resources (
             id                   INTEGER PRIMARY KEY AUTOINCREMENT,
             resource_name        TEXT    NOT NULL UNIQUE,
             protocol             TEXT    NOT NULL DEFAULT 'C',
             minor                INTEGER NOT NULL,
             nodes_json           TEXT    NOT NULL,
             net_options_json     TEXT    NOT NULL DEFAULT '{}',
             disk_options_json    TEXT    NOT NULL DEFAULT '{}',
             startup_options_json TEXT    NOT NULL DEFAULT '{}',
             source               TEXT    NOT NULL DEFAULT 'manual',
             created_at           TEXT    NOT NULL,
             updated_at           TEXT    NOT NULL
         );

         CREATE TABLE IF NOT EXISTS nodes (
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             hostname   TEXT    NOT NULL UNIQUE,
             ip         TEXT    NOT NULL DEFAULT '',
             ssh_user   TEXT    NOT NULL DEFAULT 'root',
             source     TEXT    NOT NULL DEFAULT 'manual',
             created_at TEXT    NOT NULL
         );

         CREATE TABLE IF NOT EXISTS networks (
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             name        TEXT    NOT NULL UNIQUE,
             driver      TEXT    NOT NULL DEFAULT 'ipvlan',
             interface   TEXT    NOT NULL DEFAULT '',
             subnet      TEXT    NOT NULL DEFAULT '',
             gateway     TEXT    NOT NULL DEFAULT '',
             subnet6     TEXT    NOT NULL DEFAULT '',
             gateway6    TEXT    NOT NULL DEFAULT '',
             ipv6        INTEGER NOT NULL DEFAULT 0,
             ipvlan_mode TEXT    NOT NULL DEFAULT 'l2',
             source      TEXT    NOT NULL DEFAULT 'manual',
             created_at  TEXT    NOT NULL
         );

         CREATE TABLE IF NOT EXISTS node_interfaces (
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             hostname   TEXT NOT NULL,
             interface  TEXT NOT NULL,
             ip         TEXT NOT NULL,
             prefix_len INTEGER NOT NULL DEFAULT 0,
             family     TEXT NOT NULL DEFAULT 'inet',
             created_at TEXT NOT NULL,
             UNIQUE(hostname, interface, ip)
         );

         CREATE TABLE IF NOT EXISTS ansible_profiles (
             id               INTEGER PRIMARY KEY AUTOINCREMENT,
             name             TEXT    NOT NULL UNIQUE,
             auth_method      TEXT    NOT NULL DEFAULT 'key',
             ssh_user         TEXT    NOT NULL DEFAULT 'root',
             ssh_key          TEXT    NOT NULL DEFAULT '~/.ssh/id_rsa',
             ssh_password     TEXT    NOT NULL DEFAULT '',
             become           INTEGER NOT NULL DEFAULT 1,
             become_method    TEXT    NOT NULL DEFAULT 'sudo',
             become_password  TEXT    NOT NULL DEFAULT '',
             created_at       TEXT    NOT NULL,
             updated_at       TEXT    NOT NULL
         );

         CREATE TABLE IF NOT EXISTS nft_subnet_groups (
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             name        TEXT    NOT NULL UNIQUE,
             description TEXT    NOT NULL DEFAULT '',
             created_at  TEXT    NOT NULL
         );
         CREATE TABLE IF NOT EXISTS nft_subnets (
             id       INTEGER PRIMARY KEY AUTOINCREMENT,
             group_id INTEGER NOT NULL,
             cidr     TEXT    NOT NULL,
             family   TEXT    NOT NULL DEFAULT 'v4'
         );
         CREATE TABLE IF NOT EXISTS nft_services (
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             name        TEXT    NOT NULL UNIQUE,
             description TEXT    NOT NULL DEFAULT '',
             created_at  TEXT    NOT NULL
         );
         CREATE TABLE IF NOT EXISTS nft_service_ports (
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             service_id INTEGER NOT NULL,
             protocol   TEXT    NOT NULL DEFAULT 'tcp',
             port       TEXT    NOT NULL
         );

         CREATE TABLE IF NOT EXISTS nft_global_config (
             id               INTEGER NOT NULL DEFAULT 1 CHECK (id = 1),
             table_name       TEXT    NOT NULL DEFAULT 'filter_ingress',
             device_name      TEXT    NOT NULL DEFAULT 'eth0',
             chain_name       TEXT    NOT NULL DEFAULT '',
             traceroute_start INTEGER NOT NULL DEFAULT 33434,
             traceroute_end   INTEGER NOT NULL DEFAULT 65535,
             nft_file         TEXT    NOT NULL DEFAULT '/etc/nftables/ipvlan_l2.nft',
             updated_at       TEXT    NOT NULL DEFAULT ''
         );
         INSERT OR IGNORE INTO nft_global_config (id) VALUES (1);

         CREATE TABLE IF NOT EXISTS nft_targets (
             id          INTEGER PRIMARY KEY AUTOINCREMENT,
             name        TEXT    NOT NULL UNIQUE,
             ipv4_addrs  TEXT    NOT NULL DEFAULT '[]',
             ipv6_addrs  TEXT    NOT NULL DEFAULT '[]',
             created_at  TEXT    NOT NULL
         );

         CREATE TABLE IF NOT EXISTS nft_target_rules (
             id                INTEGER PRIMARY KEY AUTOINCREMENT,
             target_id         INTEGER NOT NULL,
             service_name      TEXT    NOT NULL,
             subnet_group_name TEXT    NOT NULL DEFAULT '',
             sort_order        INTEGER NOT NULL DEFAULT 0
         );

         CREATE TABLE IF NOT EXISTS volumes (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             name          TEXT    NOT NULL UNIQUE,
             host_path     TEXT    NOT NULL DEFAULT '',
             description   TEXT    NOT NULL DEFAULT '',
             drbd_resource TEXT    NOT NULL DEFAULT '',
             created_at    TEXT    NOT NULL DEFAULT ''
         );",
    )?;
    Ok(conn)
}

// ─── DRBD Resources ────────────────────────────────────────────────────────

/// 리소스 저장 (없으면 INSERT, 있으면 UPDATE)
pub fn upsert_resource(conn: &Connection, r: &DbDrbdResource) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO drbd_resources
             (resource_name, protocol, minor, nodes_json,
              net_options_json, disk_options_json, startup_options_json,
              source, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
         ON CONFLICT(resource_name) DO UPDATE SET
             protocol             = excluded.protocol,
             minor                = excluded.minor,
             nodes_json           = excluded.nodes_json,
             net_options_json     = excluded.net_options_json,
             disk_options_json    = excluded.disk_options_json,
             startup_options_json = excluded.startup_options_json,
             source               = excluded.source,
             updated_at           = ?9",
        params![
            r.resource_name,
            r.protocol,
            r.minor,
            r.nodes_json,
            r.net_options_json,
            r.disk_options_json,
            r.startup_options_json,
            r.source,
            now,
        ],
    )?;
    Ok(())
}

/// 모든 리소스 조회
pub fn list_resources(conn: &Connection) -> Result<Vec<DbDrbdResource>> {
    let mut stmt = conn.prepare(
        "SELECT id, resource_name, protocol, minor, nodes_json,
                net_options_json, disk_options_json, startup_options_json,
                source, created_at, updated_at
         FROM drbd_resources
         ORDER BY minor ASC, resource_name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbDrbdResource {
            id:                   row.get(0)?,
            resource_name:        row.get(1)?,
            protocol:             row.get(2)?,
            minor:                row.get::<_, i64>(3)? as u32,
            nodes_json:           row.get(4)?,
            net_options_json:     row.get(5)?,
            disk_options_json:    row.get(6)?,
            startup_options_json: row.get(7)?,
            source:               row.get(8)?,
            created_at:           row.get(9)?,
            updated_at:           row.get(10)?,
        })
    })?;
    rows.collect()
}

/// 이름으로 단일 리소스 조회
pub fn get_resource(conn: &Connection, name: &str) -> Result<Option<DbDrbdResource>> {
    let mut stmt = conn.prepare(
        "SELECT id, resource_name, protocol, minor, nodes_json,
                net_options_json, disk_options_json, startup_options_json,
                source, created_at, updated_at
         FROM drbd_resources WHERE resource_name = ?1",
    )?;
    let mut rows = stmt.query_map(params![name], |row| {
        Ok(DbDrbdResource {
            id:                   row.get(0)?,
            resource_name:        row.get(1)?,
            protocol:             row.get(2)?,
            minor:                row.get::<_, i64>(3)? as u32,
            nodes_json:           row.get(4)?,
            net_options_json:     row.get(5)?,
            disk_options_json:    row.get(6)?,
            startup_options_json: row.get(7)?,
            source:               row.get(8)?,
            created_at:           row.get(9)?,
            updated_at:           row.get(10)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

/// 이름으로 리소스 삭제
pub fn delete_resource(conn: &Connection, name: &str) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM drbd_resources WHERE resource_name = ?1",
        params![name],
    )?;
    Ok(n)
}

// ─── Nodes ─────────────────────────────────────────────────────────────────

pub fn upsert_node(conn: &Connection, n: &DbNode) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO nodes (hostname, ip, ssh_user, source, created_at)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(hostname) DO UPDATE SET
             ip       = excluded.ip,
             ssh_user = excluded.ssh_user,
             source   = excluded.source",
        params![n.hostname, n.ip, n.ssh_user, n.source, now],
    )?;
    Ok(())
}

pub fn list_nodes(conn: &Connection) -> Result<Vec<DbNode>> {
    let mut stmt = conn.prepare(
        "SELECT id, hostname, ip, ssh_user, source, created_at
         FROM nodes ORDER BY hostname ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbNode {
            id:         row.get(0)?,
            hostname:   row.get(1)?,
            ip:         row.get(2)?,
            ssh_user:   row.get(3)?,
            source:     row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn delete_node(conn: &Connection, id: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM nodes WHERE id=?1", params![id])?)
}

// ─── Networks ──────────────────────────────────────────────────────────────

pub fn upsert_network(conn: &Connection, n: &DbNetwork) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO networks
             (name, driver, interface, subnet, gateway, subnet6, gateway6, ipv6, ipvlan_mode, source, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(name) DO UPDATE SET
             driver      = excluded.driver,
             interface   = excluded.interface,
             subnet      = excluded.subnet,
             gateway     = excluded.gateway,
             subnet6     = excluded.subnet6,
             gateway6    = excluded.gateway6,
             ipv6        = excluded.ipv6,
             ipvlan_mode = excluded.ipvlan_mode,
             source      = excluded.source",
        params![
            n.name, n.driver, n.interface,
            n.subnet, n.gateway,
            n.subnet6, n.gateway6, n.ipv6 as i64,
            n.ipvlan_mode, n.source, now
        ],
    )?;
    Ok(())
}

pub fn list_networks(conn: &Connection) -> Result<Vec<DbNetwork>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, driver, interface, subnet, gateway, subnet6, gateway6, ipv6, ipvlan_mode, source, created_at
         FROM networks ORDER BY name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbNetwork {
            id:          row.get(0)?,
            name:        row.get(1)?,
            driver:      row.get(2)?,
            interface:   row.get(3)?,
            subnet:      row.get(4)?,
            gateway:     row.get(5)?,
            subnet6:     row.get(6)?,
            gateway6:    row.get(7)?,
            ipv6:        row.get::<_, i64>(8)? != 0,
            ipvlan_mode: row.get(9)?,
            source:      row.get(10)?,
            created_at:  row.get(11)?,
        })
    })?;
    rows.collect()
}

pub fn delete_network(conn: &Connection, id: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM networks WHERE id=?1", params![id])?)
}

// ─── Ansible Profiles ──────────────────────────────────────────────────────

pub fn upsert_ansible_profile(conn: &Connection, p: &DbAnsibleProfile) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO ansible_profiles
             (name, auth_method, ssh_user, ssh_key, ssh_password,
              become, become_method, become_password, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
         ON CONFLICT(name) DO UPDATE SET
             auth_method     = excluded.auth_method,
             ssh_user        = excluded.ssh_user,
             ssh_key         = excluded.ssh_key,
             ssh_password    = excluded.ssh_password,
             become          = excluded.become,
             become_method   = excluded.become_method,
             become_password = excluded.become_password,
             updated_at      = ?9",
        params![
            p.name, p.auth_method, p.ssh_user, p.ssh_key, p.ssh_password,
            p.do_become as i64, p.become_method, p.become_password, now,
        ],
    )?;
    Ok(())
}

pub fn list_ansible_profiles(conn: &Connection) -> Result<Vec<DbAnsibleProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, auth_method, ssh_user, ssh_key, ssh_password,
                become, become_method, become_password, created_at, updated_at
         FROM ansible_profiles ORDER BY name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbAnsibleProfile {
            id:              row.get(0)?,
            name:            row.get(1)?,
            auth_method:     row.get(2)?,
            ssh_user:        row.get(3)?,
            ssh_key:         row.get(4)?,
            ssh_password:    row.get(5)?,
            do_become:       row.get::<_, i64>(6)? != 0,
            become_method:   row.get(7)?,
            become_password: row.get(8)?,
            created_at:      row.get(9)?,
            updated_at:      row.get(10)?,
        })
    })?;
    rows.collect()
}

pub fn get_ansible_profile_by_name(conn: &Connection, name: &str) -> Result<Option<DbAnsibleProfile>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, auth_method, ssh_user, ssh_key, ssh_password,
                become, become_method, become_password, created_at, updated_at
         FROM ansible_profiles WHERE name=?1 LIMIT 1",
    )?;
    let mut rows = stmt.query_map(params![name], |row| {
        Ok(DbAnsibleProfile {
            id:              row.get(0)?,
            name:            row.get(1)?,
            auth_method:     row.get(2)?,
            ssh_user:        row.get(3)?,
            ssh_key:         row.get(4)?,
            ssh_password:    row.get(5)?,
            do_become:       row.get::<_, i64>(6)? != 0,
            become_method:   row.get(7)?,
            become_password: row.get(8)?,
            created_at:      row.get(9)?,
            updated_at:      row.get(10)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn delete_ansible_profile(conn: &Connection, id: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM ansible_profiles WHERE id=?1", params![id])?)
}

// ─── Volumes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbVolume {
    pub id: i64,
    pub name: String,
    pub host_path: String,
    pub description: String,
    pub drbd_resource: String,
    pub created_at: String,
}

pub fn list_volumes(conn: &Connection) -> Result<Vec<DbVolume>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, host_path, description, drbd_resource, created_at
         FROM volumes ORDER BY name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbVolume {
            id:            row.get(0)?,
            name:          row.get(1)?,
            host_path:     row.get(2)?,
            description:   row.get(3)?,
            drbd_resource: row.get(4)?,
            created_at:    row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn upsert_volume(conn: &Connection, v: &DbVolume) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO volumes (name, host_path, description, drbd_resource, created_at)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(name) DO UPDATE SET
             host_path=excluded.host_path,
             description=excluded.description,
             drbd_resource=excluded.drbd_resource",
        params![v.name, v.host_path, v.description, v.drbd_resource, now],
    )?;
    Ok(())
}

pub fn delete_volume(conn: &Connection, id: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM volumes WHERE id=?1", params![id])?)
}

/// 볼륨이 없을 때만 삽입 (스캔 자동 등록용 — 수동 설정을 덮어쓰지 않음)
pub fn insert_volume_if_not_exists(conn: &Connection, v: &DbVolume) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO volumes (name, host_path, description, drbd_resource, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![v.name, v.host_path, v.description, v.drbd_resource, now],
    )?;
    Ok(())
}

// ─── Node Interfaces ───────────────────────────────────────────────────────

/// 노드별 네트워크 인터페이스 + IP 정보 (Ansible 수집 결과)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbNodeInterface {
    pub id: i64,
    pub hostname: String,
    pub interface: String,
    pub ip: String,
    pub prefix_len: u8,
    /// "inet" | "inet6"
    pub family: String,
    pub created_at: String,
}

pub fn upsert_node_interface(conn: &Connection, n: &DbNodeInterface) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO node_interfaces (hostname, interface, ip, prefix_len, family, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(hostname, interface, ip) DO UPDATE SET
             prefix_len = excluded.prefix_len,
             family     = excluded.family",
        params![n.hostname, n.interface, n.ip, n.prefix_len as i64, n.family, now],
    )?;
    Ok(())
}

pub fn list_node_interfaces(conn: &Connection) -> Result<Vec<DbNodeInterface>> {
    let mut stmt = conn.prepare(
        "SELECT id, hostname, interface, ip, prefix_len, family, created_at
         FROM node_interfaces ORDER BY hostname, interface, family, ip",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(DbNodeInterface {
            id:         row.get(0)?,
            hostname:   row.get(1)?,
            interface:  row.get(2)?,
            ip:         row.get(3)?,
            prefix_len: row.get::<_, i64>(4)? as u8,
            family:     row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn delete_node_interfaces_for_host(conn: &Connection, hostname: &str) -> Result<usize> {
    Ok(conn.execute("DELETE FROM node_interfaces WHERE hostname=?1", params![hostname])?)
}

/// 특정 네트워크 레코드의 interface 필드만 갱신
pub fn update_network_interface(conn: &Connection, name: &str, interface: &str) -> Result<()> {
    conn.execute(
        "UPDATE networks SET interface=?1 WHERE name=?2",
        params![interface, name],
    )?;
    Ok(())
}

// ─── NFT Subnet Groups ──────────────────────────────────────────────────────

pub fn list_nft_subnet_groups(conn: &Connection) -> Result<Vec<crate::models::nft::NftSubnetGroup>> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.name, g.description, g.created_at, s.cidr, s.family
         FROM nft_subnet_groups g
         LEFT JOIN nft_subnets s ON s.group_id = g.id
         ORDER BY g.name, s.family, s.cidr",
    )?;

    let mut groups: Vec<crate::models::nft::NftSubnetGroup> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64        = row.get(0)?;
        let name: String   = row.get(1)?;
        let desc: String   = row.get(2)?;
        let cidr: Option<String>   = row.get(4)?;
        let family: Option<String> = row.get(5)?;

        if let Some(grp) = groups.iter_mut().find(|g| g.id == id) {
            if let (Some(c), Some(f)) = (cidr, family) {
                if f == "v4" { grp.cidrs_v4.push(c); }
                else          { grp.cidrs_v6.push(c); }
            }
        } else {
            let mut grp = crate::models::nft::NftSubnetGroup {
                id,
                name,
                description: desc,
                cidrs_v4: Vec::new(),
                cidrs_v6: Vec::new(),
            };
            if let (Some(c), Some(f)) = (cidr, family) {
                if f == "v4" { grp.cidrs_v4.push(c); }
                else          { grp.cidrs_v6.push(c); }
            }
            groups.push(grp);
        }
    }
    Ok(groups)
}

pub fn upsert_nft_subnet_group(conn: &Connection, g: &crate::models::nft::NftSubnetGroup) -> Result<i64> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO nft_subnet_groups (name, description, created_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET
             description = excluded.description",
        params![g.name, g.description, now],
    )?;
    let group_id: i64 = conn.query_row(
        "SELECT id FROM nft_subnet_groups WHERE name = ?1",
        params![g.name],
        |row| row.get(0),
    )?;
    conn.execute("DELETE FROM nft_subnets WHERE group_id = ?1", params![group_id])?;
    for cidr in &g.cidrs_v4 {
        conn.execute(
            "INSERT INTO nft_subnets (group_id, cidr, family) VALUES (?1, ?2, 'v4')",
            params![group_id, cidr],
        )?;
    }
    for cidr in &g.cidrs_v6 {
        conn.execute(
            "INSERT INTO nft_subnets (group_id, cidr, family) VALUES (?1, ?2, 'v6')",
            params![group_id, cidr],
        )?;
    }
    Ok(group_id)
}

pub fn delete_nft_subnet_group(conn: &Connection, id: i64) -> Result<usize> {
    conn.execute("DELETE FROM nft_subnets WHERE group_id = ?1", params![id])?;
    Ok(conn.execute("DELETE FROM nft_subnet_groups WHERE id = ?1", params![id])?)
}

pub fn get_nft_subnet_groups_by_names(conn: &Connection, names: &[String]) -> Result<Vec<crate::models::nft::NftSubnetGroup>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let all = list_nft_subnet_groups(conn)?;
    Ok(all.into_iter().filter(|g| names.contains(&g.name)).collect())
}

// ─── NFT Services ──────────────────────────────────────────────────────────

pub fn list_nft_services(conn: &Connection) -> Result<Vec<crate::models::nft::NftServiceDef>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.description, s.created_at, p.protocol, p.port
         FROM nft_services s
         LEFT JOIN nft_service_ports p ON p.service_id = s.id
         ORDER BY s.name, p.protocol, p.port",
    )?;

    let mut services: Vec<crate::models::nft::NftServiceDef> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64        = row.get(0)?;
        let name: String   = row.get(1)?;
        let desc: String   = row.get(2)?;
        let proto: Option<String> = row.get(4)?;
        let port:  Option<String> = row.get(5)?;

        if let Some(svc) = services.iter_mut().find(|s| s.id == id) {
            if let (Some(proto), Some(port)) = (proto, port) {
                if proto == "tcp" { svc.tcp_ports.push(port); }
                else               { svc.udp_ports.push(port); }
            }
        } else {
            let mut svc = crate::models::nft::NftServiceDef {
                id,
                name,
                description: desc,
                tcp_ports: Vec::new(),
                udp_ports: Vec::new(),
            };
            if let (Some(proto), Some(port)) = (proto, port) {
                if proto == "tcp" { svc.tcp_ports.push(port); }
                else               { svc.udp_ports.push(port); }
            }
            services.push(svc);
        }
    }
    Ok(services)
}

pub fn upsert_nft_service(conn: &Connection, s: &crate::models::nft::NftServiceDef) -> Result<i64> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO nft_services (name, description, created_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET
             description = excluded.description",
        params![s.name, s.description, now],
    )?;
    let service_id: i64 = conn.query_row(
        "SELECT id FROM nft_services WHERE name = ?1",
        params![s.name],
        |row| row.get(0),
    )?;
    conn.execute("DELETE FROM nft_service_ports WHERE service_id = ?1", params![service_id])?;
    for port in &s.tcp_ports {
        conn.execute(
            "INSERT INTO nft_service_ports (service_id, protocol, port) VALUES (?1, 'tcp', ?2)",
            params![service_id, port],
        )?;
    }
    for port in &s.udp_ports {
        conn.execute(
            "INSERT INTO nft_service_ports (service_id, protocol, port) VALUES (?1, 'udp', ?2)",
            params![service_id, port],
        )?;
    }
    Ok(service_id)
}

pub fn delete_nft_service(conn: &Connection, id: i64) -> Result<usize> {
    conn.execute("DELETE FROM nft_service_ports WHERE service_id = ?1", params![id])?;
    Ok(conn.execute("DELETE FROM nft_services WHERE id = ?1", params![id])?)
}

pub fn get_nft_services_by_names(conn: &Connection, names: &[String]) -> Result<Vec<crate::models::nft::NftServiceDef>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let all = list_nft_services(conn)?;
    Ok(all.into_iter().filter(|s| names.contains(&s.name)).collect())
}

/// nft_service를 이름 충돌 시 무시하는 INSERT OR IGNORE 방식으로 삽입
pub fn insert_nft_service_if_not_exists(conn: &Connection, s: &crate::models::nft::NftServiceDef) -> Result<i64> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO nft_services (name, description, created_at) VALUES (?1, ?2, ?3)",
        params![s.name, s.description, now],
    )?;
    let service_id: i64 = conn.query_row(
        "SELECT id FROM nft_services WHERE name = ?1",
        params![s.name],
        |row| row.get(0),
    )?;
    // 포트가 이미 있는지 확인, 없는 경우에만 삽입
    let existing_ports: i64 = conn.query_row(
        "SELECT COUNT(*) FROM nft_service_ports WHERE service_id = ?1",
        params![service_id],
        |row| row.get(0),
    )?;
    if existing_ports == 0 {
        for port in &s.tcp_ports {
            conn.execute(
                "INSERT INTO nft_service_ports (service_id, protocol, port) VALUES (?1, 'tcp', ?2)",
                params![service_id, port],
            )?;
        }
        for port in &s.udp_ports {
            conn.execute(
                "INSERT INTO nft_service_ports (service_id, protocol, port) VALUES (?1, 'udp', ?2)",
                params![service_id, port],
            )?;
        }
    }
    Ok(service_id)
}

// ─── NFT Global Config ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbNftGlobalConfig {
    pub table_name:       String,
    pub device_name:      String,
    pub chain_name:       String,
    pub traceroute_start: u16,
    pub traceroute_end:   u16,
    pub nft_file:         String,
    pub updated_at:       String,
}

pub fn get_nft_global_config(conn: &Connection) -> Result<DbNftGlobalConfig> {
    conn.query_row(
        "SELECT table_name, device_name, chain_name, traceroute_start, traceroute_end, nft_file, updated_at
         FROM nft_global_config WHERE id = 1",
        [],
        |row| Ok(DbNftGlobalConfig {
            table_name:       row.get(0)?,
            device_name:      row.get(1)?,
            chain_name:       row.get(2)?,
            traceroute_start: row.get::<_, i64>(3)? as u16,
            traceroute_end:   row.get::<_, i64>(4)? as u16,
            nft_file:         row.get(5)?,
            updated_at:       row.get(6)?,
        }),
    )
}

pub fn update_nft_global_config(conn: &Connection, cfg: &DbNftGlobalConfig) -> Result<()> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "UPDATE nft_global_config SET
             table_name       = ?1,
             device_name      = ?2,
             chain_name       = ?3,
             traceroute_start = ?4,
             traceroute_end   = ?5,
             nft_file         = ?6,
             updated_at       = ?7
         WHERE id = 1",
        params![
            cfg.table_name,
            cfg.device_name,
            cfg.chain_name,
            cfg.traceroute_start as i64,
            cfg.traceroute_end as i64,
            cfg.nft_file,
            now,
        ],
    )?;
    Ok(())
}

// ─── NFT Targets ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbNftTargetRule {
    pub id:                i64,
    pub service_name:      String,
    pub subnet_group_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbNftTarget {
    pub id:         i64,
    pub name:       String,
    pub ipv4_addrs: Vec<String>,
    pub ipv6_addrs: Vec<String>,
    pub rules:      Vec<DbNftTargetRule>,
}

pub fn list_nft_targets(conn: &Connection) -> Result<Vec<DbNftTarget>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.ipv4_addrs, t.ipv6_addrs
         FROM nft_targets t ORDER BY t.name ASC",
    )?;
    let mut targets: Vec<DbNftTarget> = stmt.query_map([], |row| {
        let ipv4_json: String = row.get(2)?;
        let ipv6_json: String = row.get(3)?;
        Ok(DbNftTarget {
            id:         row.get(0)?,
            name:       row.get(1)?,
            ipv4_addrs: serde_json::from_str(&ipv4_json).unwrap_or_default(),
            ipv6_addrs: serde_json::from_str(&ipv6_json).unwrap_or_default(),
            rules:      Vec::new(),
        })
    })?.collect::<Result<Vec<_>>>()?;

    for target in &mut targets {
        let mut rule_stmt = conn.prepare(
            "SELECT id, service_name, subnet_group_name FROM nft_target_rules
             WHERE target_id = ?1 ORDER BY sort_order ASC",
        )?;
        target.rules = rule_stmt.query_map(params![target.id], |row| {
            Ok(DbNftTargetRule {
                id:                row.get(0)?,
                service_name:      row.get(1)?,
                subnet_group_name: row.get(2)?,
            })
        })?.collect::<Result<Vec<_>>>()?;
    }
    Ok(targets)
}

pub fn upsert_nft_target(conn: &Connection, t: &DbNftTarget) -> Result<i64> {
    let now = chrono::Local::now().to_rfc3339();
    let ipv4_json = serde_json::to_string(&t.ipv4_addrs).unwrap_or_else(|_| "[]".to_string());
    let ipv6_json = serde_json::to_string(&t.ipv6_addrs).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO nft_targets (name, ipv4_addrs, ipv6_addrs, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET
             ipv4_addrs = excluded.ipv4_addrs,
             ipv6_addrs = excluded.ipv6_addrs",
        params![t.name, ipv4_json, ipv6_json, now],
    )?;

    let target_id: i64 = conn.query_row(
        "SELECT id FROM nft_targets WHERE name = ?1",
        params![t.name],
        |row| row.get(0),
    )?;

    conn.execute("DELETE FROM nft_target_rules WHERE target_id = ?1", params![target_id])?;

    for (i, rule) in t.rules.iter().enumerate() {
        conn.execute(
            "INSERT INTO nft_target_rules (target_id, service_name, subnet_group_name, sort_order)
             VALUES (?1, ?2, ?3, ?4)",
            params![target_id, rule.service_name, rule.subnet_group_name, i as i64],
        )?;
    }

    Ok(target_id)
}

pub fn delete_nft_target(conn: &Connection, id: i64) -> Result<usize> {
    conn.execute("DELETE FROM nft_target_rules WHERE target_id = ?1", params![id])?;
    Ok(conn.execute("DELETE FROM nft_targets WHERE id = ?1", params![id])?)
}
