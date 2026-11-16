# Our own fork of tokio-postgres

PostgreSQL goes through our public fork of `tokio-postgres` (https://github.com/noctelvirei/rust-postgres), used via `[patch.crates-io]`, rather than released `tokio-postgres`, `sqlx` or libpq. The fork carries two open upstream pull requests: #1358, which keeps column type OIDs in multi-statement simple-query batches (an IDE runs whatever batch the user types), and #1379, which adds GSSAPI/SSPI sign-in, switched to the `Negotiate` package so a domain sign-in can fall back to NTLM. The owner has PostgreSQL servers that need domain sign-in.

`sqlx` can't cancel a PostgreSQL query and doesn't return notices. libpq supports domain sign-in but only does TLS through OpenSSL, which doesn't read the Windows certificate store. Upstream has left domain sign-in unanswered since 2023, so we maintain the fork ourselves from here and don't wait for it.

Research: [driver comparison](https://github.com/noctelvirei/database-ide/issues/5), [domain sign-in](https://github.com/noctelvirei/database-ide/issues/15); decision: [Choose the database drivers](https://github.com/noctelvirei/database-ide/issues/9).
