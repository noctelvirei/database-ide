//! Plans as a user drives them against the real SQL Server, PostgreSQL and MySQL servers from
//! `dbide-test-servers` (CODE_STANDARDS.md, "UI E2E harness"): the estimated plan of a join
//! (Ctrl+L), its actual plan with the run (Ctrl+M, F5), the costliest operator's details in
//! the inspector, the plan as a graph (F9) walked with the arrow keys, and on SQL Server the
//! plan saved as a `.sqlplan` file and opened again.

mod lab;
mod passwords;
mod support;

use dbide_engine::danger::Engine;
use dbide_test_servers::mssql::Mssql;
use dbide_test_servers::mysql::MySql;
use dbide_test_servers::postgres::Postgres;
use lab::Server;
use passwords::ForgetPasswords;
use serde_json::{Value, json};
use support::{run_script, state};

/// The two tables the join reads; `crates` holds four crates of each fruit.
const TABLES: [&str; 2] = [
    "CREATE TABLE fruit (id INT PRIMARY KEY, name VARCHAR(40) NOT NULL)",
    "CREATE TABLE crates (id INT PRIMARY KEY, fruit_id INT NOT NULL)",
];

/// The query whose plans the script shows.
const JOIN: &str =
    "SELECT f.name, c.id FROM fruit f JOIN crates c ON c.fruit_id = f.id WHERE f.id < 50";

/// The test's plans, operators and the operator names a join can show as.
fn plan(state: &Value) -> &Value {
    let plans = state["plans"].as_array().expect("plans");
    assert_eq!(plans.len(), 1, "one statement, one plan: {state:#}");
    &plans[0]
}

fn operators(plan: &Value) -> &Vec<Value> {
    plan["operators"].as_array().expect("operators")
}

/// Checks the plan shows the join reading both tables.
fn assert_join(plan: &Value) {
    let ops = operators(plan);
    assert!(
        ops.iter().any(|op| {
            // MySQL writes "Nested loop inner join"; the others capitalise "Loop".
            let name = op["name"].as_str().unwrap().to_lowercase();
            name.contains("nested loop") || name.contains("hash") || name.contains("merge join")
        }),
        "no join operator: {plan:#}"
    );
    for table in ["fruit", "crates"] {
        assert!(
            ops.iter()
                .any(|op| op["object"].as_str().is_some_and(|o| o.contains(table))),
            "{table} isn't read: {plan:#}"
        );
    }
}

/// Plays the plan script on `server`. `to_plan` are the keys that move from the results
/// pane's first section to the plan; `save` names a `.sqlplan` file to save and reopen.
fn show_plans(server: &Server, to_plan: &[Value], save: Option<&std::path::Path>) -> Vec<Value> {
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);

    let mut focus_plan = vec![json!({ "key": "f6" }), json!({ "key": "f6" })];
    focus_plan.extend(to_plan.iter().cloned());
    let mut steps = vec![
        json!({ "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] }),
        json!({
            "name": "estimated plan",
            "input": [{ "text": JOIN }, { "key": "ctrl-l" }, "idle"],
        }),
        json!({ "name": "include actual plan", "input": [{ "key": "ctrl-m" }] }),
        json!({ "name": "run with the actual plan", "input": [{ "key": "f5" }, "idle"] }),
        json!({ "name": "focus the plan", "input": focus_plan }),
        json!({ "name": "first operator", "input": [{ "key": "home" }] }),
        json!({ "name": "next operator", "input": [{ "key": "down" }] }),
        json!({ "name": "graph", "input": [{ "key": "f9" }, { "shot": "graph" }] }),
        json!({ "name": "to the parent", "input": [{ "key": "left" }] }),
        json!({ "name": "to the first input", "input": [{ "key": "right" }, { "shot": "input" }] }),
        json!({ "name": "back to the tree", "input": [{ "key": "f9" }] }),
    ];
    if let Some(file) = save {
        steps.push(json!({
            "name": "save the plan",
            "input": [{ "path": file }, { "key": "ctrl-s" }, "idle"],
        }));
        steps.push(json!({
            "name": "open the plan",
            "input": [{ "path": file }, { "key": "ctrl-o" }, "idle"],
        }));
    }
    let script = dir.path().join("script.json");
    std::fs::write(&script, Value::Array(steps).to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let estimated = state(&lines, "estimated plan");
    let estimated_plan = plan(estimated);
    assert_eq!(estimated_plan["actual"], false, "{estimated:#}");
    assert_join(estimated_plan);
    assert!(
        operators(estimated_plan)
            .iter()
            .all(|op| op["actual_rows"].is_null()),
        "an estimated plan has no actual rows: {estimated:#}"
    );

    let toggled = state(&lines, "include actual plan");
    let tab = &toggled["tabs"][toggled["active_tab"].as_u64().unwrap() as usize];
    assert_eq!(tab["actual_plan"], true, "{toggled:#}");

    let actual = state(&lines, "run with the actual plan");
    let actual_plan = plan(actual);
    assert_eq!(actual_plan["actual"], true, "{actual:#}");
    assert_join(actual_plan);
    // The join returns the 49 fruits below 50 with their four crates each.
    assert!(
        operators(actual_plan)
            .iter()
            .any(|op| op["actual_rows"] == json!(196.0)),
        "{actual:#}"
    );

    // The plan opens on its costliest operator, its details in the inspector.
    let focused = state(&lines, "focus the plan");
    let shown = plan(focused);
    assert_eq!(shown["focused"], true, "{focused:#}");
    let ops = operators(shown);
    let selected = &ops[shown["selected"].as_u64().unwrap() as usize];
    assert_eq!(selected["costliest"], true, "{focused:#}");
    let top = ops
        .iter()
        .map(|op| op["cost_percent"].as_f64().unwrap())
        .fold(0.0, f64::max);
    assert_eq!(selected["cost_percent"].as_f64().unwrap(), top);
    assert_eq!(focused["inspector"]["operator"], selected["name"]);
    assert!(
        focused["inspector"]["details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "Actual rows"),
        "{focused:#}"
    );

    // Home and Down move the selection, and the inspector follows it.
    let first = state(&lines, "first operator");
    assert_eq!(plan(first)["selected"], 0);
    assert_eq!(first["inspector"]["operator"], ops[0]["name"]);
    let next = state(&lines, "next operator");
    assert_eq!(plan(next)["selected"], 1);
    assert_eq!(next["inspector"]["operator"], ops[1]["name"]);

    // F9 shows the graph with the selection kept; Left goes to the operator the selected one
    // feeds, the statement's root, and Right back to its first input. F9 again keeps it too.
    let graph = state(&lines, "graph");
    assert_eq!(plan(graph)["graph"], true, "{graph:#}");
    assert_eq!(plan(graph)["selected"], 1);
    assert_eq!(graph["inspector"]["operator"], ops[1]["name"]);
    let parent = state(&lines, "to the parent");
    assert_eq!(plan(parent)["selected"], 0, "{parent:#}");
    assert_eq!(parent["inspector"]["operator"], ops[0]["name"]);
    let input = state(&lines, "to the first input");
    assert_eq!(plan(input)["selected"], 1, "{input:#}");
    assert_eq!(input["inspector"]["operator"], ops[1]["name"]);
    let tree = state(&lines, "back to the tree");
    assert_eq!(plan(tree)["graph"], false, "{tree:#}");
    assert_eq!(plan(tree)["selected"], 1);
    lines
}

#[test]
fn ui_mssql_plans_show_estimated_and_actual_and_round_trip_a_sqlplan() {
    let db = Mssql::start().expect("SQL Server test database");
    for table in TABLES {
        db.query(table).unwrap();
    }
    db.query(
        "INSERT INTO fruit SELECT TOP 200 ROW_NUMBER() OVER (ORDER BY (SELECT NULL)), 'apple' \
         FROM sys.all_objects",
    )
    .unwrap();
    db.query(
        "INSERT INTO crates SELECT TOP 800 ROW_NUMBER() OVER (ORDER BY (SELECT NULL)), 0 \
         FROM sys.all_objects a CROSS JOIN sys.all_objects b; \
         UPDATE crates SET fruit_id = (id - 1) % 200 + 1",
    )
    .unwrap();
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("join.sqlplan");
    let lines = show_plans(
        &Server {
            engine: Engine::SqlServer,
            host: db.host(),
            port: db.port(),
            user: db.user(),
            password: db.password(),
            database: db.database(),
        },
        // SQL Server's actual plan comes with the query's rows, whose grid is first.
        &[json!({ "key": "ctrl-down" })],
        Some(&file),
    );

    let ran = state(&lines, "run with the actual plan");
    assert_eq!(ran["results"].as_array().unwrap().len(), 1, "{ran:#}");
    assert_eq!(ran["results"][0]["rows"], 196);

    let saved = state(&lines, "save the plan");
    let bytes = std::fs::read(&file).expect("the saved plan");
    assert_eq!(
        &bytes[..2],
        &[0xff, 0xfe],
        "a .sqlplan is UTF-16 with a byte order mark"
    );
    let messages = saved["messages"].as_array().unwrap();
    assert_eq!(
        messages.last().unwrap(),
        &json!(format!("Saved the plan to {}.", file.display())),
        "{saved:#}"
    );

    // The file opens as the same plan, with its actual rows.
    let opened = state(&lines, "open the plan");
    let before = plan(state(&lines, "run with the actual plan"));
    let reopened = plan(opened);
    assert_eq!(reopened["actual"], true, "{opened:#}");
    let names = |plan: &Value| -> Vec<Value> {
        operators(plan)
            .iter()
            .map(|op| op["name"].clone())
            .collect()
    };
    assert_eq!(names(reopened), names(before));
    assert_eq!(opened["results"].as_array().unwrap().len(), 0, "{opened:#}");
}

/// Asserts the run with the actual plan showed no grid and said its changes were rolled back.
fn assert_rolled_back(lines: &[Value]) {
    let ran = state(lines, "run with the actual plan");
    assert_eq!(ran["results"].as_array().unwrap().len(), 0, "{ran:#}");
    assert!(
        ran["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.as_str().unwrap().contains("rolled back")),
        "{ran:#}"
    );
}

#[test]
fn ui_pg_graph_scrolls_to_the_selected_operator() {
    let db = Postgres::start().expect("PostgreSQL test server");
    let server = Server {
        engine: Engine::PostgreSql,
        host: db.host(),
        port: db.port(),
        user: db.user(),
        password: db.password(),
        database: db.database(),
    };
    for n in 1..=6 {
        db.query(&format!("CREATE TABLE t{n} (id INT PRIMARY KEY)"))
            .unwrap();
    }
    // Six tables joined in a chain: a plan wider and taller than the results pane.
    let chain = "SELECT * FROM t1 JOIN t2 ON t2.id = t1.id JOIN t3 ON t3.id = t2.id \
                 JOIN t4 ON t4.id = t3.id JOIN t5 ON t5.id = t4.id JOIN t6 ON t6.id = t5.id";
    let dir = tempfile::tempdir().expect("temp dir");
    let appdata = dir.path().join("appdata");
    std::fs::create_dir(&appdata).expect("appdata folder");
    let _forget = ForgetPasswords(&appdata);
    server.save_as_lab(&appdata);
    let steps = json!([
        { "name": "open a tab", "input": [{ "key": "f8" }, { "key": "ctrl-n" }, "idle"] },
        { "name": "plan", "input": [{ "text": chain }, { "key": "ctrl-l" }, "idle"] },
        // The last operator is chosen while the tree shows, before the graph was ever drawn.
        { "name": "last operator", "input": [{ "key": "f6" }, { "key": "f6" }, { "key": "end" }] },
        { "name": "graph", "input": [{ "key": "f9" }, "idle", { "shot": "last" }] },
        { "name": "first operator", "input": [{ "key": "home" }, "idle", { "shot": "first" }] },
    ]);
    let script = dir.path().join("script.json");
    std::fs::write(&script, steps.to_string()).unwrap();
    let lines = run_script(&script, Some(&appdata));

    let last = state(&lines, "graph");
    let shown = plan(last);
    assert_eq!(shown["graph"], true, "{last:#}");
    assert_eq!(
        shown["selected"].as_u64().unwrap() as usize,
        operators(shown).len() - 1,
        "{last:#}"
    );
    assert_eq!(shown["selected_in_view"], true, "{last:#}");
    let first = state(&lines, "first operator");
    assert_eq!(plan(first)["selected"], 0);
    assert_eq!(plan(first)["selected_in_view"], true, "{first:#}");
}

#[test]
fn ui_pg_plans_show_estimated_and_actual_and_roll_back() {
    let db = Postgres::start().expect("PostgreSQL test server");
    for table in TABLES {
        db.query(table).unwrap();
    }
    db.query("INSERT INTO fruit SELECT g, 'apple' FROM generate_series(1, 200) g")
        .unwrap();
    db.query("INSERT INTO crates SELECT g, (g - 1) % 200 + 1 FROM generate_series(1, 800) g")
        .unwrap();
    db.query("ANALYZE").unwrap();
    let lines = show_plans(
        &Server {
            engine: Engine::PostgreSql,
            host: db.host(),
            port: db.port(),
            user: db.user(),
            password: db.password(),
            database: db.database(),
        },
        &[],
        None,
    );
    assert_rolled_back(&lines);
}

#[test]
fn ui_mysql_plans_show_estimated_and_actual_and_roll_back() {
    let db = MySql::start().expect("MySQL test server");
    for table in TABLES {
        db.query(table).unwrap();
    }
    db.query(
        "INSERT INTO fruit WITH RECURSIVE g (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM g \
         WHERE n < 200) SELECT n, 'apple' FROM g",
    )
    .unwrap();
    db.query(
        "INSERT INTO crates WITH RECURSIVE g (n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM g \
         WHERE n < 800) SELECT n, (n - 1) % 200 + 1 FROM g",
    )
    .unwrap();
    db.query("ANALYZE TABLE fruit, crates").unwrap();
    let lines = show_plans(
        &Server {
            engine: Engine::MySql,
            host: db.host(),
            port: db.port(),
            user: db.user(),
            password: db.password(),
            database: db.database(),
        },
        &[],
        None,
    );
    assert_rolled_back(&lines);
}
