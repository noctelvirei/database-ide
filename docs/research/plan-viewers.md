# Plan viewers Database IDE could reuse

Research for [#6](https://github.com/noctelvirei/database-ide/issues/6). Checked 2026-11-16 against each project's repository, licence file, commit history and issues, and against Microsoft's and PostgreSQL's own documentation. Every claim under "Verified" links to its source. Sections marked "Judgement" are my reading of those facts.

## Answer

- **Tauri 2 (webview).** pev2 can be embedded as-is for PostgreSQL. It is a Vue 3 component under the PostgreSQL licence, with releases every month or two (v1.25.0 on 2026-10-20). For SQL Server, html-query-plan is the only drop-in component, but it renders through the browser's `XSLTProcessor`, which Chromium turns off on Stable from 17 November 2026. WebView2 is Chromium. So it needs an XSLT polyfill or a rewrite of its 793-line stylesheet. vscode-mssql's React Flow viewer is the closest to SSMS and is MIT, but it is part of the extension, not a package.
- **Native Rust toolkit (egui, Slint, gpui).** Nothing can be embedded. The code worth porting is vscode-mssql's TypeScript plan model and layout for SQL Server, PerformanceStudio's C# parser and 30 analysis rules, and pev2's PostgreSQL metrics. The SSMS-style operator icons in vscode-mssql are MIT and work in either toolkit.
- **Engine handoff.** The engine should return each plan both ways: the server's raw payload, kept byte for byte so a `.sqlplan` file can be saved, and a parsed operator tree, one per statement. Webview viewers take the raw string. A native redraw needs the tree.

## Viewers at a glance

"Last commit" is the newest commit on the default branch, not GitHub's `pushed_at`.

| Viewer | Engine | What it renders | Technology | Webview (Tauri) | Native toolkit | Licence | Last commit |
|---|---|---|---|---|---|---|---|
| [html-query-plan](https://github.com/JustinPealing/html-query-plan) | SQL Server | Estimated and actual plans, SSMS-like tree, tooltips, warning/parallel/batch badges | TypeScript plus an XSLT 1.0 stylesheet | At risk: depends on browser XSLT | Reference only | MIT | 2021-03-02 (v2.6.1) |
| [vscode-mssql plan view](https://github.com/microsoft/vscode-mssql/tree/main/extensions/mssql/src/webviews/pages/ExecutionPlan) | SQL Server | Estimated, actual and live plans, properties pane, find node, expensive-operator highlight, plan comparison | React, Fluent UI, `@xyflow/react` | Yes, by extracting code from the extension | Port the model and layout | MIT | 2026-10-26 |
| [azdataGraph](https://github.com/microsoft/azdataGraph) | SQL Server (generic graph) | Graph canvas derived from mxGraph | JavaScript | Yes, but only a graph canvas | No | Apache-2.0 | 2026-07-28 (0.0.140) |
| [PerformanceStudio](https://github.com/erikdarlingdata/PerformanceStudio) | SQL Server | Graphical plan with SSMS icons, 30 analysis rules, missing indexes, plan comparison, Query Store | C#, .NET 10, Avalonia | No | Reference (port parser and rules) | MIT | 2026-11-16 |
| [pev2](https://github.com/dalibo/pev2) | PostgreSQL | JSON or text `EXPLAIN`, plan tree, grid, estimate-factor and slow-node badges, buffers, I/O | Vue 3, Bootstrap 5, d3 | Yes, as a component | Reference | PostgreSQL | 2026-11-03 (v1.25.0 on 2026-10-20) |
| [pev](https://github.com/AlexTatiyants/pev) | PostgreSQL | Tree, flame chart, outline, plain-language findings | React, Vite (an app, not a package) | Only by copying `src/lib` and views | Reference (`src/lib`) | MIT | 2026-10-08 (first since 2016) |
| [Azure Data Studio](https://github.com/microsoft/azuredatastudio) | SQL Server | Was the azdataGraph host | TypeScript | No: archived | No | MIT | 2026-02-28, archived |

Out of scope for reuse, one line each: [pgAdmin 4](https://github.com/pgadmin-org/pgadmin4) and [DBeaver](https://github.com/dbeaver/dbeaver) both draw plans, but their viewers are built into those apps rather than published as components. [pg_flame](https://github.com/mgartner/pg_flame) (Apache-2.0) has not had a commit since 2020-01-13. [flame-explain](https://github.com/felixge/flame-explain) is AGPL-3.0 and has not had a commit since 2020. A crates.io search for "explain plan" and "showplan" found no Rust plan parser or viewer.

## SQL Server viewers

### html-query-plan

Verified:
- `QP.showPlan(container, xml)` takes the plan XML as a string. Under the hood it is an XSLT 1.0 stylesheet, and JavaScript draws the connecting lines ([README](https://github.com/JustinPealing/html-query-plan#readme)).
- It renders through `new XSLTProcessor()` in the page, with an `ActiveXObject` fallback for old IE ([src/xml.ts](https://github.com/JustinPealing/html-query-plan/blob/master/src/xml.ts)).
- [qp.xslt](https://github.com/JustinPealing/html-query-plan/blob/master/src/qp.xslt) is 793 lines. It reads `ActualRows`, `ActualExecutions`, `RunTimeCountersPerThread`, `EstimateRows`, `Warnings`, batch mode and adaptive joins, so it renders both estimated and actual plans with warning, parallel and batch overlays.
- MIT ([LICENSE](https://github.com/JustinPealing/html-query-plan/blob/master/LICENSE)). It was sponsored by Brent Ozar Unlimited for Paste The Plan.
- The last commit was 2021-03-02 ("Bump version number for release", v2.6.1). Pull requests #115 and #116 (2024) are unmerged. Issue [#118](https://github.com/JustinPealing/html-query-plan/issues/118) (2026-09-20) reports that Chrome drops XSLT in November 2026 and suggests `xslt_polyfill`. Nobody has answered it.
- Chrome's schedule: XSLT stops working on Stable in Chrome 158, on 17 November 2026. An enterprise policy and an origin trial keep it alive until Chrome 176 (17 August 2027) ([Chrome for Developers](https://developer.chrome.com/docs/web-platform/deprecating-xslt)).
- Edge has an `XSLTEnabled` policy, supported from Edge 147 on Windows and macOS, described as temporary ([Microsoft Learn](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-policies/XSLTEnabled)). That page names the browser registry path only, not WebView2's.

Judgement:
- Tauri on Windows uses WebView2, which follows Chromium, so html-query-plan will stop rendering there. Reusing it means adding a polyfill or running the XSLT somewhere other than the browser, on top of a project with no commits in five years. Its stylesheet is still a compact, readable map of which showplan attributes matter for each operator.

### vscode-mssql execution plan view

Verified:
- The repository is MIT ([LICENSE.txt](https://github.com/microsoft/vscode-mssql/blob/main/LICENSE.txt), [extensions/mssql/LICENSE.txt](https://github.com/microsoft/vscode-mssql/blob/main/extensions/mssql/LICENSE.txt)). GitHub's API reports the licence as NOASSERTION because the file wraps the MIT text in banner lines.
- The plan view is a React webview drawn with `@xyflow/react` (React Flow) and Fluent UI ([reactFlowExecutionPlan.tsx](https://github.com/microsoft/vscode-mssql/blob/main/extensions/mssql/src/webviews/pages/ExecutionPlan/reactFlowExecutionPlan.tsx), [extensions/mssql/package.json](https://github.com/microsoft/vscode-mssql/blob/main/extensions/mssql/package.json)). No package.json in the repository lists azdataGraph any more. [executionPlanModel.ts](https://github.com/microsoft/vscode-mssql/blob/main/extensions/mssql/src/webviews/pages/ExecutionPlan/executionPlanModel.ts) says it "Ports azdataGraph's execution-plan-specific layout".
- It does not parse showplan XML itself. It receives `ExecutionPlanGraph` objects (a `root` node tree, the query, recommendations, live statistics) plus the raw `xmlPlans` from SQL Tools Service ([sharedInterfaces/executionPlan.ts](https://github.com/microsoft/vscode-mssql/blob/main/extensions/mssql/src/sharedInterfaces/executionPlan.ts)).
- The parsing happens in [microsoft/sqltoolsservice](https://github.com/microsoft/sqltoolsservice/tree/main/src/Microsoft.SqlTools.ServiceLayer/ExecutionPlan/ShowPlan) (C#, MIT). It contains node builders for estimated and actual plans and an XSD-generated `showplanxml.cs`.
- Recent work: live query statistics (2026-10-05), plan comparison (2026-10-07), Query Studio (2026-10-26). There are 120 operator icons under [ExecutionPlan/icons](https://github.com/microsoft/vscode-mssql/tree/main/extensions/mssql/src/webviews/pages/ExecutionPlan/icons). PerformanceStudio's [THIRD_PARTY_NOTICES.md](https://github.com/erikdarlingdata/PerformanceStudio/blob/main/THIRD_PARTY_NOTICES.md) credits these icons to vscode-mssql under MIT.

Judgement:
- It is the closest open-source match to SSMS's graphical plan, and it is actively maintained. It is not published as a package, so reusing it means copying the `ExecutionPlan` folder and its shared webview helpers into a Tauri frontend and keeping up with upstream by hand. Its `ExecutionPlanGraph`/`ExecutionPlanNode` contract is a ready-made model for what our engine should produce.

### azdataGraph

Verified:
- A derivative of mxGraph for Azure Data Studio and mssql for VS Code ([README](https://github.com/microsoft/azdataGraph#readme)). Its LICENSE text and package.json both say Apache-2.0, while GitHub's API reports NOASSERTION. Latest release 0.0.140 on 2026-07-28.
- Azure Data Studio was archived on 2026-02-28 with a retirement notice ([repo](https://github.com/microsoft/azuredatastudio)).

Judgement:
- Its only known consumers are gone or have moved away from it, so it adds a dependency for nothing that React Flow plus vscode-mssql's ported layout do not already cover.

### PerformanceStudio (Erik Darling)

Verified:
- A cross-platform SQL Server plan analyser: an Avalonia GUI with SSMS-style icons, a `planview` CLI that outputs JSON or text, an SSMS extension and an MCP server ([README](https://github.com/erikdarlingdata/PerformanceStudio#readme)).
- `PlanViewer.Core` holds the parser, 30 analysis rules and the layout, and has 37 xUnit tests on real `.sqlplan` fixtures. MIT ([LICENSE](https://github.com/erikdarlingdata/PerformanceStudio/blob/main/LICENSE)). Last commit 2026-11-16. Its sibling [PerformanceMonitor](https://github.com/erikdarlingdata/PerformanceMonitor) (MIT) also has a plan viewer and covers PostgreSQL.

Judgement:
- It cannot be embedded in a Rust app without shipping .NET. It is the best reference for the analysis side: which warnings to raise, row-estimate mismatches, missing indexes, and comparing plans. Its rules are the most direct source for a DBA-facing "what is wrong with this plan" panel.

## PostgreSQL viewers

### pev2 (Dalibo)

Verified:
- A Vue 3 component (`<pev2 :plan-source :plan-query>`) that needs Bootstrap 5 CSS. It is available from npm or as a single offline `pev2.html` file ([README](https://github.com/dalibo/pev2#readme), [package.json](https://github.com/dalibo/pev2/blob/master/package.json): vue ^3.5, d3 ^7.9, bootstrap ^5.3).
- It takes JSON or text `EXPLAIN` output. Its tests cover parallel plans, CTEs, plans without `ANALYZE` and plans without costs ([src/services/\_\_tests\_\_](https://github.com/dalibo/pev2/tree/master/src/services/__tests__)). Metrics include `estimate_factor` and exclusive cost, duration and I/O ([src/enums.ts](https://github.com/dalibo/pev2/blob/master/src/enums.ts)). Node badges flag slow nodes, high cost, row-estimate errors and high heap fetches ([NodeBadges.vue](https://github.com/dalibo/pev2/blob/master/src/components/NodeBadges.vue)).
- PostgreSQL licence ([LICENSE](https://github.com/dalibo/pev2/blob/master/LICENSE)). It has 71 open issues. v1.24.0 came out on 2026-09-10 and v1.25.0 on 2026-10-20, and issues were active up to 2026-11-14 (#980, a text-parser bug).

Judgement:
- It is the one viewer we could drop into a Tauri webview unchanged, with a Vue island inside whatever frontend framework we pick. It draws a PostgreSQL-style plan, not an SSMS-style one, which matches what PostgreSQL DBAs expect.

### pev (Alex Tatiyants)

Verified:
- The original pev had no commits from 2016 until a full rebuild on React, Vite and TypeScript, pushed by Alex Tatiyants on 2026-10-08 ([commits](https://github.com/AlexTatiyants/pev/commits)). pev2's README describes pev as abandoned, which was written before that rebuild.
- `package.json` is `"private": true`, so it is an app, not an npm package. Its parsing and analysis live in `src/lib` (`parse.ts`, `textParser.ts`, `analyze.ts`, `insights.ts`, `layout.ts`, `flame.ts`) ([README](https://github.com/AlexTatiyants/pev#readme)). MIT.

Judgement:
- It has one commit of new history, so it is too soon to depend on. `src/lib` is a small, MIT-licensed reference for plain-language findings and a flame view.

## MySQL

[MySQL Workbench](https://github.com/mysql/mysql-workbench) has a visual explain, but it is GPLv2 with Oracle's additional permissions ([License.txt](https://github.com/mysql/mysql-workbench/blob/8.0/License.txt)), so reusing its code would put Database IDE under the GPL. In MySQL 8.4, `EXPLAIN FORMAT=JSON` gives an estimated plan, but `EXPLAIN ANALYZE` accepts only `FORMAT=TREE` and rejects JSON with error 1235 ([MySQL 8.4 manual](https://dev.mysql.com/doc/refman/8.4/en/explain.html)). So a MySQL actual plan arrives as indented text. I did not look further at MySQL.

## The SQL Server showplan XSD

Verified:
- Microsoft publishes it at [schemas.microsoft.com/sqlserver/2004/07/showplan](https://schemas.microsoft.com/sqlserver/2004/07/showplan/). The index page, dated November 2022, links one file per release: sql2005, sql2005sp2, sql2008, sql2012, sql2014, sql2014sp2, sql2016, sql2016sp1, sql2017, sql2019, sql2022. The namespace has stayed `http://schemas.microsoft.com/sqlserver/2004/07/showplan` throughout.
- Schema `version` attributes: 2012 is 1.2, 2014 SP2 is 1.3.1, 2016 is 1.5, 2017 is 1.6, 2019 is 1.539, and 2022 is 1.571 (2022's annotation says "Last updated: 10/10/2022").
- There is no `sql2025/showplanxml.xsd` (HTTP 404). The unversioned `showplanxml.xsd` link returns a "resource ... has been removed" message.
- The number of named types grows with each release: 121 complex types in 2012 and 160 in 2022. No named complex or simple type in 2012 or 2019 is missing from 2022. 2019 to 2022 added six: DOPFeedbackInfoType, DispatcherType, ExternalDistributedComputationType, OptimizationReplayType, ParameterSensitivePredicateType and ResourceEstimateType. I compared type names only, not elements or attributes inside the types.
- sqltoolsservice keeps its own copy ([showplanxml.xsd](https://github.com/microsoft/sqltoolsservice/blob/main/src/Microsoft.SqlTools.ServiceLayer/ExecutionPlan/ShowPlan/showplanxml.xsd)) at version 1.550. It was patched on 2026-03-19 for SQL Server 2025 JSON indexes (#2632), adds PredictFunctionType, and lacks three of the 2022 types (DOPFeedbackInfoType, OptimizationReplayType, ResourceEstimateType). So no single published schema covers SQL Server 2025.
- The XSD's header says Microsoft provides it "AS IS" and makes no warranty against infringement, and that users must seek licences for any intellectual property themselves.
- PostgreSQL publishes no schema for `EXPLAIN (FORMAT JSON)`. The manual says only that non-text formats carry the same information as text and are "easier for programs to parse" ([sql-explain](https://www.postgresql.org/docs/current/sql-explain.html)).

Judgement:
- The engine should parse showplan XML leniently: read the elements and attributes it knows and ignore the rest, rather than validating against an XSD or generating strict types from one. New releases add types, and there is no 2025 schema to validate against.
- Do not commit Microsoft's XSD into this repository, because it may be released as open source. Link to it, and fetch it in tests if needed.

## What the engine should hand the UI

Judgement, built on the facts above:

- **Raw payload, kept byte for byte.** CONTEXT.md defines a plan file as a `.sqlplan` the IDE saves as well as opens. The engine therefore has to keep the server's showplan XML exactly as received, and PostgreSQL's JSON as received. html-query-plan (`QP.showPlan(container, xml)`) and pev2 (`plan-source`) both take that string directly.
- **Parsed operator tree, one per statement.** A batch or procedure returns several statements, each with its own plan, so the result is a list. Each node needs: operator name (physical and logical), its icon key, estimated and actual rows, executions, estimated cost and its share of the total, actual elapsed and CPU time where present, warnings, parallel and batch-mode flags, and a property bag for everything else so the UI can show a full properties pane without the engine knowing every attribute. Edges carry row counts so the UI can draw line thickness. vscode-mssql's `ExecutionPlanGraph`/`ExecutionPlanNode` ([sharedInterfaces/executionPlan.ts](https://github.com/microsoft/vscode-mssql/blob/main/extensions/mssql/src/sharedInterfaces/executionPlan.ts)) is a worked example of this shape, with `xmlPlans` and `executionPlanGraphs[]` side by side.
- **Analysis as a separate step.** Warnings such as row-estimate mismatches, spills and missing indexes are computed from the tree (PerformanceStudio's rules, pev2's estimate factor). They belong in the engine, so they can be tested without a window, and they attach to node IDs in the tree.
- Under Tauri, the UI may use the raw string with an embedded viewer. Under a native toolkit, it uses only the tree. Producing both keeps the toolkit decision open.

## Not verified

- Whether WebView2 disables XSLT on the same date as Chrome, and whether a registry or `XSLTEnabled`-style policy applies to WebView2. Microsoft's policy page lists only the browser path. A secondary source mentions a WebView2 path, but I could not confirm it. Tauri's macOS (WKWebView) and Linux (WebKitGTK) webviews were not checked for XSLT support either.
- How closely each viewer's look matches SSMS. I did not run any of them; the comparisons rest on READMEs and source.
- Whether html-query-plan works with `xslt_polyfill`, and what that adds to the bundle size.
- Element- and attribute-level differences between XSD versions, and what SQL Server 2025 adds beyond the JSON-index patch.
- Hosting a separate webview just for the plan pane inside a native egui, Slint or gpui window (for example with wry). This seems possible, but I did not test it.
