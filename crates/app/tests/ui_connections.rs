//! Saved connections as a user makes and uses them, against the real SQL Server, PostgreSQL
//! and MySQL servers from `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness").
//!
//! The scripts are written at run time, because they type the servers' generated ports
//! and passwords; they live in a temp folder and never in the repository. The app keeps its
//! connections file in a temp APPDATA, and its passwords under the tests' Credential
//! Manager service, which the test empties afterwards.

mod support;

use std::path::Path;

use dbide_engine::connections::credentials::Credentials;
use dbide_engine::connections::{Connection, Connections, SignInMethod};
use dbide_engine::danger::Engine;
use dbide_test_servers::mssql::Mssql;
use dbide_test_servers::mysql::MySql;
use dbide_test_servers::postgres::Postgres;
use serde_json::{Value, json};
use support::{run_script, state};

/// One server's sign-in, as the dialog asks for it.
struct Server<'a> {
    name: &'a str,
    /// Keys that pick the engine from the dialog's default, SQL Server.
    engine_keys: &'a [&'a str],
    host: &'a str,
    port: u16,
    user: &'a str,
    password: &'a str,
    database: &'a str,
}

/// Steps that add `server` as a connection with a saved password, then connect to it.
fn add_and_connect(server: &Server) -> Vec<Value> {
    let mut keys = vec![json!({ "key": "insert" }), json!({ "text": server.name })];
    keys.push(json!({ "key": "tab" }));
    keys.extend(server.engine_keys.iter().map(|k| json!({ "key": k })));
    keys.extend([
        json!({ "key": "tab" }),
        json!({ "text": server.host }),
        json!({ "key": "tab" }),
        json!({ "text": server.port.to_string() }),
    ]);
    if server.engine_keys.is_empty() {
        // SQL Server shows an Instance field after the port.
        keys.push(json!({ "key": "tab" }));
    }
    keys.extend([
        // Sign-in: from "Ask for the password" up to "Saved password".
        json!({ "key": "tab" }),
        json!({ "key": "down" }),
        json!({ "key": "up" }),
        json!({ "key": "up" }),
        json!({ "key": "enter" }),
        json!({ "key": "tab" }),
        json!({ "text": server.user }),
        json!({ "key": "tab" }),
        json!({ "text": server.password }),
        json!({ "key": "tab" }),
        json!({ "text": server.database }),
        // The lab servers' certificates are self-signed: Encryption off, and trust the
        // certificate SQL Server still uses to protect the sign-in.
        json!({ "key": "tab" }),
        json!({ "key": "tab" }),
        json!({ "key": "tab" }),
        json!({ "key": "down" }),
        json!({ "key": "down" }),
        json!({ "key": "down" }),
        json!({ "key": "enter" }),
        json!({ "key": "tab" }),
        json!({ "key": "space" }),
        json!({ "key": "enter" }),
    ]);
    vec![
        json!({ "name": format!("save {}", server.name), "input": keys }),
        json!({
            "name": format!("connect {}", server.name),
            "input": [{ "key": "enter" }, "idle"],
        }),
    ]
}

/// Deletes the passwords the app saved for the connections in `appdata`.
struct ForgetPasswords<'a>(&'a Path);

impl Drop for ForgetPasswords<'_> {
    fn drop(&mut self) {
        let file = self.0.join("Database IDE").join("connections.toml");
        let (Ok(saved), Ok(credentials)) = (
            Connections::load(file),
            Credentials::new("database-ide-test"),
        ) else {
            return;
        };
        for connection in saved.connections() {
            let _ = credentials.delete(&connection.id);
        }
    }
}

fn row<'a>(state: &'a Value, name: &str) -> &'a Value {
    state["explorer"]["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["name"] == name))
        .unwrap_or_else(|| panic!("no explorer row {name:?}: {state:#}"))
}

/// Plays one run of the app: a red folder "Lab", then each of `servers` saved in it with its
/// password and connected. The connections file stays in `appdata`.
fn save_in_a_red_folder_and_connect(servers: &[Server], dir: &Path, appdata: &Path) -> Vec<Value> {
    let mut steps = vec![
        json!({ "name": "explorer", "input": [{ "key": "f8" }] }),
        json!({
            "name": "save folder",
            // Enter on the closed Colour choice saves the dialog.
            "input": [{ "key": "ctrl-shift-n" }, { "text": "Lab" }, { "key": "tab" },
                      { "key": "down" }, { "key": "down" }, { "key": "enter" }, { "key": "enter" }],
        }),
    ];
    for server in servers {
        steps.extend(add_and_connect(server));
    }
    let script = dir.join("first.json");
    std::fs::write(&script, serde_json::to_string(&steps).unwrap()).unwrap();
    let lines = run_script(&script, Some(appdata));
    let folder = state(&lines, "save folder");
    assert_eq!(row(folder, "Lab")["kind"], "folder");
    assert_eq!(row(folder, "Lab")["colour"], "Red");
    lines
}

/// Checks that `name` connected, sits in the folder and wears the folder's colour.
fn assert_connected_in_the_red_folder(lines: &[Value], name: &str) {
    let connected = state(lines, &format!("connect {name}"));
    let row = row(connected, name);
    assert_eq!(row["status"], "connected", "{connected:#}");
    assert_eq!(row["depth"], 1, "{name} is in the folder");
    assert_eq!(row["colour"], "Red", "{name} inherits the folder's colour");
    assert_eq!(
        connected["messages"].as_array().unwrap().last().unwrap(),
        &json!(format!("Connected to {name}."))
    );
}

/// Plays a second run on the same connections file and Credential Manager: checks the
/// explorer shows `rows` once the folder is open, then connects the folder's first
/// connection with its saved password.
fn reopen_and_connect_the_first(dir: &Path, appdata: &Path, rows: &[&str]) -> Vec<Value> {
    let steps = json!([
        { "name": "after restart", "input": [{ "key": "f8" }, { "key": "right" }] },
        {
            "name": "connect from the saved password",
            "input": [{ "key": "down" }, { "key": "enter" }, "idle"],
        },
    ]);
    let script = dir.join("second.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(appdata));

    let reopened = state(&lines, "after restart");
    let names: Vec<&str> = reopened["explorer"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, rows);
    let connected = state(&lines, "connect from the saved password");
    assert_eq!(
        row(connected, rows[1])["status"],
        "connected",
        "{connected:#}"
    );
    lines
}

fn assert_no_password_shown(lines: &[Value], servers: &[Server]) {
    for line in lines {
        let text = line.to_string();
        for server in servers {
            assert!(
                !text.contains(server.password),
                "a state line shows a password"
            );
        }
    }
}

#[test]
fn ui_mssql_connections_in_a_coloured_folder_survive_a_restart_and_report_a_wrong_password() {
    let mssql = Mssql::start().expect("SQL Server test database");
    let servers = [
        Server {
            name: "Lab SQL Server",
            engine_keys: &[],
            host: mssql.host(),
            port: mssql.port(),
            user: mssql.user(),
            password: mssql.password(),
            database: mssql.database(),
        },
        // A SQL login the harness made for this test, never sa, with a wrong password.
        Server {
            name: "Wrong password",
            engine_keys: &[],
            host: mssql.host(),
            port: mssql.port(),
            user: mssql.user(),
            password: "not-the-password",
            database: mssql.database(),
        },
    ];
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);

    let first = save_in_a_red_folder_and_connect(&servers, dir.path(), &appdata);
    assert_connected_in_the_red_folder(&first, "Lab SQL Server");
    let refused = state(&first, "connect Wrong password");
    assert_eq!(row(refused, "Wrong password")["status"], "disconnected");
    assert_eq!(
        refused["messages"].as_array().unwrap().last().unwrap(),
        "Couldn't connect to Wrong password. Sign-in failed: the user name or password is wrong."
    );

    let second = reopen_and_connect_the_first(
        dir.path(),
        &appdata,
        &["Lab", "Lab SQL Server", "Wrong password"],
    );
    assert_no_password_shown(&first, &servers);
    assert_no_password_shown(&second, &servers);
}

#[test]
fn ui_pg_a_connection_in_a_coloured_folder_survives_a_restart() {
    let postgres = Postgres::start().expect("PostgreSQL test server");
    let servers = [Server {
        name: "Lab PostgreSQL",
        engine_keys: &["down", "down", "enter"],
        host: postgres.host(),
        port: postgres.port(),
        user: postgres.user(),
        password: postgres.password(),
        database: postgres.database(),
    }];
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);

    let first = save_in_a_red_folder_and_connect(&servers, dir.path(), &appdata);
    assert_connected_in_the_red_folder(&first, "Lab PostgreSQL");
    let second = reopen_and_connect_the_first(dir.path(), &appdata, &["Lab", "Lab PostgreSQL"]);
    assert_no_password_shown(&first, &servers);
    assert_no_password_shown(&second, &servers);
}

#[test]
fn ui_pg_a_test_result_clears_when_a_detail_changes() {
    let postgres = Postgres::start().expect("PostgreSQL test server");
    let server = Server {
        name: "Lab PostgreSQL",
        engine_keys: &["down", "down", "enter"],
        host: postgres.host(),
        port: postgres.port(),
        user: postgres.user(),
        password: postgres.password(),
        database: postgres.database(),
    };
    // The save step's keys stop on the Trust server certificate checkbox, then Enter saves.
    let mut keys = add_and_connect(&server)[0]["input"]
        .as_array()
        .unwrap()
        .clone();
    keys.pop();
    keys.extend([json!({ "key": "alt-t" }), json!("idle")]);
    let steps = json!([
        { "name": "explorer", "input": [{ "key": "f8" }] },
        { "name": "tested", "input": keys },
        { "name": "changed", "input": [{ "key": "space" }] },
    ]);
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(dir.path()));

    let tested = state(&lines, "tested");
    assert_eq!(
        tested["explorer"]["dialog"]["test"]["state"], "connected",
        "{tested:#}"
    );
    let changed = state(&lines, "changed");
    assert_eq!(
        changed["explorer"]["dialog"]["trust_server_certificate"],
        false
    );
    assert_eq!(
        changed["explorer"]["dialog"]["test"]["state"], "not_run",
        "{changed:#}"
    );
}

#[test]
fn ui_mysql_a_connection_in_a_coloured_folder_survives_a_restart() {
    let mysql = MySql::start().expect("MySQL test server");
    let servers = [Server {
        name: "Lab MySQL",
        engine_keys: &["down", "down", "down", "enter"],
        host: mysql.host(),
        port: mysql.port(),
        user: mysql.user(),
        password: mysql.password(),
        database: mysql.database(),
    }];
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);

    let first = save_in_a_red_folder_and_connect(&servers, dir.path(), &appdata);
    assert_connected_in_the_red_folder(&first, "Lab MySQL");
    let second = reopen_and_connect_the_first(dir.path(), &appdata, &["Lab", "Lab MySQL"]);
    assert_no_password_shown(&first, &servers);
    assert_no_password_shown(&second, &servers);
}

/// Sets or clears the read-only flag on `path`.
fn set_readonly(path: &Path, readonly: bool) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn ui_a_folder_that_cant_be_saved_keeps_its_dialog_open_and_adds_no_row() {
    let dir = tempfile::tempdir().expect("temp dir");
    let folder = dir.path().join("Database IDE");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("connections.toml");
    std::fs::write(&file, "").unwrap();
    // A read-only file stops the save on Windows, and a read-only folder on other systems.
    set_readonly(&file, true);
    set_readonly(&folder, true);
    let steps = json!([{
        "name": "save folder",
        "input": [{ "key": "f8" }, { "key": "ctrl-shift-n" }, { "text": "Lab" }, { "key": "enter" }],
    }]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(dir.path()));
    set_readonly(&folder, false);
    set_readonly(&file, false);

    let refused = state(&lines, "save folder");
    assert_eq!(refused["explorer"]["rows"], json!([]));
    let dialog = &refused["explorer"]["dialog"];
    assert_eq!(dialog["name"], "Lab", "{refused:#}");
    assert!(
        dialog["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("Couldn't save folder Lab: ")),
        "{refused:#}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
}

#[test]
fn ui_a_delete_that_cant_be_saved_keeps_the_connection_and_its_password() {
    let dir = tempfile::tempdir().expect("temp dir");
    let folder = dir.path().join("Database IDE");
    let file = folder.join("connections.toml");
    let mut saved = Connections::default();
    let mut details = Connection::new(Engine::PostgreSql, "db.example.test");
    details.name = "Kept".into();
    details.sign_in = SignInMethod::Password;
    details.user = Some("reader".into());
    let id = saved.add_connection(details, None).unwrap();
    saved.save(&file).unwrap();
    let _forget = ForgetPasswords(dir.path());
    let credentials = Credentials::new("database-ide-test").unwrap();
    let kept = saved.connections()[0].clone();
    credentials.store(&kept, "invented-password").unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    set_readonly(&file, true);
    set_readonly(&folder, true);
    let steps = json!([{
        "name": "delete Kept",
        "input": [{ "key": "f8" }, { "key": "delete" }, { "key": "enter" }],
    }]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(dir.path()));
    set_readonly(&folder, false);
    set_readonly(&file, false);

    let refused = state(&lines, "delete Kept");
    assert_eq!(row(refused, "Kept")["kind"], "connection");
    assert!(
        refused["messages"]
            .as_array()
            .unwrap()
            .last()
            .and_then(Value::as_str)
            .is_some_and(|m| m.starts_with("Couldn't delete: ")),
        "{refused:#}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), written);
    let password = credentials
        .read(&kept)
        .unwrap()
        .expect("the saved password");
    assert_eq!(password.expose(), "invented-password");
    assert_eq!(kept.id, id);
}

/// Each explorer row as (depth, name), top to bottom.
fn rows(state: &Value) -> Vec<(u64, String)> {
    state["explorer"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["depth"].as_u64().unwrap(),
                r["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn ui_folders_nest_move_rename_refuse_a_cycle_and_delete_after_asking() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scripts/explorer-folders.json");
    let lines = run_script(&script, Some(dir.path()));
    let cases: [(&str, &[(u64, &str)]); 8] = [
        ("make A", &[(0, "A")]),
        ("make B in A", &[(0, "A"), (1, "B")]),
        ("move B to the top level", &[(0, "A"), (0, "B")]),
        ("rename B", &[(0, "A"), (0, "Archive")]),
        ("move A into Archive", &[(0, "Archive"), (1, "A")]),
        ("move Archive into A", &[(0, "Archive"), (1, "A")]),
        ("ask before deleting Archive", &[(0, "Archive"), (1, "A")]),
        ("delete Archive", &[]),
    ];
    for (step, expected) in cases {
        let expected: Vec<(u64, String)> = expected
            .iter()
            .map(|(d, n)| (*d, (*n).to_owned()))
            .collect();
        let seen = state(&lines, step);
        assert_eq!(rows(seen), expected, "rows after {step:?}: {seen:#}");
    }
    let refused = state(&lines, "move Archive into A");
    assert_eq!(
        refused["messages"].as_array().unwrap().last().unwrap(),
        "A folder can't move into itself or a folder inside it."
    );
    let asking = state(&lines, "ask before deleting Archive");
    assert_eq!(asking["explorer"]["dialog"]["dialog"], "delete");
    assert_eq!(asking["explorer"]["dialog"]["name"], "Archive");
}
