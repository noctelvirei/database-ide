use dbide_engine::connections::{Connection, SignInMethod, TlsMode};
use dbide_engine::danger::Engine;
use dbide_engine::session::{ConnectErrorKind, connect};

/// A port on 127.0.0.1 that refuses connections: bound, so nothing else takes it, but not
/// listening. Keep the socket alive for the test.
fn closed_port() -> (tokio::net::TcpSocket, u16) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = socket.local_addr().unwrap().port();
    (socket, port)
}

fn details(engine: Engine, host: &str, port: u16, user: &str, database: &str) -> Connection {
    let mut details = Connection::new(engine, host);
    details.port = Some(port);
    details.user = Some(user.into());
    details.database = Some(database.into());
    details.tls = TlsMode::Off;
    details.trust_server_certificate = true;
    details
}

#[tokio::test]
async fn unresolvable_host_returns_unreachable_without_disclosing_password() {
    let mut details = Connection::new(Engine::PostgreSql, "dbide-connect.invalid");
    details.user = Some("dbide_test".into());
    details.tls = TlsMode::Off;
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::Unreachable, "{error:?}");
    assert_eq!(
        error.message,
        "Can't reach dbide-connect.invalid:5432: the host name wasn't found."
    );
    assert!(!format!("{error:?}").contains("invented-secret"));
}

#[tokio::test]
async fn mysql_windows_sign_in_returns_invalid_details_without_disclosing_password() {
    let mut details = Connection::new(Engine::MySql, "127.0.0.1");
    details.sign_in = SignInMethod::Windows;
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::InvalidDetails);
    assert_eq!(error.message, "Windows sign-in is not supported for MySQL");
    assert!(!format!("{error:?}").contains("invented-secret"));
}

#[tokio::test]
async fn mssql_closed_port_returns_unreachable_without_disclosing_password() {
    let (_socket, port) = closed_port();
    let mut details = Connection::new(Engine::SqlServer, "127.0.0.1");
    details.port = Some(port);
    details.user = Some("dbide_test".into());
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::Unreachable);
    assert_eq!(
        error.message,
        format!("Can't reach 127.0.0.1:{port}: the connection was refused.")
    );
    assert!(!format!("{error:?}").contains("invented-secret"));
}

#[tokio::test]
async fn pg_closed_port_returns_unreachable_without_disclosing_password() {
    let (_socket, port) = closed_port();
    let mut details = Connection::new(Engine::PostgreSql, "127.0.0.1");
    details.port = Some(port);
    details.user = Some("dbide_test".into());
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::Unreachable);
    assert_eq!(
        error.message,
        format!("Can't reach 127.0.0.1:{port}: the connection was refused.")
    );
    assert!(!format!("{error:?}").contains("invented-secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_wrong_password_returns_bad_credentials_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::mssql::Mssql::start)
        .await
        .unwrap()
        .unwrap();
    let details = details(
        Engine::SqlServer,
        database.host(),
        database.port(),
        database.user(),
        database.database(),
    );
    let error = connect(&details, Some("invented-wrong-password"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::BadCredentials);
    assert_eq!(
        error.message,
        "Sign-in failed: the user name or password is wrong."
    );
    assert!(!format!("{error:?}").contains("invented-wrong-password"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_missing_password_returns_bad_credentials() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::postgres::Postgres::start)
        .await
        .unwrap()
        .unwrap();
    let details = details(
        Engine::PostgreSql,
        database.host(),
        database.port(),
        database.user(),
        database.database(),
    );
    let error = connect(&details, None).await.unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::BadCredentials, "{error:?}");
    assert_eq!(
        error.message,
        "Sign-in failed: the user name or password is wrong."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_wrong_password_returns_bad_credentials_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::postgres::Postgres::start)
        .await
        .unwrap()
        .unwrap();
    let details = details(
        Engine::PostgreSql,
        database.host(),
        database.port(),
        database.user(),
        database.database(),
    );
    let error = connect(&details, Some("invented-wrong-password"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::BadCredentials);
    assert_eq!(
        error.message,
        "Sign-in failed: the user name or password is wrong."
    );
    assert!(!format!("{error:?}").contains("invented-wrong-password"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_wrong_password_returns_bad_credentials_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::mysql::MySql::start)
        .await
        .unwrap()
        .unwrap();
    let details = details(
        Engine::MySql,
        database.host(),
        database.port(),
        database.user(),
        database.database(),
    );
    let error = connect(&details, Some("invented-wrong-password"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::BadCredentials);
    assert_eq!(
        error.message,
        "Sign-in failed: the user name or password is wrong."
    );
    assert!(!format!("{error:?}").contains("invented-wrong-password"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mssql_missing_database_returns_database_unavailable_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::mssql::Mssql::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = details(
        Engine::SqlServer,
        database.host(),
        database.port(),
        database.user(),
        "dbide_connect_missing",
    );
    details.sign_in = SignInMethod::Windows;
    let error = connect(&details, Some(database.password()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::DatabaseUnavailable);
    assert_eq!(
        error.message,
        "Signed in, but database dbide_connect_missing doesn't exist or this login can't open it."
    );
    assert!(!format!("{error:?}").contains(database.password()));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_missing_database_returns_database_unavailable_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::postgres::Postgres::start)
        .await
        .unwrap()
        .unwrap();
    let details = details(
        Engine::PostgreSql,
        database.host(),
        database.port(),
        database.user(),
        "dbide_connect_missing",
    );
    let error = connect(&details, Some(database.password()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::DatabaseUnavailable);
    assert_eq!(
        error.message,
        "Signed in, but database dbide_connect_missing doesn't exist or this login can't open it."
    );
    assert!(!format!("{error:?}").contains(database.password()));
}

#[tokio::test(flavor = "multi_thread")]
async fn pg_unavailable_default_database_names_the_user() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::postgres::Postgres::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = details(
        Engine::PostgreSql,
        database.host(),
        database.port(),
        database.user(),
        database.database(),
    );
    details.database = None;
    let error = connect(&details, Some(database.password()))
        .await
        .unwrap_err();
    assert_eq!(
        error.kind,
        ConnectErrorKind::DatabaseUnavailable,
        "{error:?}"
    );
    assert_eq!(
        error.message,
        "Signed in, but database dbide_test doesn't exist or this login can't open it."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_missing_database_returns_database_unavailable_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::mysql::MySql::start)
        .await
        .unwrap()
        .unwrap();
    let details = details(
        Engine::MySql,
        database.host(),
        database.port(),
        database.user(),
        "dbide_connect_missing",
    );
    let error = connect(&details, Some(database.password()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::DatabaseUnavailable);
    assert_eq!(
        error.message,
        "Signed in, but database dbide_connect_missing doesn't exist or this login can't open it."
    );
    assert!(!format!("{error:?}").contains(database.password()));
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn pg_rejected_windows_role_returns_windows_sign_in_failed_without_disclosing_password() {
    let database =
        tokio::task::spawn_blocking(dbide_test_servers::postgres::Postgres::start_with_sspi)
            .await
            .unwrap()
            .unwrap();
    let mut details = details(
        Engine::PostgreSql,
        database.host(),
        database.port(),
        "dbide_connect_nonexistent_role",
        database.database(),
    );
    details.sign_in = SignInMethod::Windows;
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::WindowsSignInFailed);
    assert_eq!(
        error.message,
        "Windows sign-in failed: the server didn't accept this Windows account."
    );
    assert!(!format!("{error:?}").contains("invented-secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_untrusted_certificate_returns_tls_without_disclosing_password() {
    let database = tokio::task::spawn_blocking(dbide_test_servers::mysql::MySql::start)
        .await
        .unwrap()
        .unwrap();
    let mut details = details(
        Engine::MySql,
        database.host(),
        database.port(),
        database.user(),
        database.database(),
    );
    details.tls = TlsMode::Mandatory;
    details.trust_server_certificate = false;
    let error = connect(&details, Some(database.password()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::Tls);
    assert_eq!(
        error.message,
        "The server's certificate isn't trusted. Turn on Trust server certificate only if you know this server."
    );
    assert!(!format!("{error:?}").contains(database.password()));
}

#[tokio::test]
async fn silent_endpoint_returns_timeout_without_disclosing_password() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accept = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let mut details = Connection::new(Engine::PostgreSql, "127.0.0.1");
    details.port = Some(port);
    details.user = Some("dbide_test".into());
    details.connect_timeout = 1;
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    accept.abort();
    assert_eq!(error.kind, ConnectErrorKind::Timeout);
    assert_eq!(
        error.message,
        format!("No answer from 127.0.0.1:{port} within 1 s.")
    );
    assert!(!format!("{error:?}").contains("invented-secret"));
}

#[tokio::test]
async fn mysql_closed_port_returns_unreachable_without_disclosing_password() {
    let (_socket, port) = closed_port();
    let mut details = Connection::new(Engine::MySql, "127.0.0.1");
    details.port = Some(port);
    details.user = Some("dbide_test".into());
    let error = connect(&details, Some("invented-secret"))
        .await
        .unwrap_err();
    assert_eq!(error.kind, ConnectErrorKind::Unreachable);
    assert_eq!(
        error.message,
        format!("Can't reach 127.0.0.1:{port}: the connection was refused.")
    );
    assert!(!format!("{error:?}").contains("invented-secret"));
}
