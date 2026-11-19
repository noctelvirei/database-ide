use dbide_engine::completion;
use dbide_engine::danger::Engine;
use rand::{Rng, SeedableRng, rngs::StdRng};

#[test]
fn caret_before_from_still_finds_the_alias_and_replacement() {
    let context = completion::context("SELECT o.na FROM orders o", 11, Engine::PostgreSql);
    assert_eq!(context.fragment.range, 9..11);
    assert_eq!(context.fragment.text, "na");
    assert_eq!(context.qualifier, ["o"]);
    assert_eq!(
        context.tables,
        [completion::Table {
            schema: None,
            name: "orders".into(),
            alias: Some("o".into()),
            is_cte: false,
        }]
    );
    assert!(context.suggest);
}

type Source<'a> = (&'a str, &'a str, &'a str, bool);
type Case<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a [&'a str],
    &'a [Source<'a>],
    bool,
);

fn check_cases(engine: Engine, cases: &[Case<'_>]) {
    for &(marked, replacement, prefix, qualifier, sources, suggest) in cases {
        let offset = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let actual = completion::context(&text, offset, engine);
        assert_eq!(
            &text[actual.fragment.range.clone()],
            replacement,
            "{engine:?}: {marked}"
        );
        assert_eq!(actual.fragment.text, prefix, "{engine:?}: {marked}");
        assert_eq!(actual.qualifier, qualifier, "{engine:?}: {marked}");
        let tables: Vec<_> = actual
            .tables
            .iter()
            .map(|table| {
                (
                    table.schema.as_deref().unwrap_or(""),
                    table.name.as_str(),
                    table.alias.as_deref().unwrap_or(""),
                    table.is_cte,
                )
            })
            .collect();
        assert_eq!(tables, sources, "{engine:?}: {marked}");
        assert_eq!(actual.suggest, suggest, "{engine:?}: {marked}");
    }
}

#[test]
fn each_engine_recovers_fragments_and_statement_sources() {
    let cases: &[Case<'_>] = &[
        ("SELECT na|", "na", "na", &[], &[], true),
        ("SELECT na|me", "name", "na", &[], &[], true),
        (
            "SELECT o.| FROM orders o",
            "",
            "",
            &["o"],
            &[("", "orders", "o", false)],
            true,
        ),
        ("SELECT dbo.Ord|", "Ord", "Ord", &["dbo"], &[], true),
        (
            "SELECT sales.dbo.Ord|",
            "Ord",
            "Ord",
            &["sales", "dbo"],
            &[],
            true,
        ),
        (
            "SELECT | FROM sales.orders AS o, customers c",
            "",
            "",
            &[],
            &[
                ("sales", "orders", "o", false),
                ("", "customers", "c", false),
            ],
            true,
        ),
        (
            "SELECT | FROM orders o JOIN customers AS c ON c.id=o.id",
            "",
            "",
            &[],
            &[("", "orders", "o", false), ("", "customers", "c", false)],
            true,
        ),
        (
            "SELECT | FROM orders o JOIN customers c ON c.id=o.id, invoices i",
            "",
            "",
            &[],
            &[
                ("", "orders", "o", false),
                ("", "customers", "c", false),
                ("", "invoices", "i", false),
            ],
            true,
        ),
        (
            "UPDATE sales.orders o SET na|",
            "na",
            "na",
            &[],
            &[("sales", "orders", "o", false)],
            true,
        ),
        (
            "DELETE FROM orders WHERE na|",
            "na",
            "na",
            &[],
            &[("", "orders", "", false)],
            true,
        ),
        (
            "INSERT INTO orders (na|",
            "na",
            "na",
            &[],
            &[("", "orders", "", false)],
            true,
        ),
        (
            "MERGE INTO orders o USING staging s ON o.id=s.id |",
            "",
            "",
            &[],
            &[("", "orders", "o", false), ("", "staging", "s", false)],
            true,
        ),
        (
            "WITH c AS (SELECT * FROM orders) SELECT c.| FROM c",
            "",
            "",
            &["c"],
            &[("", "c", "", true)],
            true,
        ),
        (
            "WITH c AS (SELECT 1), d AS (SELECT 2) SELECT | FROM c",
            "",
            "",
            &[],
            &[("", "c", "", true), ("", "d", "", true)],
            true,
        ),
        (
            "SELECT * FROM customers; SELECT o.| FROM orders o",
            "",
            "",
            &["o"],
            &[("", "orders", "o", false)],
            true,
        ),
        (
            "SELECT o.| FROM orders o; SELECT * FROM customers",
            "",
            "",
            &["o"],
            &[("", "orders", "o", false)],
            true,
        ),
        ("SELECT 'a|b'", "", "", &[], &[], false),
        ("SELECT 'open|", "", "", &[], &[], false),
        ("SELECT 1 -- co|mment", "", "", &[], &[], false),
        ("SELECT 1 -- comment|", "", "", &[], &[], false),
        ("SELECT /* co|mment */ 1", "", "", &[], &[], false),
        ("SELECT /* open|", "", "", &[], &[], false),
        ("SELECT 'closed'|", "", "", &[], &[], true),
        ("SELECT /* closed */ |", "", "", &[], &[], true),
        (
            "SELECT | FROM /* source */ orders -- alias\n o",
            "",
            "",
            &[],
            &[("", "orders", "o", false)],
            true,
        ),
        (
            "SELECT | FROM orders WHERE id IN (SELECT id FROM customers)",
            "",
            "",
            &[],
            &[("", "orders", "", false)],
            true,
        ),
        ("SELECT | FROM", "", "", &[], &[], true),
        ("SELECT | FROM (", "", "", &[], &[], true),
        (
            "MERGE INTO orders o USING staging s ON o.id=s.id WHEN MATCHED THEN UPDATE SET id=1 |",
            "",
            "",
            &[],
            &[("", "orders", "o", false), ("", "staging", "s", false)],
            true,
        ),
    ];
    for engine in [Engine::SqlServer, Engine::PostgreSql, Engine::MySql] {
        check_cases(engine, cases);
    }
}

#[test]
fn dialect_quotes_and_literals_keep_their_meaning_at_the_caret() {
    check_cases(
        Engine::SqlServer,
        &[
            ("SELECT [Order |", "[Order ", "Order ", &[], &[], true),
            ("SELECT [my schema].|", "", "", &["my schema"], &[], true),
            ("SELECT [a]]|b]", "[a]]b]", "a]", &[], &[], true),
            ("SELECT \"My Ta|", "\"My Ta", "My Ta", &[], &[], true),
            (
                "SELECT * FROM customers\nGO\nSELECT o.| FROM orders o",
                "",
                "",
                &["o"],
                &[("", "orders", "o", false)],
                true,
            ),
            (
                "SELECT * FROM customers\nGO 2 -- twice\nSELECT o.| FROM orders o",
                "",
                "",
                &["o"],
                &[("", "orders", "o", false)],
                true,
            ),
            (
                "SELECT 'a\nGO\nb', o.| FROM orders o",
                "",
                "",
                &["o"],
                &[("", "orders", "o", false)],
                true,
            ),
            ("SELECT N'a|b'", "", "", &[], &[], false),
        ],
    );
    check_cases(
        Engine::PostgreSql,
        &[
            (
                "SELECT | FROM public.orders name",
                "",
                "",
                &[],
                &[("public", "orders", "name", false)],
                true,
            ),
            (
                "WITH C AS (SELECT 1) SELECT | FROM c",
                "",
                "",
                &[],
                &[("", "c", "", true)],
                true,
            ),
            (
                "WITH c AS (SELECT 1) SELECT count(*), label | FROM orders",
                "",
                "",
                &[],
                &[("", "c", "", true), ("", "orders", "", false)],
                true,
            ),
            ("SELECT \"My Ta|", "\"My Ta", "My Ta", &[], &[], true),
            ("SELECT \"my schema\".|", "", "", &["my schema"], &[], true),
            ("SELECT \"a\"\"|b\"", "\"a\"\"b\"", "a\"", &[], &[], true),
            ("SELECT $tag$a|b$tag$", "", "", &[], &[], false),
            ("SELECT $tag$open|", "", "", &[], &[], false),
            ("SELECT E'a\\'|b'", "", "", &[], &[], false),
            ("SELECT é.名|", "名", "名", &["é"], &[], true),
        ],
    );
    check_cases(
        Engine::MySql,
        &[
            ("SELECT `ord|", "`ord", "ord", &[], &[], true),
            ("SELECT `my schema`.|", "", "", &["my schema"], &[], true),
            ("SELECT `a``|b`", "`a``b`", "a`", &[], &[], true),
            ("SELECT \"a|b\"", "", "", &[], &[], false),
            ("SELECT 1 # co|mment", "", "", &[], &[], false),
        ],
    );
}

#[test]
fn catalog_names_are_quoted_and_compared_for_each_engine() {
    for (engine, name, expected) in [
        (Engine::SqlServer, "orders", "orders"),
        (Engine::SqlServer, "Order Items", "[Order Items]"),
        (Engine::SqlServer, "select", "[select]"),
        (Engine::SqlServer, "a]b", "[a]]b]"),
        (Engine::PostgreSql, "orders", "orders"),
        (Engine::PostgreSql, "public", "public"),
        (Engine::PostgreSql, "name", "name"),
        (Engine::PostgreSql, "join", "\"join\""),
        (Engine::PostgreSql, "Orders", "\"Orders\""),
        (Engine::PostgreSql, "select", "\"select\""),
        (Engine::PostgreSql, "a\"b", "\"a\"\"b\""),
        (Engine::MySql, "Orders", "Orders"),
        (Engine::MySql, "begin", "begin"),
        (Engine::MySql, "qualify", "`qualify`"),
        (Engine::MySql, "_filename", "`_filename`"),
        (Engine::MySql, "order items", "`order items`"),
        (Engine::MySql, "select", "`select`"),
        (Engine::MySql, "a`b", "`a``b`"),
    ] {
        assert_eq!(
            completion::quote_name(name, engine).as_deref(),
            Some(expected)
        );
    }
    for (engine, typed, catalog, expected) in [
        (Engine::SqlServer, "ORDERS", "orders", true),
        (Engine::SqlServer, "[Order]]Items]", "order]items", true),
        (Engine::PostgreSql, "ORDERS", "orders", true),
        (Engine::PostgreSql, "orders", "Orders", false),
        (Engine::PostgreSql, "\"Orders\"", "Orders", true),
        (Engine::PostgreSql, "\"Orders\"", "orders", false),
        (Engine::PostgreSql, "\"a\"\"b\"", "a\"b", true),
        (Engine::MySql, "ORDERS", "orders", true),
        (Engine::MySql, "`Order``Items`", "order`items", true),
        (Engine::MySql, "`open", "open", false),
        (Engine::SqlServer, "a.b", "a.b", false),
        (Engine::PostgreSql, "'orders'", "orders", false),
    ] {
        assert_eq!(completion::names_equal(typed, catalog, engine), expected);
    }
    for engine in [Engine::SqlServer, Engine::PostgreSql, Engine::MySql] {
        assert_eq!(completion::quote_name("", engine), None);
        assert_eq!(completion::quote_name("a\0b", engine), None);
    }
}

#[test]
fn seed_0xda57_cafe_keeps_every_random_caret_range_inside_character_boundaries() {
    const SEED: u64 = 0xda57_cafe;
    const PIECES: &[&str] = &[
        "SELECT ",
        "FROM ",
        "JOIN ",
        "WITH c AS (",
        "UPDATE ",
        "DELETE FROM ",
        "INSERT INTO ",
        "MERGE INTO ",
        "USING ",
        "orders",
        "alias",
        "WHERE ",
        "'",
        "\"",
        "[",
        "]",
        "`",
        ".",
        ";",
        "\nGO\n",
        "\r\nGO 2 -- repeat\r\n",
        "-- comment\n",
        "/*",
        "*/",
        "# comment\n",
        "$$",
        "$tag$",
        "é",
        "名",
        "🦀",
        "\0",
    ];
    let mut rng = StdRng::seed_from_u64(SEED);
    for sample in 0..1500 {
        let mut text = String::new();
        for _ in 0..rng.random_range(0..40) {
            if rng.random_ratio(1, 4) {
                text.push(rng.random::<char>());
            } else {
                text.push_str(PIECES[rng.random_range(0..PIECES.len())]);
            }
        }
        for offset in text
            .char_indices()
            .map(|(offset, _)| offset)
            .chain([text.len()])
        {
            for engine in [Engine::SqlServer, Engine::PostgreSql, Engine::MySql] {
                let context = completion::context(&text, offset, engine);
                let range = context.fragment.range;
                assert!(
                    range.start <= offset && offset <= range.end && range.end <= text.len(),
                    "seed={SEED:#x} sample={sample} engine={engine:?} offset={offset} text={text:?} range={range:?}"
                );
                assert!(
                    text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
                    "seed={SEED:#x} sample={sample} engine={engine:?} offset={offset} text={text:?} range={range:?}"
                );
            }
        }
    }
}
