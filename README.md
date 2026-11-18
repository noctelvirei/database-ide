# Database IDE

A lightweight SQL IDE for SQL Server, PostgreSQL and MySQL, written in Rust: a fast everyday alternative to SSMS for writing queries, reading execution plans and exporting results.

Targets: opens in under 1 second, under 100 MB of memory with every feature loaded, installer under 20 MB.

## Status

In development, with no release yet. The engine is built for all three servers: connections, runs with cancel, results streamed to disk, estimated and actual plans, exports, the danger check and the formatter. The UI has connections, query tabs with run and cancel, and a results grid that handles a million rows. Next: the object explorer, the menu bar, autocomplete, plans, grid editing and an installer.

What v1 will do is in [docs/spec.md](docs/spec.md), the decisions behind it are linked from the [planning map](https://github.com/noctelvirei/database-ide/issues/1), and the vocabulary is in [CONTEXT.md](CONTEXT.md).

## Contributing

Open work is in [Issues](https://github.com/noctelvirei/database-ide/issues); start with a [`good first issue`](https://github.com/noctelvirei/database-ide/labels/good%20first%20issue). [CONTRIBUTING.md](CONTRIBUTING.md) covers building, the checks and the test servers.

## Licence

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
