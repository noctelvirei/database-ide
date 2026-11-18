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

#[test]
fn ui_connections_in_a_coloured_folder_connect_survive_a_restart_and_report_a_wrong_password() {
    let mssql = Mssql::start().expect("SQL Server test database");
    let postgres = Postgres::start().expect("PostgreSQL test server");
    let mysql = MySql::start().expect("MySQL test server");
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
        Server {
            name: "Lab PostgreSQL",
            engine_keys: &["down", "down", "enter"],
            host: postgres.host(),
            port: postgres.port(),
            user: postgres.user(),
            password: postgres.password(),
            database: postgres.database(),
        },
        Server {
            name: "Lab MySQL",
            engine_keys: &["down", "down", "down", "enter"],
            host: mysql.host(),
            port: mysql.port(),
            user: mysql.user(),
            password: mysql.password(),
            database: mysql.database(),
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

    // First run: a red folder, a connection per server in it, and a connect to each.
    let mut steps = vec![
        json!({ "name": "explorer", "input": [{ "key": "f8" }] }),
        json!({
            "name": "save folder",
            // Enter on the closed Colour choice saves the dialog.
            "input": [{ "key": "ctrl-shift-n" }, { "text": "Lab" }, { "key": "tab" },
                      { "key": "down" }, { "key": "down" }, { "key": "enter" }, { "key": "enter" }],
        }),
    ];
    for server in &servers {
        steps.extend(add_and_connect(server));
    }
    let script = dir.path().join("first.json");
    std::fs::write(&script, serde_json::to_string(&steps).unwrap()).unwrap();
    let first = run_script(&script, Some(&appdata));

    let folder = state(&first, "save folder");
    assert_eq!(row(folder, "Lab")["kind"], "folder");
    assert_eq!(row(folder, "Lab")["colour"], "Red");
    for name in ["Lab SQL Server", "Lab PostgreSQL", "Lab MySQL"] {
        let connected = state(&first, &format!("connect {name}"));
        let row = row(connected, name);
        assert_eq!(row["status"], "connected", "{connected:#}");
        assert_eq!(row["depth"], 1, "{name} is in the folder");
        assert_eq!(row["colour"], "Red", "{name} inherits the folder's colour");
        let messages = connected["messages"].as_array().unwrap();
        assert_eq!(
            messages.last().unwrap(),
            &json!(format!("Connected to {name}."))
        );
    }
    let refused = state(&first, "connect Wrong password");
    assert_eq!(row(refused, "Wrong password")["status"], "disconnected");
    assert_eq!(
        refused["messages"].as_array().unwrap().last().unwrap(),
        "Couldn't connect to Wrong password. Sign-in failed: the user name or password is wrong."
    );

    // Second run: the same file and Credential Manager, so the saved password connects.
    let steps = json!([
        { "name": "after restart", "input": [{ "key": "f8" }, { "key": "right" }] },
        {
            "name": "connect from the saved password",
            "input": [{ "key": "down" }, { "key": "enter" }, "idle"],
        },
    ]);
    let script = dir.path().join("second.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let second = run_script(&script, Some(&appdata));

    let reopened = state(&second, "after restart");
    let names: Vec<&str> = reopened["explorer"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Lab",
            "Lab SQL Server",
            "Lab PostgreSQL",
            "Lab MySQL",
            "Wrong password"
        ]
    );
    let connected = state(&second, "connect from the saved password");
    assert_eq!(row(connected, "Lab SQL Server")["status"], "connected");

    for line in first.iter().chain(&second) {
        let text = line.to_string();
        for server in &servers {
            assert!(
                !text.contains(server.password),
                "a state line shows a password"
            );
        }
    }
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
        assert_eq!(rows(state(&lines, step)), expected, "rows after {step:?}");
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
