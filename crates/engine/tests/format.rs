use dbide_engine::danger::Engine;
use dbide_engine::format::format;

#[test]
fn short_statements_stay_on_one_line() {
    assert_eq!(
        format("select  a,b  from things where id=1;", Engine::PostgreSql).unwrap(),
        "SELECT a, b FROM things WHERE id = 1;"
    );
}

fn assert_all_engines(input: &str, expected: &str) {
    assert_eq!(format(input, Engine::SqlServer).unwrap(), expected);
    assert_eq!(format(input, Engine::PostgreSql).unwrap(), expected);
    assert_eq!(format(input, Engine::MySql).unwrap(), expected);
}

#[test]
fn bracketed_identifiers_keep_their_bytes() {
    assert_eq!(
        format("select [a b],[MiXeD] from [a table]", Engine::SqlServer).unwrap(),
        "SELECT [a b], [MiXeD] FROM [a table]"
    );
}

#[test]
fn backticked_identifiers_keep_their_bytes() {
    assert_eq!(
        format("select `a b`,`MiXeD` from `a table`", Engine::MySql).unwrap(),
        "SELECT `a b`, `MiXeD` FROM `a table`"
    );
}

#[test]
fn dollar_quoted_bodies_keep_their_bytes() {
    assert_eq!(
        format(
            "select $tag$select 'x';\n  -- body\n$tag$",
            Engine::PostgreSql
        )
        .unwrap(),
        "SELECT\n    $tag$select 'x';\n  -- body\n$tag$"
    );
}

#[test]
fn national_literals_keep_their_prefix_case_and_escapes() {
    assert_all_engines("select n'it''s \\text'", "SELECT n'it''s \\text'");
}

#[test]
fn backslashes_in_standard_literals_keep_their_bytes() {
    assert_all_engines("select 'a\\b\\n', 'it''s'", "SELECT 'a\\b\\n', 'it''s'");
}

#[test]
fn escaped_quotes_keep_their_bytes_in_mysql() {
    assert_eq!(
        format(r"select 'it\'s', 'a\\b'", Engine::MySql).unwrap(),
        r"SELECT 'it\'s', 'a\\b'"
    );
}

#[test]
fn escaped_postgresql_literals_keep_their_prefix_case_and_bytes() {
    assert_eq!(
        format(r"select e'it\'s\n'", Engine::PostgreSql).unwrap(),
        r"SELECT e'it\'s\n'"
    );
}

#[test]
fn comments_before_a_statement_end_their_line() {
    assert_all_engines(
        "-- before  \nselect a from t",
        "-- before  \nSELECT\n    a\nFROM\n    t",
    );
}

#[test]
fn comments_inside_a_statement_end_their_line() {
    assert_all_engines(
        "select a, -- inside  \n b from t",
        "SELECT\n    a, -- inside  \n    b\nFROM\n    t",
    );
}

#[test]
fn comments_after_a_statement_keep_their_bytes() {
    assert_all_engines("select a from t -- after  ", "SELECT a FROM t -- after  ");
}

#[test]
fn a_comment_after_a_semicolon_stays_before_the_next_statement() {
    assert_all_engines(
        "select a; -- after\nselect b;",
        "SELECT a;\n\n-- after\nSELECT\n    b;",
    );
}

#[test]
fn block_comments_inside_a_select_list_keep_their_bytes() {
    assert_all_engines(
        "select a,/* x  y */b from t",
        "SELECT a, /* x  y */ b FROM t",
    );
}

#[test]
fn nonlisted_words_keep_their_case() {
    assert_all_engines(
        "select name,Type,STATUS,uSeR from things",
        "SELECT name, Type, STATUS, uSeR FROM things",
    );
}

#[test]
fn newline_separated_literals_stay_on_separate_lines() {
    assert_all_engines("select 'a'\n'b'", "SELECT\n    'a'\n    'b'");
}

#[test]
fn same_line_literals_are_never_split() {
    assert_all_engines(
        "select 'a' 'b', extraordinarily_long_column_name from extraordinarily_long_table_name",
        "SELECT\n    'a' 'b',\n    extraordinarily_long_column_name\nFROM\n    extraordinarily_long_table_name",
    );
}

#[test]
fn an_unclosed_string_is_refused() {
    assert!(format("select 'unclosed", Engine::SqlServer).is_err());
    assert!(format("select 'unclosed", Engine::PostgreSql).is_err());
    assert!(format("select 'unclosed", Engine::MySql).is_err());
}

#[test]
fn long_queries_put_clause_items_on_indented_lines() {
    assert_all_engines(
        "select customer_name, total_amount from sales where total_amount>100 group by customer_name,total_amount having total_amount>200 order by customer_name,total_amount;",
        "SELECT\n    customer_name,\n    total_amount\nFROM\n    sales\nWHERE\n    total_amount > 100\nGROUP BY\n    customer_name,\n    total_amount\nHAVING\n    total_amount > 200\nORDER BY\n    customer_name,\n    total_amount;",
    );
}

#[test]
fn joins_keep_on_inline_and_indent_further_conditions() {
    assert_all_engines(
        "select a.customer_name,b.total_amount from customers a left join orders b on a.id=b.id and b.active=1 or b.active=2 where a.active=1",
        "SELECT\n    a.customer_name,\n    b.total_amount\nFROM\n    customers a\nLEFT JOIN orders b ON a.id = b.id\n    AND b.active = 1\n    OR b.active = 2\nWHERE\n    a.active = 1",
    );
}

#[test]
fn subqueries_indent_one_level_below_the_enclosing_item() {
    assert_all_engines(
        "select customer_name from customers where id in (select customer_id from orders where total_amount>100)",
        "SELECT\n    customer_name\nFROM\n    customers\nWHERE\n    id IN (\n        SELECT\n            customer_id\n        FROM\n            orders\n        WHERE\n            total_amount > 100\n    )",
    );
}

#[test]
fn commas_inside_functions_do_not_split_select_items() {
    assert_all_engines(
        "select coalesce(customer_name,alternate_name),total_amount from extraordinarily_long_table_name",
        "SELECT\n    coalesce(customer_name, alternate_name),\n    total_amount\nFROM\n    extraordinarily_long_table_name",
    );
}

#[test]
fn statements_have_one_blank_line_between_them() {
    assert_all_engines("select 1;select 2;", "SELECT 1;\n\nSELECT 2;");
}

#[test]
fn batch_separators_stay_on_their_own_line() {
    assert_eq!(
        format("select 1\nGO\nselect 2", Engine::SqlServer).unwrap(),
        "SELECT 1\nGO\n\nSELECT 2"
    );
}

#[test]
fn a_fragment_formats_without_a_complete_statement() {
    assert_all_engines("name,Type,status", "name, Type, status");
}

#[test]
fn unrecognised_long_statements_retain_safe_layout() {
    assert_all_engines(
        "create table extraordinarily_long_table_name (id integer, name varchar(200), status integer);",
        "CREATE table extraordinarily_long_table_name (id integer, name varchar(200), status integer);",
    );
}

#[test]
fn procedural_blocks_preserve_literal_and_comment_bytes() {
    assert_eq!(format("create function f() returns void as $tag$begin\n  perform 'x'; -- body\nend$tag$ language plpgsql;", Engine::PostgreSql).unwrap(), "CREATE function f() returns void AS $tag$begin\n  perform 'x'; -- body\nend$tag$ language plpgsql;");
}

#[test]
fn executable_comments_are_opaque_in_mysql() {
    assert_eq!(
        format("select /*!50000 SQL_NO_CACHE */ a", Engine::MySql).unwrap(),
        "select /*!50000 SQL_NO_CACHE */ a"
    );
}

#[test]
fn unicode_and_crlf_locations_preserve_token_bytes() {
    assert_all_engines(
        "select \"état\",\r\n'🌳' from t",
        "SELECT \"état\", '🌳' FROM t",
    );
}

#[test]
fn formatting_twice_gives_the_same_text() {
    let once = format("select customer_name, total_amount from sales where total_amount>100 order by customer_name,total_amount;", Engine::PostgreSql).unwrap();
    assert_eq!(
        once,
        "SELECT\n    customer_name,\n    total_amount\nFROM\n    sales\nWHERE\n    total_amount > 100\nORDER BY\n    customer_name,\n    total_amount;"
    );
    assert_all_engines(
        &once,
        "SELECT\n    customer_name,\n    total_amount\nFROM\n    sales\nWHERE\n    total_amount > 100\nORDER BY\n    customer_name,\n    total_amount;",
    );
}

#[test]
fn empty_fragments_format_to_empty_text() {
    assert_all_engines(" \r\n\t", "");
}

#[test]
fn qualified_keywords_keep_identifier_case() {
    assert_eq!(
        format("select * from app.select", Engine::MySql).unwrap(),
        "SELECT * FROM app.select"
    );
}

#[test]
fn nonreserved_full_and_end_keep_identifier_case() {
    assert_eq!(
        format("select full,end from things", Engine::MySql).unwrap(),
        "SELECT full, end FROM things"
    );
}

#[test]
fn a_comment_after_go_keeps_the_batch_boundary() {
    assert_eq!(
        format("select 1\nGO -- batch\nselect 2", Engine::SqlServer).unwrap(),
        "SELECT 1\nGO -- batch\n\nSELECT 2"
    );
}

#[test]
fn comments_in_qualified_names_do_not_turn_identifiers_into_keywords() {
    assert_eq!(
        format("select * from app./* name */select", Engine::MySql).unwrap(),
        "SELECT * FROM app./* name */ select"
    );
}

#[test]
fn block_comments_after_go_keep_the_batch_boundary() {
    assert_eq!(
        format("select 1\nGO /* batch */\nselect 2", Engine::SqlServer).unwrap(),
        "SELECT 1\nGO /* batch */\n\nSELECT 2"
    );
}

#[test]
fn spaces_that_separate_numeric_tokens_are_not_removed() {
    assert_all_engines("1 . 2", "1 . 2");
}

#[test]
fn nonreserved_between_and_exists_keep_identifier_case() {
    assert_eq!(
        format("select between,exists from things", Engine::PostgreSql).unwrap(),
        "SELECT between, exists FROM things"
    );
}

#[test]
fn a_go_repeat_count_keeps_the_batch_boundary() {
    assert_eq!(
        format("select 1\nGO 2\nselect 2", Engine::SqlServer).unwrap(),
        "SELECT 1\nGO 2\n\nSELECT 2"
    );
}

#[test]
fn literals_continued_across_a_comment_keep_their_line_break() {
    assert_eq!(
        format("select 'a' /* c */\n'b'", Engine::PostgreSql).unwrap(),
        "SELECT\n    'a' /* c */\n    'b'"
    );
}

#[test]
fn adjacent_operator_characters_stay_together() {
    assert_eq!(
        format(
            "select 1 === 2, 3 OPERATOR(public.===) 4",
            Engine::PostgreSql
        )
        .unwrap(),
        "SELECT 1 === 2, 3 OPERATOR(public.===) 4"
    );
}

#[test]
fn formatting_that_would_make_a_column_named_go_a_batch_separator_is_refused() {
    let input = "select go from extraordinarily_long_table_name_that_makes_this_statement_exceed_eighty_characters";
    assert!(format(input, Engine::SqlServer).is_err());
}
