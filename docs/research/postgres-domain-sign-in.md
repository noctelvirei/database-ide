# PostgreSQL sign-in with Kerberos or a Windows domain account

Research for [#15](https://github.com/noctelvirei/database-ide/issues/15), done 2026-11-16 against the PostgreSQL 18 manual, libpq 18.6 source (as vendored in pq-src), crate source downloaded from crates.io, and the rust-postgres issue tracker. It builds on [#5](https://github.com/noctelvirei/database-ide/issues/5) and [postgres-mysql-drivers.md](https://github.com/noctelvirei/database-ide/blob/research/postgres-mysql-drivers/docs/research/postgres-mysql-drivers.md), which chose tokio-postgres 0.7.18 with a `[patch.crates-io]` fork carrying PR #1358.

## Recommendation

Add domain sign-in to the tokio-postgres fork that #5 already requires: cherry-pick [rust-postgres PR #1379](https://github.com/rust-postgres/rust-postgres/pull/1379) next to PR #1358, with one change. For the server's `AuthenticationSSPI` request, use the `Negotiate` SSPI package, as libpq does, instead of `Kerberos`. Do not bring in libpq.

- PR #1379 is a working GSSAPI/SSPI implementation for tokio-postgres. It applies cleanly on top of #1358 to the 0.7.18 source and type-checks on this PC with the MSVC toolchain. The fork stays one fork, with one extra PR in it.
- libpq does SSPI out of the box on Windows, but its only TLS library is OpenSSL. That breaks #5's choice of SChannel, which reads corporate CA certificates from the Windows store, and adds a Perl-driven OpenSSL build. A second driver would also mean a second path for rows, cancel and notices in the engine, kept only for the servers that use domain sign-in.
- The `Negotiate` change matters for testing as well as for SSPI servers. This PC is not joined to a domain, so the only domain-style sign-in the engine's tests can run here is a local PostgreSQL 18 server with `sspi` in `pg_hba.conf`, which falls back to NTLM. A Kerberos-only client cannot complete that exchange.

## Versions checked

| Item | Version | Released | Source |
|---|---|---|---|
| tokio-postgres | 0.7.18 | 2026-06-12 | [rust-postgres/rust-postgres](https://github.com/rust-postgres/rust-postgres) |
| postgres-protocol | 0.6.12 | 2026-06-12 | same |
| PR #1379 (GSSAPI/SSPI) | head on 2026-09-01 | open | [PR #1379](https://github.com/rust-postgres/rust-postgres/pull/1379) |
| pq-sys | 0.7.8 | 2026-10-30 | [sgrif/pq-sys](https://github.com/sgrif/pq-sys) |
| pq-src (bundled libpq) | 0.3.14+libpq-18.6 | 2026-10-30 | same repository |
| libpq (safe wrapper) | 6.0.1 | 2025-10-29 | [sanpii/libpq.rs](https://github.com/sanpii/libpq.rs) |
| libpq-sys (its bindings) | 0.8.0 | 2023-03-12 | same |
| openssl-src | 400.0.2+4.0.3 | 2026-10-01 | [alexcrichton/openssl-src-rs](https://github.com/alexcrichton/openssl-src-rs) |

## Verified facts

### What the server asks for

- `pg_hba.conf` method `gss` makes the server send AuthenticationGSS (code 7); method `sspi` sends AuthenticationSSPI (code 9). The client answers each with GSSResponse messages, and the server replies with AuthenticationGSSContinue (code 8) until it sends AuthenticationOk or an error ([Message Flow](https://www.postgresql.org/docs/18/protocol-flow.html)).
- The server runs SSPI "in `negotiate` mode, which will use Kerberos when possible and automatically fall back to NTLM in other cases", and "an SSPI client can authenticate to an GSSAPI server" ([SSPI Authentication](https://www.postgresql.org/docs/18/sspi-auth.html)).
- The client builds the service principal as `postgres/<host>` by default; `krbsrvname` changes the first part, and Active Directory may need it in upper case (`POSTGRES`) ([libpq connection parameters](https://www.postgresql.org/docs/18/libpq-connect.html)).
- GSSAPI session encryption (`gssencmode`, `hostgssenc` lines in `pg_hba.conf`) is a separate handshake started with GSSENCRequest instead of a StartupMessage, followed by `gss_wrap` framing of all traffic ([Message Flow](https://www.postgresql.org/docs/18/protocol-flow.html)).
- The role name the server checks depends on the server's `include_realm`, `krb_realm`, `pg_ident.conf`, and for SSPI `compat_realm` and `upn_username` ([GSSAPI Authentication](https://www.postgresql.org/docs/18/gssapi-auth.html), [SSPI Authentication](https://www.postgresql.org/docs/18/sspi-auth.html)). The SSPI page notes that "libpq uses the SAM-compatible name if no explicit user name is specified".

### libpq on Windows

- `src/include/port/win32_port.h` says "Always build with SSPI support" and defines `ENABLE_SSPI 1` on every Windows build. `ENABLE_GSS` needs MIT Kerberos at build time ([meson `-Dgssapi`](https://www.postgresql.org/docs/18/install-meson.html)).
- In `fe-auth.c`, a libpq with SSPI and without GSSAPI answers both code 7 and code 9 through SSPI: `pg_SSPI_startup(conn, 0, ...)` for GSS, which acquires the `kerberos` package, and `pg_SSPI_startup(conn, 1, ...)` for SSPI, which acquires `negotiate`. Both pass a null identity, so they use the signed-in Windows account, and both target `krbsrvname/host`. `gsslib` matters only on builds with both libraries ([libpq connection parameters](https://www.postgresql.org/docs/18/libpq-connect.html)).
- libpq's only TLS library is OpenSSL: "The only LIBRARY supported is `openssl`" ([meson `-Dssl`](https://www.postgresql.org/docs/18/install-meson.html)). With `sslrootcert=system`, `fe-secure-openssl.c` calls `SSL_CTX_set_default_verify_paths`, which loads OpenSSL's own default file and directory, steered by `SSL_CERT_FILE` and `SSL_CERT_DIR` ([libpq connection parameters](https://www.postgresql.org/docs/18/libpq-connect.html)).
- In the OpenSSL 4.0.3 source shipped in openssl-src 400.0.2, `X509_STORE_set_default_paths_ex` (`crypto/x509/x509_d2.c`) adds the default file, the default directory and a store lookup with a NULL URI, whose comment says it activates "any default URIs (presently none)". The Windows certificate store loader exists (`providers/implementations/storemgmt/winstore_store.c`) but answers only URIs starting `org.openssl.winstore:`, which libpq never passes. So a libpq built on it does not read the Windows certificate store.

### libpq through Rust

- **pq-sys 0.7.8** has features `bundled` (libpq plus OpenSSL) and `bundled_without_openssl`. Without them it links a system libpq found through `PQ_LIB_DIR`, pkg-config, vcpkg or `pg_config`.
- **pq-src 0.3.14** compiles libpq 18.6 from source with `cc`. Its `build.rs` supports `windows` with `msvc`, compiles `fe-auth.c` on every target, and links `secur32`, `shell32` and `advapi32` on Windows. Its TLS feature, `with-openssl`, depends on `openssl-sys`, which needs either an installed OpenSSL or the `vendored` feature (openssl-src). openssl-src runs OpenSSL's Perl `Configure` script, and on MSVC uses NASM when it finds it and otherwise builds with `no-asm`.
- **Built on this PC.** A test binary depending on `pq-sys = "=0.7.8"` with `bundled_without_openssl` built in 16 s with the MSVC toolchain. The stripped release `.exe` is 409,088 bytes, reports `PQlibVersion()` 180600, and contains `AcquireCredentialsHandle`, `InitializeSecurityContextA` and the `negotiate` and `kerberos` package names. That build has no TLS. The OpenSSL variant was not built.
- **Bindings present** in pq-sys 0.7.8's Windows bindings: `PQsetSingleRowMode`, `PQsetChunkedRowsMode`, `PQcancelCreate`, `PQcancelBlocking`, `PQsetNoticeReceiver`, `PQsendQuery`, `PQftype`, `PQftable`, `PQftablecol`, `PQsocket`, `PQconsumeInput`, `PQisBusy`.
- **Features #5 required, as libpq offers them:**
  - Streaming: `PQsetChunkedRowsMode` returns up to N rows per `PGresult`; `PQsetSingleRowMode` returns one ([Retrieving Query Results in Chunks](https://www.postgresql.org/docs/18/libpq-single-row-mode.html)). Outside pipeline mode, `fe-exec.c` resets the mode only in `PQsendQueryStart`, so one call covers every statement of a multi-statement `PQsendQuery`.
  - Cancel: `PQcancelCreate` plus `PQcancelBlocking` or `PQcancelStart`, usable "in a thread-safe way", with the original connection's `sslmode` and `gssencmode` requirements. `PQgetCancel`/`PQcancel` are deprecated because they send the cancel unencrypted ([Canceling Queries](https://www.postgresql.org/docs/18/libpq-cancel.html)).
  - Notices: `PQsetNoticeReceiver` callback.
  - Batches with type OIDs: each statement of a multi-statement `PQsendQuery` gives its own `PGresult`, with `PQftype` per column, so libpq has no equivalent of tokio-postgres's missing OIDs in simple-query runs.
  - Base table and column: `PQftable` and `PQftablecol`.
- **The safe `libpq` crate 6.0.1** wraps chunked mode and the GSS status calls, but depends on `libpq-sys` 0.8.0 (last release 2023-03-12), not on pq-sys, so pq-src's bundled build does not apply to it.

### tokio-postgres and the upstream work

- tokio-postgres 0.7.18 `connect_raw.rs` returns "unsupported authentication method" for `AuthenticationKerberosV5`, `AuthenticationScmCredential`, `AuthenticationGss` and `AuthenticationSspi`. Sign-in happens inside `connect_raw`, and `Config::connect_raw` still runs the handshake itself, so a caller cannot do the GSS exchange from outside the crate.
- postgres-protocol 0.6.12 already parses codes 7, 8 and 9 (`AuthenticationGss`, `AuthenticationGssContinue(body)`, `AuthenticationSspi`), and `frontend::sasl_response` writes the `'p'` message with a raw binary body, which is the GSSResponse framing.
- [Issue #994](https://github.com/rust-postgres/rust-postgres/issues/994) "Support gssapi authentication", opened 2023-02-11, is open with no replies.
- [PR #1379](https://github.com/rust-postgres/rust-postgres/pull/1379) "Add GSSAPI/SSPI (Kerberos) authentication behind a `gss` feature", opened 2026-09-01: +1,194/−10 lines in 9 files, one commit, mergeable, no reviews or comments. In the same period paolobarbolini merged five other PRs (#1363 to #1390, the latest on 2026-10-03).
  - Size: the exchange loop and config in `gss/mod.rs` are 217 lines; the Windows SSPI client in `gss/windows.rs` is 304 lines over `windows-sys` 0.59; the Unix client over `libgssapi` is 290 lines; error classification is 216 lines; `connect_raw.rs` gains 37 lines.
  - Windows uses `AcquireCredentialsHandleW("Kerberos", ...)` for both code 7 and code 9; the file explains that it avoids `Negotiate` so that an NTLM fallback fails early. With no principal configured it passes no identity, so it uses the signed-in account.
  - Its test table lists Windows only with an explicit principal and password against an MIT KDC. Ambient Windows credentials and SSPI servers were not tested.
  - It does not implement `gssencmode`.
  - It adds config `krbsrvname`, `gss_principal`, `gss_password` and `gss_keytab`.
- **Combined with #1358 on this PC.** Both PRs' `tokio-postgres/` changes applied to the 0.7.18 crate source with no conflicts in any `.rs` file. #1379's `Cargo.toml` hunks did not apply to the crates.io-normalised manifest and were added by hand (feature `gss`, `windows-sys` 0.59). `cargo check --features gss --lib` then finished on Windows MSVC with two warnings (an unused import and an unused method in `gss/error.rs`).

### Other options looked at

- **sspi-rs** (`sspi` 0.23.1, Devolutions, MIT OR Apache-2.0) implements NTLM, Kerberos and Negotiate in Rust and can call the native Windows packages. Its README examples pass an explicit user name and password ([README](https://github.com/Devolutions/sspi-rs)). The native Windows API through `windows-sys` already gives single sign-on with the signed-in account, so sspi-rs adds nothing on Windows.
- **winauth** 0.0.5, which tiberius uses for SQL Server integrated sign-in, hard-codes the `NTLM` package. A PostgreSQL `gss` server rejects NTLM.
- **cross-krb5** 0.5.0 and **libgssapi** 0.11.0 bind MIT Kerberos or Heimdal. PR #1379 uses libgssapi for Unix.

## Comparison

| | Fork with #1358 + #1379 | libpq via pq-sys (bundled) | tokio-postgres plus libpq for domain sign-in |
|---|---|---|---|
| `gss` servers (code 7) | Yes, SSPI `Kerberos` | Yes, SSPI `kerberos` | Yes, through libpq |
| `sspi` servers (code 9) | Kerberos only, no NTLM fallback; `Negotiate` would match libpq | Yes, SSPI `negotiate` | Yes, through libpq |
| GSS session encryption | No | No (bundled Windows build has no `ENABLE_GSS`) | No |
| TLS | native-tls (SChannel) as #5 chose | OpenSSL only; Windows store not read | Two TLS stacks |
| Build on this PC | Pure Rust plus `windows-sys` | C via `cc`; TLS needs Perl and OpenSSL source | Both |
| Size | Small (Rust code only, not measured) | 409 KB test binary without TLS; OpenSSL not measured | Both |
| Streaming, cancel, notices, OIDs in batches, base column | As #5 found, OIDs via #1358 | All present, through a blocking C API | Two implementations of each |
| Upstream | Open PR, no review in 2.5 months | Maintained (pq-sys 2026-10-30) | Both |

## Judgement

- **Why not libpq for everything.** It meets every feature #5 asked for, and closes the OID gap without a patch. The cost is elsewhere: OpenSSL in place of SChannel, so corporate CAs would have to be exported to a PEM file or found through `SSL_CERT_FILE`; a Perl-driven OpenSSL build for every developer and CI run; and a blocking C API that needs a thread per connection or a `PQsocket` readiness loop around `PQconsumeInput`. #5's TLS and async choices would both be reversed.
- **Why not both drivers.** Rows, cancel, notices and errors would each be built twice, once over tokio-postgres types and once over `PGresult`, and the libpq path would carry OpenSSL anyway for TLS to domain servers. The engine's tests would also need both paths.
- **The fork.** #5 already accepts a `[patch.crates-io]` fork for #1358. Adding #1379 keeps one driver and one TLS stack. The code to own is about 520 lines on Windows (`gss/mod.rs` and `gss/windows.rs`) plus errors, and none of it touches row handling.
- **Changes to make in the fork, beyond #1379 as submitted:**
  1. Acquire `Negotiate` for `AuthenticationSspi` and keep `Kerberos` for `AuthenticationGss`, as libpq's `fe-auth.c` does. Without this, a Windows server using `sspi` gets a raw Kerberos token where it expects Negotiate, and an NTLM fallback is impossible.
  2. Drop the `[lib] test = true` change if it is not needed, and fix the two warnings.
  3. Keep `gss_password` out of saved connections; domain sign-in in this app means the signed-in account.
- **User name.** tokio-postgres fills a missing `user` from `whoami::username()`. Whether the server accepts that name, `DOMAIN\user`, or `user@REALM` depends on its `include_realm`, `pg_ident.conf`, `compat_realm` and `upn_username`. The connection dialog needs a user field that defaults to the Windows account name and can be overridden.
- **Upstream acceptance** is unknown. Issue #994 has had no maintainer reply in over three years and #1379 has had no review while other PRs merged. Plan for the fork to last; offer the `Negotiate` change on #1379 so the upstream version matches.
- **If the fork becomes a burden,** the fallback is pq-sys with `bundled`, used only for domain-sign-in connections.
- **Testing on Windows without a domain.** A native PostgreSQL 18 server on this PC with `host all all 127.0.0.1/32 sspi` covers the Negotiate path with NTLM and a local account. Kerberos itself needs a domain controller or an MIT KDC, which this PC does not have; those tests have to run against the owner's servers.
- **Encrypted sessions.** Neither route does `gssencmode`. If any of the owner's servers use `hostgssenc` lines, that is a separate question; TLS covers encryption otherwise.

## Not verified

- No sign-in was run against a live server. This PC has no PostgreSQL install and is not joined to a domain.
- Whether a Windows PostgreSQL server using `sspi` accepts the raw Kerberos token PR #1379 sends as submitted.
- That a local PostgreSQL 18 `sspi` server on this PC completes sign-in for a local account over NTLM with a `Negotiate` client, and that a `Kerberos`-only client cannot. This follows from the SSPI manual page, not from a run.
- Whether the patched fork's tests pass; only `cargo check` was run.
- The size and build of pq-sys with `bundled` (OpenSSL), and whether openssl-src builds with Git for Windows' Perl.
- How the upstream maintainer would receive #1379.
- What `whoami::username()` returns compared with libpq's SAM-compatible name for a domain account.
