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
    pub subnet: String,
    pub gateway: String,
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
             ipvlan_mode TEXT    NOT NULL DEFAULT 'l2',
             source      TEXT    NOT NULL DEFAULT 'manual',
             created_at  TEXT    NOT NULL
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
        "INSERT INTO networks (name, driver, interface, subnet, gateway, ipvlan_mode, source, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(name) DO UPDATE SET
             driver      = excluded.driver,
             interface   = excluded.interface,
             subnet      = excluded.subnet,
             gateway     = excluded.gateway,
             ipvlan_mode = excluded.ipvlan_mode,
             source      = excluded.source",
        params![n.name, n.driver, n.interface, n.subnet, n.gateway, n.ipvlan_mode, n.source, now],
    )?;
    Ok(())
}

pub fn list_networks(conn: &Connection) -> Result<Vec<DbNetwork>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, driver, interface, subnet, gateway, ipvlan_mode, source, created_at
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
            ipvlan_mode: row.get(6)?,
            source:      row.get(7)?,
            created_at:  row.get(8)?,
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

pub fn delete_ansible_profile(conn: &Connection, id: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM ansible_profiles WHERE id=?1", params![id])?)
}
