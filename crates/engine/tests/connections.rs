use dbide_engine::connections::{
    Connection, Connections, EffectiveSettings, SignInMethod, TlsMode,
};
use dbide_engine::connections::{connection_string, credentials::Credentials, regsrvr};
use dbide_engine::danger::Engine;

#[test]
fn pasted_uri_decodes_credentials_without_exposing_them_for_mysql() {
    let parsed = connection_string::parse(
        "mysql://reader:invented%40secret@db.example.test:3307/trees?sslmode=disabled",
    )
    .unwrap();
    assert_eq!(parsed.connection.engine, Engine::MySql);
    assert_eq!(parsed.connection.host, "db.example.test");
    assert_eq!(parsed.connection.port, Some(3307));
    assert_eq!(parsed.connection.database.as_deref(), Some("trees"));
    assert_eq!(parsed.connection.tls, TlsMode::Off);
    assert_eq!(
        parsed.password.as_ref().unwrap().expose(),
        "invented@secret"
    );
    assert!(!format!("{parsed:?}").contains("invented@secret"));
}

#[test]
fn pasted_ado_provider_selects_password_sign_in_for_mysql() {
    let parsed = connection_string::parse("Provider=MySQL;Server=db.example.test;Port=3307;Database=trees;User ID=reader;Password=invented-secret;SslMode=Disabled").unwrap();
    assert_eq!(parsed.connection.engine, Engine::MySql);
    assert_eq!(parsed.connection.port, Some(3307));
    assert_eq!(parsed.connection.tls, TlsMode::Off);
    assert_eq!(parsed.connection.sign_in, SignInMethod::Password);
}

#[test]
fn a_missing_file_is_an_empty_tree() {
    let directory = tempfile::tempdir().unwrap();
    let tree = Connections::load(directory.path().join("connections.toml")).unwrap();
    assert!(tree.connections().is_empty());
    assert!(tree.folders().is_empty());
}

#[test]
fn a_round_trip_keeps_every_connection_and_folder_field() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/connections.toml");
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    tree.set_folder_settings(&parent, Some("#FF0000".into()), Some(false))
        .unwrap();
    let child = tree.add_folder("Europe", Some(&parent)).unwrap();
    let id = tree
        .add_connection(
            Connection {
                name: "Orders".into(),
                engine: Engine::SqlServer,
                host: "sql01.example.test".into(),
                port: None,
                instance: Some("REPORTS".into()),
                sign_in: SignInMethod::Password,
                user: Some("report_reader".into()),
                database: Some("orders".into()),
                colour: Some("#123456".into()),
                tls: TlsMode::Optional,
                trust_server_certificate: true,
                application_name: "Orders IDE".into(),
                connect_timeout: 27,
                query_timeout: 83,
                danger_check: Some(true),
            },
            Some(&child),
        )
        .unwrap();
    tree.save(&path).unwrap();
    let loaded = Connections::load(&path).unwrap();
    assert_eq!(
        loaded.connections()[0].connection,
        Connection {
            name: "Orders".into(),
            engine: Engine::SqlServer,
            host: "sql01.example.test".into(),
            port: None,
            instance: Some("REPORTS".into()),
            sign_in: SignInMethod::Password,
            user: Some("report_reader".into()),
            database: Some("orders".into()),
            colour: Some("#123456".into()),
            tls: TlsMode::Optional,
            trust_server_certificate: true,
            application_name: "Orders IDE".into(),
            connect_timeout: 27,
            query_timeout: 83,
            danger_check: Some(true),
        }
    );
    assert_eq!(loaded.connections()[0].id, id);
    assert_eq!(
        loaded.connections()[0].parent.as_deref(),
        Some(child.as_str())
    );
    assert_eq!(loaded.folders()[0].name, "Prod");
    assert_eq!(loaded.folders()[0].colour.as_deref(), Some("#FF0000"));
    assert_eq!(loaded.folders()[0].danger_check, Some(false));
    assert_eq!(loaded.folders()[1].name, "Europe");
    assert_eq!(loaded.folders()[1].parent.as_deref(), Some(parent.as_str()));
}

#[test]
fn tcp_ports_and_windows_sign_in_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    let mut tree = Connections::default();
    let mut connection = Connection::new(Engine::PostgreSql, "pg01.example.test");
    connection.port = Some(5543);
    connection.sign_in = SignInMethod::Windows;
    tree.add_connection(connection, None).unwrap();
    tree.save(&path).unwrap();
    let loaded = Connections::load(path).unwrap();
    assert_eq!(
        loaded.connections()[0].connection.engine,
        Engine::PostgreSql
    );
    assert_eq!(loaded.connections()[0].connection.port, Some(5543));
    assert_eq!(
        loaded.connections()[0].connection.sign_in,
        SignInMethod::Windows
    );
}

#[test]
fn settings_inherit_independently_from_the_nearest_folder() {
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    tree.set_folder_settings(&parent, Some("red".into()), Some(false))
        .unwrap();
    let child = tree.add_folder("Orders", Some(&parent)).unwrap();
    tree.set_folder_settings(&child, Some("orange".into()), None)
        .unwrap();
    let id = tree
        .add_connection(
            Connection::new(Engine::MySql, "mysql01.example.test"),
            Some(&child),
        )
        .unwrap();
    assert_eq!(
        tree.effective_settings(&id).unwrap(),
        EffectiveSettings {
            colour: Some("orange"),
            danger_check: false
        }
    );
}

#[test]
fn connection_overrides_win_over_folder_settings() {
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    tree.set_folder_settings(&parent, Some("red".into()), Some(false))
        .unwrap();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.colour = Some("green".into());
    connection.danger_check = Some(true);
    let id = tree.add_connection(connection, Some(&parent)).unwrap();
    assert_eq!(
        tree.effective_settings(&id).unwrap(),
        EffectiveSettings {
            colour: Some("green"),
            danger_check: true
        }
    );
}

#[test]
fn danger_check_defaults_to_on_and_colour_to_none() {
    let mut tree = Connections::default();
    let id = tree
        .add_connection(
            Connection::new(Engine::SqlServer, "sql01.example.test"),
            None,
        )
        .unwrap();
    assert_eq!(
        tree.effective_settings(&id).unwrap(),
        EffectiveSettings {
            colour: None,
            danger_check: true
        }
    );
}

#[test]
fn renaming_and_moving_keep_connection_identity() {
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    let child = tree.add_folder("Orders", None).unwrap();
    let id = tree
        .add_connection(
            Connection::new(Engine::SqlServer, "sql01.example.test"),
            Some(&child),
        )
        .unwrap();
    tree.rename(&id, "Reports").unwrap();
    tree.rename(&child, "Reporting").unwrap();
    tree.move_to(&child, Some(&parent)).unwrap();
    tree.move_to(&id, Some(&parent)).unwrap();
    assert_eq!(tree.connections()[0].id, id);
    assert_eq!(tree.connections()[0].connection.name, "Reports");
    assert_eq!(
        tree.connections()[0].parent.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(tree.folders()[1].name, "Reporting");
    assert_eq!(tree.folders()[1].parent.as_deref(), Some(parent.as_str()));
}

#[test]
fn moving_a_folder_below_itself_is_refused_without_changing_the_tree() {
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    let child = tree.add_folder("Orders", Some(&parent)).unwrap();
    assert!(tree.move_to(&parent, Some(&child)).is_err());
    assert_eq!(tree.folders()[0].parent, None);
    assert_eq!(tree.folders()[1].parent.as_deref(), Some(parent.as_str()));
}

#[test]
fn edits_reject_missing_parents_and_blank_names() {
    let mut tree = Connections::default();
    assert!(tree.add_folder("Prod", Some("missing")).is_err());
    assert!(tree.add_folder(" ", None).is_err());
    assert!(
        tree.add_connection(
            Connection::new(Engine::MySql, "mysql01.example.test"),
            Some("missing")
        )
        .is_err()
    );
    assert!(tree.connections().is_empty());
    assert!(tree.folders().is_empty());
}

#[test]
fn saving_replaces_an_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    let mut tree = Connections::default();
    let id = tree.add_folder("Before", None).unwrap();
    tree.save(&path).unwrap();
    tree.rename(&id, "After").unwrap();
    tree.save(&path).unwrap();
    assert_eq!(Connections::load(path).unwrap().folders()[0].name, "After");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn a_failed_replace_removes_the_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep.txt"), "kept").unwrap();
    assert!(Connections::default().save(&path).is_err());
    assert_eq!(
        std::fs::read_to_string(path.join("keep.txt")).unwrap(),
        "kept"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn a_failed_replace_keeps_the_existing_regular_file() {
    use std::os::windows::fs::OpenOptionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    std::fs::write(&path, "folders = []\nconnections = []\n").unwrap();
    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let mut tree = Connections::default();
    tree.add_folder("Prod", None).unwrap();
    assert!(tree.save(&path).is_err());
    drop(locked);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "folders = []\nconnections = []\n"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn pasted_passwords_never_reach_toml_or_debug() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    let parsed = connection_string::parse(
        "Server=sql01.example.test;User ID=reader;Password=fixture-secret",
    )
    .unwrap();
    assert!(!format!("{parsed:?}").contains("fixture-secret"));
    assert_eq!(parsed.password.as_ref().unwrap().expose(), "fixture-secret");
    let mut tree = Connections::default();
    tree.add_connection(parsed.connection, None).unwrap();
    tree.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("fixture-secret"));
    assert!(!text.contains("password ="));
    assert_eq!(
        Connections::load(path).unwrap().connections()[0]
            .connection
            .user
            .as_deref(),
        Some("reader")
    );
}

#[test]
fn malformed_credential_password_bytes_are_redacted_in_errors() {
    let error = dbide_engine::connections::credentials::Error::from(keyring::Error::BadEncoding(
        b"fixture-secret".to_vec(),
    ));
    assert_eq!(
        format!("{error:?}"),
        "Keyring(\"Data is not UTF-8 encoded\")"
    );
    assert_eq!(
        error.to_string(),
        "credential store: Data is not UTF-8 encoded"
    );
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn password_fields_in_hand_edited_toml_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    std::fs::write(&path, "password = 'fixture-secret'").unwrap();
    assert!(Connections::load(path).is_err());
}

#[test]
fn cyclic_toml_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    std::fs::write(
        &path,
        "[[folders]]\nid='a'\nname='A'\nparent='b'\n[[folders]]\nid='b'\nname='B'\nparent='a'\n",
    )
    .unwrap();
    assert!(Connections::load(path).is_err());
}

#[test]
fn duplicate_ids_in_toml_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    std::fs::write(
        &path,
        "[[folders]]\nid='a'\nname='A'\n[[folders]]\nid='a'\nname='B'\n",
    )
    .unwrap();
    assert!(Connections::load(path).is_err());
}

#[test]
fn missing_parents_in_toml_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    std::fs::write(&path, "[[folders]]\nid='a'\nname='A'\nparent='missing'\n").unwrap();
    assert!(Connections::load(path).is_err());
}

#[test]
fn ado_tcp_address_and_windows_sign_in_parse() {
    let parsed = connection_string::parse("Server=tcp:sql01.example.test,1444;Database=orders;Integrated Security=true;Encrypt=Optional;TrustServerCertificate=yes;Application Name=Reports;Connect Timeout=12;Command Timeout=33").unwrap();
    assert_eq!(parsed.connection.engine, Engine::SqlServer);
    assert_eq!(parsed.connection.host, "sql01.example.test");
    assert_eq!(parsed.connection.port, Some(1444));
    assert_eq!(parsed.connection.instance, None);
    assert_eq!(parsed.connection.database.as_deref(), Some("orders"));
    assert_eq!(parsed.connection.sign_in, SignInMethod::Windows);
    assert_eq!(parsed.connection.tls, TlsMode::Optional);
    assert!(parsed.connection.trust_server_certificate);
    assert_eq!(parsed.connection.application_name, "Reports");
    assert_eq!(parsed.connection.connect_timeout, 12);
    assert_eq!(parsed.connection.query_timeout, 33);
}

#[test]
fn ado_named_instance_and_quoted_password_parse() {
    let parsed = connection_string::parse("Data Source=sql01.example.test\\REPORTS;Initial Catalog=orders;UID=reader;Pwd=\"a;b\"\"c\"").unwrap();
    assert_eq!(parsed.connection.host, "sql01.example.test");
    assert_eq!(parsed.connection.instance.as_deref(), Some("REPORTS"));
    assert_eq!(parsed.connection.port, None);
    assert_eq!(parsed.connection.database.as_deref(), Some("orders"));
    assert_eq!(parsed.connection.sign_in, SignInMethod::Password);
    assert_eq!(parsed.connection.engine, Engine::SqlServer);
    assert_eq!(parsed.password.unwrap().expose(), "a;b\"c");
}

#[test]
fn libpq_keywords_decode_quotes_and_backslash_escapes() {
    let parsed = connection_string::parse("host=pg01.example.test port=5543 dbname='order archive' user=reader password='a\\'b\\\\c' sslmode=verify-full").unwrap();
    assert_eq!(parsed.connection.host, "pg01.example.test");
    assert_eq!(parsed.connection.port, Some(5543));
    assert_eq!(parsed.connection.database.as_deref(), Some("order archive"));
    assert_eq!(parsed.connection.sign_in, SignInMethod::Password);
    assert_eq!(parsed.connection.engine, Engine::PostgreSql);
    assert_eq!(parsed.connection.tls, TlsMode::Mandatory);
    assert!(!parsed.connection.trust_server_certificate);
    assert_eq!(parsed.password.unwrap().expose(), "a'b\\c");
}

#[test]
fn libpq_detection_ignores_option_order_and_quoted_semicolons() {
    let parsed = connection_string::parse("options='-c search_path=public' user=reader password='a;b' host=pg01.example.test dbname=orders").unwrap();
    assert_eq!(parsed.connection.host, "pg01.example.test");
    assert_eq!(parsed.connection.engine, Engine::PostgreSql);
    assert_eq!(parsed.password.unwrap().expose(), "a;b");
}

#[test]
fn libpq_uri_percent_decodes_the_hostname() {
    let parsed = connection_string::parse("postgresql://%70g01.example.test/orders").unwrap();
    assert_eq!(parsed.connection.host, "pg01.example.test");
}

#[test]
fn libpq_uri_can_supply_the_host_in_query_options() {
    let parsed =
        connection_string::parse("postgresql:///orders?host=pg01.example.test&port=5543").unwrap();
    assert_eq!(parsed.connection.host, "pg01.example.test");
    assert_eq!(parsed.connection.port, Some(5543));
    assert_eq!(parsed.connection.database.as_deref(), Some("orders"));
}

#[test]
fn jdbc_query_values_decode_plus_as_a_space() {
    let parsed =
        connection_string::parse("jdbc:postgresql://pg01.example.test/orders?password=a+b%2Bc")
            .unwrap();
    assert_eq!(parsed.password.unwrap().expose(), "a b+c");
}

#[test]
fn libpq_query_values_preserve_plus_and_ssl_true_means_require() {
    let parsed =
        connection_string::parse("postgresql://pg01.example.test/orders?password=a+b&ssl=true")
            .unwrap();
    assert_eq!(parsed.password.unwrap().expose(), "a+b");
    assert_eq!(parsed.connection.tls, TlsMode::Mandatory);
    assert!(parsed.connection.trust_server_certificate);
}

#[test]
fn jdbc_explicit_tls_modes_override_legacy_flags_in_either_order() {
    let cases = [
        "jdbc:postgresql://pg01.example.test/orders?sslmode=verify-full&ssl=false",
        "jdbc:postgresql://pg01.example.test/orders?ssl=false&sslmode=verify-full",
        "jdbc:mysql://mysql01.example.test/orders?sslMode=VERIFY_IDENTITY&useSSL=false",
        "jdbc:mysql://mysql01.example.test/orders?useSSL=false&sslMode=VERIFY_IDENTITY",
    ];
    for input in cases {
        let parsed = connection_string::parse(input).unwrap();
        assert_eq!(parsed.connection.tls, TlsMode::Mandatory);
        assert!(!parsed.connection.trust_server_certificate);
    }
}

#[test]
fn libpq_uri_decodes_credentials_database_and_options() {
    let parsed = connection_string::parse("postgresql://read%40only:p%3Bass@pg01.example.test:5543/order%20archive?application_name=Reports&sslmode=require").unwrap();
    assert_eq!(parsed.connection.host, "pg01.example.test");
    assert_eq!(parsed.connection.port, Some(5543));
    assert_eq!(parsed.connection.database.as_deref(), Some("order archive"));
    assert_eq!(parsed.connection.user.as_deref(), Some("read@only"));
    assert_eq!(parsed.connection.sign_in, SignInMethod::Password);
    assert_eq!(parsed.connection.engine, Engine::PostgreSql);
    assert_eq!(parsed.password.unwrap().expose(), "p;ass");
    assert_eq!(parsed.connection.application_name, "Reports");
    assert!(parsed.connection.trust_server_certificate);
}

#[test]
fn jdbc_sqlserver_parses_instance_and_brace_escapes() {
    let parsed = connection_string::parse("jdbc:sqlserver://sql01.example.test\\REPORTS;databaseName=orders;user=reader;password={a;}}b};encrypt=true").unwrap();
    assert_eq!(parsed.connection.host, "sql01.example.test");
    assert_eq!(parsed.connection.port, None);
    assert_eq!(parsed.connection.instance.as_deref(), Some("REPORTS"));
    assert_eq!(parsed.connection.database.as_deref(), Some("orders"));
    assert_eq!(parsed.connection.engine, Engine::SqlServer);
    assert_eq!(parsed.connection.sign_in, SignInMethod::Password);
    assert_eq!(parsed.password.unwrap().expose(), "a;}b");
}

#[test]
fn jdbc_postgresql_parses_port_database_and_password_prompting() {
    let parsed = connection_string::parse(
        "jdbc:postgresql://pg01.example.test:5543/orders?user=reader&connectTimeout=8",
    )
    .unwrap();
    assert_eq!(parsed.connection.host, "pg01.example.test");
    assert_eq!(parsed.connection.port, Some(5543));
    assert_eq!(parsed.connection.database.as_deref(), Some("orders"));
    assert_eq!(parsed.connection.sign_in, SignInMethod::AskEachTime);
    assert_eq!(parsed.connection.engine, Engine::PostgreSql);
    assert_eq!(parsed.connection.connect_timeout, 8);
}

#[test]
fn jdbc_mysql_parses_credentials_and_converts_millisecond_timeouts() {
    let parsed = connection_string::parse("jdbc:mysql://mysql01.example.test:3307/orders?user=reader&password=fixture&connectTimeout=2500&socketTimeout=6000&sslMode=VERIFY_IDENTITY").unwrap();
    assert_eq!(parsed.connection.host, "mysql01.example.test");
    assert_eq!(parsed.connection.port, Some(3307));
    assert_eq!(parsed.connection.database.as_deref(), Some("orders"));
    assert_eq!(parsed.connection.sign_in, SignInMethod::Password);
    assert_eq!(parsed.connection.engine, Engine::MySql);
    assert_eq!(parsed.connection.connect_timeout, 3);
    assert_eq!(parsed.connection.query_timeout, 6);
    assert!(!parsed.connection.trust_server_certificate);
}

#[test]
fn bracketed_ipv6_hosts_parse_without_losing_the_port() {
    let parsed = connection_string::parse("postgresql://[2001:db8::1]:5543/orders").unwrap();
    assert_eq!(parsed.connection.host, "2001:db8::1");
    assert_eq!(parsed.connection.port, Some(5543));
}

#[test]
fn malformed_connection_strings_are_errors_without_echoing_secrets() {
    let cases = [
        "nonsense",
        "Server=",
        "Server=sql01.example.test;Password='fixture-secret",
        "host=pg01.example.test port=99999",
        "jdbc:postgresql://pg01.example.test/orders?password=%XY",
        "Server=sql01.example.test;Integrated Security=perhaps",
        "host='pg01.example.test'garbage",
        "jdbc:mysql://a.example.test,b.example.test/orders",
        "Server=sql01.example.test;Connect Timeout=-1",
    ];
    for input in cases {
        let error = connection_string::parse(input).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("fixture-secret"));
    }
}

struct CredentialCleanup {
    credentials: Credentials,
    ids: Vec<String>,
}

impl CredentialCleanup {
    fn new(ids: Vec<String>) -> Self {
        Self {
            credentials: Credentials::new("database-ide-test").unwrap(),
            ids,
        }
    }
}

impl Drop for CredentialCleanup {
    fn drop(&mut self) {
        for id in &self.ids {
            let _ = self.credentials.delete(id);
        }
    }
}

#[test]
fn regsrvr_import_preserves_nested_folders_and_drops_passwords() {
    let tree = regsrvr::parse(include_str!("fixtures/connections/RegSrvr.xml")).unwrap();
    assert_eq!(tree.folders().len(), 2);
    assert_eq!(tree.folders()[0].name, "DatabaseEngineServerGroup");
    assert_eq!(tree.folders()[1].name, "Prod");
    assert_eq!(
        tree.folders()[1].parent.as_deref(),
        Some(tree.folders()[0].id.as_str())
    );
    assert_eq!(tree.connections().len(), 1);
    assert_eq!(
        tree.connections()[0].parent.as_deref(),
        Some(tree.folders()[1].id.as_str())
    );
    assert_eq!(tree.connections()[0].connection.name, "Orders & Reports");
    assert_eq!(tree.connections()[0].connection.host, "sql01.example.test");
    assert_eq!(tree.connections()[0].connection.port, Some(1444));
    assert_eq!(
        tree.connections()[0].connection.database.as_deref(),
        Some("orders")
    );
    assert_eq!(
        tree.connections()[0].connection.sign_in,
        SignInMethod::AskEachTime
    );
    assert_eq!(
        tree.connections()[0].connection.colour.as_deref(),
        Some("#FF0000")
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    tree.save(&path).unwrap();
    assert!(
        !std::fs::read_to_string(path)
            .unwrap()
            .contains("fixture-encrypted-password")
    );
    assert!(!format!("{tree:?}").contains("fixture-encrypted-password"));
}

#[test]
fn regsrvr_export_import_keeps_windows_sign_in_and_external_parent_becomes_root() {
    let tree = regsrvr::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/connections/servers.regsrvr"
    ))
    .unwrap();
    assert_eq!(tree.folders().len(), 1);
    assert_eq!(tree.folders()[0].name, "Dev");
    assert_eq!(tree.folders()[0].parent, None);
    assert_eq!(tree.connections()[0].connection.host, "sql02.example.test");
    assert_eq!(
        tree.connections()[0].connection.instance.as_deref(),
        Some("DEV")
    );
    assert_eq!(
        tree.connections()[0].connection.database.as_deref(),
        Some("inventory")
    );
    assert_eq!(
        tree.connections()[0].connection.sign_in,
        SignInMethod::Windows
    );
    assert_eq!(tree.connections()[0].connection.engine, Engine::SqlServer);
    assert!(!format!("{tree:?}").contains("fixture-ignored-password"));
}

#[test]
fn regsrvr_import_accepts_utf16_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("servers.regsrvr");
    let text = include_str!("fixtures/connections/servers.regsrvr");
    let bytes: Vec<_> = [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(
        regsrvr::open(path).unwrap().connections()[0]
            .connection
            .name,
        "Inventory"
    );
}

#[test]
fn malformed_regsrvr_and_dtds_are_errors() {
    assert!(regsrvr::parse("<model><document>").is_err());
    assert!(regsrvr::parse("<!DOCTYPE model [<!ENTITY password 'secret'>]><model/>").is_err());
    assert!(regsrvr::parse("<unrelated/>").is_err());
}

#[test]
fn duplicate_regsrvr_aliases_are_rejected() {
    let xml = include_str!("fixtures/connections/RegSrvr.xml").replace(
        "/RegisteredServersStore/ServerGroup/DatabaseEngineServerGroup/ServerGroup/Prod",
        "/RegisteredServersStore/ServerGroup/DatabaseEngineServerGroup",
    );
    assert!(regsrvr::parse(&xml).is_err());
}

#[test]
fn cyclic_regsrvr_folders_are_rejected() {
    let xml = include_str!("fixtures/connections/RegSrvr.xml").replace(
        "<sml:Uri>/RegisteredServersStore</sml:Uri>",
        "<sml:Uri>/RegisteredServersStore/ServerGroup/DatabaseEngineServerGroup/ServerGroup/Prod</sml:Uri>",
    );
    assert!(regsrvr::parse(&xml).is_err());
}

#[test]
fn sign_in_edits_return_credential_deletions_only_for_non_password_methods() {
    let mut tree = Connections::default();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.sign_in = SignInMethod::Password;
    let id = tree.add_connection(connection.clone(), None).unwrap();

    assert!(
        tree.update_connection(&id, connection.clone())
            .unwrap()
            .is_empty()
    );
    connection.sign_in = SignInMethod::Windows;
    assert_eq!(
        tree.update_connection(&id, connection.clone()).unwrap(),
        vec![id.clone()]
    );
    assert_eq!(
        tree.connections()[0].connection.sign_in,
        SignInMethod::Windows
    );
    connection.sign_in = SignInMethod::AskEachTime;
    assert_eq!(
        tree.update_connection(&id, connection.clone()).unwrap(),
        vec![id.clone()]
    );
    let before = tree.clone();
    connection.host.clear();
    assert!(tree.update_connection(&id, connection).is_err());
    assert_eq!(tree, before);
    assert!(
        tree.update_connection(
            "missing",
            Connection::new(Engine::SqlServer, "sql01.example.test")
        )
        .is_err()
    );
    assert_eq!(tree, before);
}

#[test]
fn deleting_a_subtree_returns_only_its_connection_credential_ids() {
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    let child = tree.add_folder("Orders", Some(&parent)).unwrap();
    let connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    let root = tree
        .add_connection(connection.clone(), Some(&parent))
        .unwrap();
    let nested = tree
        .add_connection(connection.clone(), Some(&child))
        .unwrap();
    let kept = tree.add_connection(connection, None).unwrap();

    assert_eq!(tree.delete(&parent).unwrap(), vec![root, nested]);
    assert!(tree.folders().is_empty());
    assert_eq!(tree.connections().len(), 1);
    assert_eq!(tree.connections()[0].id, kept);
    assert!(tree.delete("missing").unwrap().is_empty());
    assert_eq!(tree.delete(&kept).unwrap(), vec![kept]);
    assert!(tree.connections().is_empty());
}

#[test]
fn credman_failed_save_after_delete_keeps_the_password() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    let mut tree = Connections::default();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.sign_in = SignInMethod::Password;
    let id = tree.add_connection(connection, None).unwrap();
    let cleanup = CredentialCleanup::new(vec![id.clone()]);
    let saved = tree.connections()[0].clone();
    cleanup.credentials.store(&saved, "fixture-secret").unwrap();
    tree.save(&path).unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, "blocked").unwrap();

    tree.delete(&id).unwrap();
    assert!(tree.save(blocker.join("connections.toml")).is_err());

    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert_eq!(
        cleanup.credentials.read(&saved).unwrap().unwrap().expose(),
        "fixture-secret"
    );
}

#[test]
fn credman_failed_save_after_sign_in_change_keeps_the_password() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    let mut tree = Connections::default();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.sign_in = SignInMethod::Password;
    let id = tree.add_connection(connection, None).unwrap();
    let cleanup = CredentialCleanup::new(vec![id.clone()]);
    let saved = tree.connections()[0].clone();
    cleanup.credentials.store(&saved, "fixture-secret").unwrap();
    tree.save(&path).unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, "blocked").unwrap();
    let mut details = saved.connection.clone();
    details.sign_in = SignInMethod::AskEachTime;

    tree.update_connection(&id, details).unwrap();
    assert!(tree.save(blocker.join("connections.toml")).is_err());

    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert_eq!(
        cleanup.credentials.read(&saved).unwrap().unwrap().expose(),
        "fixture-secret"
    );
}

#[test]
fn credman_password_survives_rename_and_is_deleted_with_the_connection() {
    let mut tree = Connections::default();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.sign_in = SignInMethod::Password;
    let id = tree.add_connection(connection, None).unwrap();
    let cleanup = CredentialCleanup::new(vec![id.clone()]);
    cleanup
        .credentials
        .store(&tree.connections()[0], "fixture-secret")
        .unwrap();
    tree.rename(&id, "Reports").unwrap();
    assert_eq!(
        cleanup
            .credentials
            .read(&tree.connections()[0])
            .unwrap()
            .unwrap()
            .expose(),
        "fixture-secret"
    );
    let saved = tree.connections()[0].clone();
    let deletions = tree.delete(&id).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    tree.save(&path).unwrap();
    assert!(Connections::load(&path).unwrap().connections().is_empty());
    assert_eq!(deletions, vec![id]);
    assert!(cleanup.credentials.read(&saved).unwrap().is_some());
    for id in deletions {
        cleanup.credentials.delete(&id).unwrap();
    }
    assert!(tree.connections().is_empty());
    assert!(cleanup.credentials.read(&saved).unwrap().is_none());
}

#[test]
fn credman_ask_each_time_deletes_a_previously_saved_password() {
    let mut tree = Connections::default();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.sign_in = SignInMethod::Password;
    let id = tree.add_connection(connection, None).unwrap();
    let cleanup = CredentialCleanup::new(vec![id.clone()]);
    cleanup
        .credentials
        .store(&tree.connections()[0], "fixture-secret")
        .unwrap();
    let saved = tree.connections()[0].clone();
    let mut details = saved.connection.clone();
    details.sign_in = SignInMethod::AskEachTime;
    let deletions = tree.update_connection(&id, details).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("connections.toml");
    tree.save(&path).unwrap();
    assert_eq!(
        Connections::load(&path).unwrap().connections()[0]
            .connection
            .sign_in,
        SignInMethod::AskEachTime
    );
    assert_eq!(deletions, vec![id]);
    assert!(cleanup.credentials.read(&saved).unwrap().is_some());
    for id in deletions {
        cleanup.credentials.delete(&id).unwrap();
    }
    assert!(cleanup.credentials.read(&saved).unwrap().is_none());
    cleanup
        .credentials
        .store(&tree.connections()[0], "must-not-store")
        .unwrap();
    assert!(
        cleanup
            .credentials
            .read(&tree.connections()[0])
            .unwrap()
            .is_none()
    );
    assert!(cleanup.credentials.read(&saved).unwrap().is_none());
}

#[test]
fn credman_deleting_a_folder_deletes_descendants_and_their_passwords() {
    let mut tree = Connections::default();
    let parent = tree.add_folder("Prod", None).unwrap();
    let child = tree.add_folder("Orders", Some(&parent)).unwrap();
    let mut connection = Connection::new(Engine::SqlServer, "sql01.example.test");
    connection.sign_in = SignInMethod::Password;
    let id = tree.add_connection(connection, Some(&child)).unwrap();
    let cleanup = CredentialCleanup::new(vec![id]);
    let saved = tree.connections()[0].clone();
    cleanup.credentials.store(&saved, "fixture-secret").unwrap();
    let deletions = tree.delete(&parent).unwrap();
    let directory = tempfile::tempdir().unwrap();
    tree.save(directory.path().join("connections.toml"))
        .unwrap();
    assert_eq!(deletions, vec![saved.id.clone()]);
    assert!(cleanup.credentials.read(&saved).unwrap().is_some());
    for id in deletions {
        cleanup.credentials.delete(&id).unwrap();
    }
    assert!(tree.folders().is_empty());
    assert!(tree.connections().is_empty());
    assert!(cleanup.credentials.read(&saved).unwrap().is_none());
}
