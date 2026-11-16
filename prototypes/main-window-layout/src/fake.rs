//! PROTOTYPE. The fake content every layout variant shows: two connections in two connection
//! folders, an object explorer tree, three query tabs and the messages of the one tab that ran.
//! All names are invented.

use gpui::{Hsla, SharedString};
use gpui_component::tree::TreeItem;

/// A connection folder with its connection colour.
#[derive(Debug)]
pub struct Folder {
    pub name: &'static str,
    pub colour: Hsla,
}

/// A saved connection.
#[derive(Debug)]
pub struct Connection {
    pub name: &'static str,
    pub folder: usize,
    pub engine: &'static str,
    pub user: &'static str,
    pub danger_check: bool,
    pub databases: &'static [Database],
}

#[derive(Debug)]
pub struct Database {
    pub name: &'static str,
    pub tables: &'static [Table],
    pub views: &'static [&'static str],
    pub procedures: &'static [&'static str],
}

/// A table with its columns as (name, type, note).
#[derive(Debug)]
pub struct Table {
    pub name: &'static str,
    pub columns: &'static [(&'static str, &'static str, &'static str)],
    pub keys: &'static [&'static str],
    pub indexes: &'static [&'static str],
    pub rows: &'static str,
}

pub const FOLDERS: [Folder; 2] = [
    Folder { name: "Prod", colour: Hsla { h: 0.0, s: 0.72, l: 0.45, a: 1. } },
    Folder { name: "Dev", colour: Hsla { h: 0.36, s: 0.62, l: 0.36, a: 1. } },
];

const ORDERS: Table = Table {
    name: "dbo.Orders",
    columns: &[
        ("order_id", "bigint", "PK, not null"),
        ("customer_id", "int", "FK, not null"),
        ("created_at", "datetime2(3)", "not null"),
        ("status", "varchar(20)", "not null"),
        ("amount", "decimal(12,2)", "not null"),
        ("currency", "char(3)", "not null"),
        ("region_id", "int", "FK, null"),
        ("note", "nvarchar(400)", "null"),
    ],
    keys: &["PK_Orders", "FK_Orders_Customers", "FK_Orders_Regions"],
    indexes: &["PK_Orders (clustered)", "IX_Orders_created_at", "IX_Orders_status"],
    rows: "1,000,000",
};

const CUSTOMERS: Table = Table {
    name: "dbo.Customers",
    columns: &[
        ("customer_id", "int", "PK, not null"),
        ("name", "nvarchar(200)", "not null"),
        ("email", "varchar(320)", "null"),
        ("region_id", "int", "FK, not null"),
    ],
    keys: &["PK_Customers", "FK_Customers_Regions"],
    indexes: &["PK_Customers (clustered)", "IX_Customers_email"],
    rows: "250,000",
};

const REGIONS: Table = Table {
    name: "dbo.Regions",
    columns: &[("region_id", "int", "PK, not null"), ("name", "nvarchar(80)", "not null")],
    keys: &["PK_Regions"],
    indexes: &["PK_Regions (clustered)"],
    rows: "5",
};

const STOCK: Table = Table {
    name: "dbo.StockLevels",
    columns: &[
        ("sku", "varchar(16)", "PK, not null"),
        ("warehouse_id", "int", "PK, not null"),
        ("on_hand", "int", "not null"),
    ],
    keys: &["PK_StockLevels"],
    indexes: &["PK_StockLevels (clustered)"],
    rows: "84,120",
};

const USERS: Table = Table {
    name: "public.users",
    columns: &[
        ("id", "bigint", "PK, not null"),
        ("email", "text", "not null"),
        ("last_login", "timestamptz", "null"),
    ],
    keys: &["users_pkey"],
    indexes: &["users_pkey", "users_email_key"],
    rows: "3,410",
};

pub const CONNECTIONS: [Connection; 2] = [
    Connection {
        name: "PROD-SQL01",
        folder: 0,
        engine: "SQL Server 2022",
        user: "dba_ops",
        danger_check: true,
        databases: &[
            Database {
                name: "Sales",
                tables: &[ORDERS, CUSTOMERS, REGIONS],
                views: &["dbo.vw_OpenOrders", "dbo.vw_MonthlyTotals"],
                procedures: &["dbo.usp_CloseMonth", "dbo.usp_ArchiveOrders"],
            },
            Database {
                name: "Inventory",
                tables: &[STOCK],
                views: &["dbo.vw_LowStock"],
                procedures: &["dbo.usp_Restock"],
            },
        ],
    },
    Connection {
        name: "dev-pg",
        folder: 1,
        engine: "PostgreSQL 17",
        user: "dev_user",
        danger_check: false,
        databases: &[Database {
            name: "app_dev",
            tables: &[USERS],
            views: &["public.active_users"],
            procedures: &["public.purge_sessions()"],
        }],
    },
];

/// A query tab: one connection, one current database and the SQL in its editor.
#[derive(Debug)]
pub struct TabSpec {
    pub title: &'static str,
    pub connection: usize,
    pub database: &'static str,
    pub sql: &'static str,
    /// Whether the tab's last run left results, messages and a plan.
    pub ran: bool,
}

pub const TABS: [TabSpec; 3] = [
    TabSpec {
        title: "month-end orders.sql",
        connection: 0,
        database: "Sales",
        sql: "-- Month-end check for October
SELECT o.order_id, c.name AS customer, o.created_at, o.status, o.amount,
       o.currency, r.name AS region, w.code AS warehouse, l.sku, o.note
FROM dbo.Orders AS o
JOIN dbo.Customers AS c ON c.customer_id = o.customer_id
JOIN dbo.Regions AS r ON r.region_id = c.region_id
ORDER BY o.created_at DESC;

PRINT 'Orders checked for 2026-10';

UPDATE dbo.Orders
SET status = 'closed'
WHERE status = 'shipped' AND created_at < '2026-10-01';

SELECT COUNT(*) FROM dbo.OrderAudit WHERE audit_month = '2026-10';
",
        ran: true,
    },
    TabSpec {
        title: "low stock.sql",
        connection: 0,
        database: "Inventory",
        sql: "SELECT s.sku, s.warehouse_id, s.on_hand
FROM dbo.StockLevels AS s
WHERE s.on_hand < 10
ORDER BY s.on_hand;
",
        ran: false,
    },
    TabSpec {
        title: "stale users.sql",
        connection: 1,
        database: "app_dev",
        sql: "SELECT id, email, last_login
FROM public.users
WHERE last_login < now() - interval '90 days'
ORDER BY last_login;
",
        ran: false,
    },
];

/// The kind of a line in the messages view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MessageKind {
    Rows,
    Print,
    Error,
    Info,
}

/// The messages of the first tab's run: (kind, text).
pub const MESSAGES: [(MessageKind, &str); 6] = [
    (MessageKind::Rows, "(1,000,000 rows affected)"),
    (MessageKind::Print, "Orders checked for 2026-10"),
    (MessageKind::Rows, "(4,212 rows affected)"),
    (MessageKind::Error, "Msg 208, Level 16, State 1, Line 15"),
    (MessageKind::Error, "Invalid object name 'dbo.OrderAudit'."),
    (MessageKind::Info, "Completion time: 2026-11-16T10:42:07.418"),
];

pub const ELAPSED: &str = "00:00:02.418";
pub const ROW_COUNT: &str = "1,000,000 rows";

/// Builds the object explorer tree: folders, connections, databases, object kinds, objects,
/// and columns, keys and indexes under each table. Ids are slash-separated paths so the
/// inspector can look a selected item up with [`describe`].
pub fn explorer_items() -> Vec<TreeItem> {
    FOLDERS
        .iter()
        .enumerate()
        .map(|(fi, folder)| {
            let conns = CONNECTIONS.iter().enumerate().filter(|(_, c)| c.folder == fi).map(
                |(ci, conn)| {
                    let cid = format!("c{ci}");
                    let dbs = conn.databases.iter().enumerate().map(|(di, db)| {
                        let did = format!("{cid}/d{di}");
                        let tables = db.tables.iter().enumerate().map(|(ti, t)| {
                            let tid = format!("{did}/t{ti}");
                            TreeItem::new(tid.clone(), t.name)
                                .expanded(ci == 0 && di == 0 && ti == 0)
                                .child(
                                    TreeItem::new(format!("{tid}/cols"), "Columns")
                                        .expanded(ci == 0 && di == 0 && ti == 0)
                                        .children(t.columns.iter().enumerate().map(
                                            |(i, (n, ty, note))| {
                                                TreeItem::new(
                                                    format!("{tid}/col{i}"),
                                                    format!("{n}  ({ty}, {note})"),
                                                )
                                            },
                                        )),
                                )
                                .child(
                                    TreeItem::new(format!("{tid}/keys"), "Keys")
                                        .expanded(ci == 0 && di == 0 && ti == 0)
                                        .children(t.keys.iter().enumerate().map(|(i, k)| {
                                            TreeItem::new(format!("{tid}/key{i}"), *k)
                                        })),
                                )
                                .child(TreeItem::new(format!("{tid}/ix"), "Indexes").children(
                                    t.indexes.iter().enumerate().map(|(i, k)| {
                                        TreeItem::new(format!("{tid}/ix{i}"), *k)
                                    }),
                                ))
                        });
                        let leaf_group = |id: &str, label: &str, names: &[&'static str]| {
                            TreeItem::new(format!("{did}/{id}"), label.to_string())
                                .expanded(ci == 0 && di == 0)
                                .children(names.iter().enumerate().map(|(i, n)| {
                                    TreeItem::new(format!("{did}/{id}{i}"), *n)
                                }))
                        };
                        TreeItem::new(did.clone(), db.name)
                            .expanded(ci == 0 && di == 0 || ci == 1)
                            .child(
                                TreeItem::new(format!("{did}/tables"), "Tables")
                                    .expanded(ci == 0 && di == 0)
                                    .children(tables),
                            )
                            .child(leaf_group("v", "Views", db.views))
                            .child(leaf_group("p", "Procedures", db.procedures))
                    });
                    TreeItem::new(cid.clone(), conn.name).expanded(true).children(dbs)
                },
            );
            TreeItem::new(format!("f{fi}"), folder.name).expanded(true).children(conns)
        })
        .collect()
}

/// The kind of explorer item an id names, for the icon and the colour of its label.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemKind {
    Folder(usize),
    Connection(usize),
    Database,
    Group,
    Table,
    Other,
}

pub fn item_kind(id: &str) -> ItemKind {
    let parts: Vec<&str> = id.split('/').collect();
    let num = |s: &str, prefix: &str| s.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok());
    match parts.as_slice() {
        [f] if f.starts_with('f') => ItemKind::Folder(num(f, "f").unwrap_or(0)),
        [c] => ItemKind::Connection(num(c, "c").unwrap_or(0)),
        [_, _] => ItemKind::Database,
        [_, _, g] if ["tables", "v", "p"].contains(g) => ItemKind::Group,
        [_, _, t] if t.starts_with('t') => ItemKind::Table,
        [_, _, _, g] if ["cols", "keys", "ix"].contains(g) => ItemKind::Group,
        _ => ItemKind::Other,
    }
}

/// The table an explorer id points into, if any: the table itself or anything under it.
pub fn table_of(id: &str) -> Option<&'static Table> {
    let parts: Vec<&str> = id.split('/').collect();
    let ci: usize = parts.first()?.strip_prefix('c')?.parse().ok()?;
    let di: usize = parts.get(1)?.strip_prefix('d')?.parse().ok()?;
    let ti: usize = parts.get(2)?.strip_prefix('t')?.parse().ok()?;
    CONNECTIONS.get(ci)?.databases.get(di)?.tables.get(ti)
}

/// Title and (label, value) rows describing an explorer item, for an inspector.
pub fn describe(id: &str, label: &str) -> (SharedString, Vec<(SharedString, SharedString)>) {
    if let Some(t) = table_of(id) {
        let mut rows: Vec<(SharedString, SharedString)> =
            vec![("Rows".into(), t.rows.into()), ("Columns".into(), "".into())];
        rows.extend(
            t.columns.iter().map(|(n, ty, note)| ((*n).into(), format!("{ty}  {note}").into())),
        );
        rows.push(("Keys".into(), t.keys.join(", ").into()));
        rows.push(("Indexes".into(), t.indexes.join(", ").into()));
        return (format!("Table {}", t.name).into(), rows);
    }
    match item_kind(id) {
        ItemKind::Connection(ci) => {
            let c = &CONNECTIONS[ci];
            (
                format!("Connection {}", c.name).into(),
                vec![
                    ("Engine".into(), c.engine.into()),
                    ("Folder".into(), FOLDERS[c.folder].name.into()),
                    ("User".into(), c.user.into()),
                    ("Danger check".into(), if c.danger_check { "On" } else { "Off" }.into()),
                ],
            )
        }
        _ => (label.to_string().into(), vec![("Kind".into(), "Explorer item".into())]),
    }
}
