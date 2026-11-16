# Can a local Hyper-V lab test Kerberos sign-in?

Research for [#18](https://github.com/noctelvirei/database-ide/issues/18), 2026-11-16. Paper research only: nothing was installed, no VM was made, no setting was changed. Related decisions: [#4](https://github.com/noctelvirei/database-ide/issues/4) (SQL Server through `mssql-tds`, SSPI `Negotiate`), [#15](https://github.com/noctelvirei/database-ide/issues/15) (PostgreSQL through a tokio-postgres fork, SSPI `Negotiate` for `sspi` servers and `Kerberos` for `gss` servers), [#16](https://github.com/noctelvirei/database-ide/issues/16) (test servers; the baseline is a manual checklist against domain servers at work).

## Answer

Probably yes, with one Windows Server 2025 evaluation VM that is the domain controller for `lab.test` and also runs SQL Server 2025 Developer and PostgreSQL 18. The test harness starts the engine's Kerberos tests with `CreateProcessWithLogonW(LOGON_NETCREDENTIALS_ONLY)` as `tester@lab.test`. The PC needs three changes, all reversible: Hyper-V enabled, an internal virtual switch, and one DNS policy (NRPT) rule that sends `lab.test` lookups to the DC. A fourth is optional: the owner in the Hyper-V Administrators group, so tests can start the VM without an elevated shell.

"Probably" because one link is not documented in so many words: that the Kerberos package, inside a `/netonly` logon session on a PC that is not on a domain, finds the KDC through DNS and gets a ticket. The first build step is a cheap gate that settles it before SQL Server or PostgreSQL go in (see [Build steps](#build-steps)).

Proving Kerberos rather than an NTLM fallback is easy for SQL Server (`auth_scheme`) and not possible from inside a Windows PostgreSQL `sspi` server. The lab solves both the same way: it denies incoming NTLM on the VM, so any sign-in that succeeds was Kerberos.

## Verified facts

### Hyper-V on this PC

- Hyper-V runs on Windows 11 Pro or Enterprise with a SLAT-capable 64-bit CPU and at least 4 GB of memory. It is enabled with `Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All` as administrator, followed by a restart. ([Install Hyper-V](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/get-started/install-hyper-v))
- Read-only checks on this PC (Windows 11 Pro, not on a domain, 32 GB RAM, 32 logical processors): a hypervisor is already running (`HypervisorPresent = True`, the `HvHost` and `vmcompute` services run), but the Hyper-V management service `vmms` and the Hyper-V PowerShell module are absent, so the Hyper-V feature itself is not enabled. `Get-WindowsOptionalFeature` refused to run without elevation. `VirtualizationFirmwareEnabled` reads `False`, which is what Windows reports once a hypervisor already owns the CPU's virtualisation; it does not mean virtualisation is off in firmware. The Secondary Logon service (`seclogon`), which `runas` needs, is running.
- An internal virtual switch connects the VMs with each other and with the host; a private switch connects VMs only. ([Plan Hyper-V networking](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/plan/plan-hyper-v-networking-in-windows-server)) `New-VMSwitch -SwitchType Internal` creates one. ([New-VMSwitch](https://learn.microsoft.com/en-us/powershell/module/hyper-v/new-vmswitch))

### Signing in as a lab account from a PC off the domain

- `runas /netonly` means "the user information specified is for remote access only". Its syntax is `[{/netonly | /savecred}]`: the two cannot be combined, so `runas /netonly` always prompts for the password. The documented example uses it for "a server in another forest". ([runas](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-server-2012-r2-and-2012/cc771525(v=ws.11)))
- `CreateProcessWithLogonW` with `LOGON_NETCREDENTIALS_ONLY` gives the new process the caller's token locally and the given credentials on the network, in a new LSA logon session. It is meant for "inter-domain scenarios where there is no trust relationship". It takes the password in clear text and "does not validate the specified credentials", so a wrong password shows up only when the process connects. With a UPN user name (`user@dns.domain`) the domain parameter must be NULL. It cannot be called from a LocalSystem process. ([CreateProcessWithLogonW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createprocesswithlogonw))
- The same logon type exists in process as `LogonUser(..., LOGON32_LOGON_NEW_CREDENTIALS, LOGON32_PROVIDER_WINNT50, ...)`: it clones the caller's token and uses the new credentials for outbound connections. ([LogonUser](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-logonusera))

### Finding the KDC

- The Kerberos SSP finds the KDC for the user's account domain through the domain controller locator, which only finds KDCs in Active Directory domains. For other realms the KDC names must be in the registry. ([Finding the KDC](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-2000-server/cc961977(v=technet.10)), archived Windows 2000 page)
- DNS discovery uses SRV records such as `_kerberos._tcp.<domain>` and `_ldap._tcp.dc._msdcs.<domain>`. ([MS-ADTS 6.3.2.3 SRV records](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-adts/c1987d42-1847-4cc9-acf7-aab2136d6952)) A hosts file cannot hold SRV records.
- `ksetup /addkdc <REALM> <kdc>` stores the mapping under `HKLM\System\CurrentControlSet\Control\LSA\Kerberos\Domains` and needs a restart before it is used. ([ksetup addkdc](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/ksetup-addkdc)) This PC has no such mappings today.
- `Add-DnsClientNrptRule -Namespace <ns> -NameServers <ip>` adds a Name Resolution Policy Table rule that sends that namespace's DNS queries to the given server. Without `-GpoName` the rule is added for the local computer. `Remove-DnsClientNrptRule` removes it. ([Add-DnsClientNrptRule](https://learn.microsoft.com/en-us/powershell/module/dnsclient/add-dnsclientnrptrule))
- By default Windows does not try Kerberos when the host name is an IP address and falls back to NTLM. Allowing it needs the `TryIPSPN` value under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\Kerberos\Parameters` on every client. ([Kerberos for IP address](https://learn.microsoft.com/en-us/windows-server/security/kerberos/configuring-kerberos-over-ip))
- Kerberos rejects a client whose clock differs from the DC's by more than the tolerance, 5 minutes by default on a DC. ([Maximum tolerance for clock synchronization](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/maximum-tolerance-for-computer-clock-synchronization))

### Proving Kerberos, not NTLM

**SQL Server**
- `sys.dm_exec_connections.auth_scheme` is `KERBEROS` for a Kerberos connection. If no ticket can be obtained for the SPN, authentication silently uses NTLM. Reading the view needs `VIEW SERVER PERFORMANCE STATE` on SQL Server 2022 and later. ([Register an SPN](https://learn.microsoft.com/en-us/sql/database-engine/configure-windows/register-a-service-principal-name-for-kerberos-connections), [sys.dm_exec_connections](https://learn.microsoft.com/en-us/sql/relational-databases/system-dynamic-management-views/sys-dm-exec-connections-transact-sql))
- The SPN for TCP is `MSSQLSvc/<FQDN>:<port>`; the default instance also uses `MSSQLSvc/<FQDN>`. A service account without rights to write its own SPN needs them registered with `setspn -S`. (Same SPN page.)

**PostgreSQL**
- `pg_stat_gssapi` reports `gss_authenticated` and `principal` for GSSAPI connections only; its documentation does not mention SSPI. ([PG 18 monitoring](https://www.postgresql.org/docs/18/monitoring-stats.html))
- A Windows server using the `sspi` method uses the `negotiate` package: "Kerberos when possible and automatically fall back to NTLM in other cases". ([PG 18 SSPI](https://www.postgresql.org/docs/18/sspi-auth.html)) The PostgreSQL 18 source confirms it: `pg_SSPI_recvauth` calls `AcquireCredentialsHandle(NULL, "negotiate", SECPKG_CRED_INBOUND, ...)`. ([auth.c, REL_18_STABLE](https://github.com/postgres/postgres/blob/REL_18_STABLE/src/backend/libpq/auth.c))
- `log_connections = 'authentication'` logs `connection authenticated: identity="..." method=sspi` (same source file; [PG 18 logging](https://www.postgresql.org/docs/18/runtime-config-logging.html)). The method is the `pg_hba.conf` method, not the SSPI package, so the log cannot tell Kerberos from NTLM either.
- So on a Windows PostgreSQL server with `sspi`, nothing inside PostgreSQL shows which protocol was used.
- The `gss` method speaks Kerberos only and fills `pg_stat_gssapi`. It needs a keytab (`krb_server_keyfile`) and a server built with GSSAPI. The service principal is `postgres/<fqdn>@REALM` by default. ([PG 18 GSSAPI](https://www.postgresql.org/docs/18/gssapi-auth.html))

**Either engine**
- "Network security: Restrict NTLM: Incoming NTLM traffic" set to "Deny all accounts" makes the server refuse every incoming NTLM sign-in and log the block in `Applications and Services Logs\Microsoft\Windows\NTLM`. No restart is needed. ([Restrict NTLM incoming](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/network-security-restrict-ntlm-incoming-ntlm-traffic)) Set on the lab VM, it turns any successful sign-in into proof of Kerberos.

### The VM: Windows Server 2025 evaluation

- The evaluation is free, comes as an ISO or a VHD (English only), covers Standard and Datacenter, and "expires in 180 days". It must be activated over the internet within the first 10 days or it shuts down. ([Evaluation Center](https://www.microsoft.com/en-us/evalcenter/evaluate-windows-server-2025))
- An evaluation domain controller cannot be converted to retail. ([Convert editions](https://learn.microsoft.com/en-us/windows-server/get-started/upgrade-conversion-options))
- Minimums: 2 GB RAM (Server Core and Desktop Experience; 4 GB recommended for Desktop Experience), at least 1,280 MB during setup in a VM, 32 GB system disk; Server Core is about 4 GB smaller. ([Hardware requirements](https://learn.microsoft.com/en-us/windows-server/get-started/hardware-requirements))
- `Install-ADDSForest -DomainName <fqdn> -SafeModeAdministratorPassword <securestring> -Force` creates a forest without prompts; DNS Server is installed by default. ([Install-ADDSForest](https://learn.microsoft.com/en-us/powershell/module/addsdeployment/install-addsforest))

### SQL Server 2025 on that VM

- SQL Server 2025 supports Windows Server 2025, including Server Core. Minimum 1 GB memory, at least 4 GB recommended; at least 6 GB free disk for setup; the Database Engine needs 1,480 MB. ([SQL Server 2025 requirements](https://learn.microsoft.com/en-us/sql/sql-server/install/hardware-and-software-requirements-for-installing-sql-server-2025))
- On a domain controller: "For security reasons, don't install SQL Server on a domain controller." Setup does not block it, but the services cannot run under a local service account, and the machine cannot change between DC and member while SQL Server is installed. (Same page.)

### Samba AD instead of Windows

- Samba 4.0 and later can be an AD DC, with forest functional level Windows Server 2008 R2; its internal DNS is the default. The Samba team advises against using the DC for anything else, such as a file server. ([Samba wiki: Setting up Samba as an AD DC](https://wiki.samba.org/index.php/Setting_up_Samba_as_an_Active_Directory_Domain_Controller))

## Judgement

### Which variant

Windows Server 2025 evaluation, one VM, Server Core. Samba would replace only the DC: SQL Server 2025 on Windows still needs a Windows machine joined to the domain (a second VM and a second evaluation licence), or SQL Server on Linux, which is a different server from the one the IDE meets at work. The domains users sign into at work are Active Directory. Samba's one advantage is PostgreSQL with the `gss` method on Linux, where `pg_stat_gssapi` proves Kerberos directly; the NTLM block below gives the same proof on Windows.

DC, SQL Server and PostgreSQL on one VM goes against Microsoft's advice for production. For a throwaway lab it saves a VM, a domain join and about 4 GB of memory. The costs: SQL Server runs as a domain account (`svc-sql`), and the SPNs are registered by the build script.

### How the tests run

- The harness launches the Kerberos test binary with `CreateProcessWithLogonW(LOGON_NETCREDENTIALS_ONLY)` and the user name in UPN form, `tester@lab.test`. The password is a lab-only secret read from a git-ignored file or from the user's Credential Manager (`CredReadW`); neither changes a machine setting. `runas /netonly` stays a manual fallback, since it always prompts.
- Use the UPN or `LAB.TEST\tester`, not `LAB\tester`. Nothing validates the credentials at launch. If the PC cannot map a NetBIOS domain name to a KDC, Negotiate can quietly fall back to NTLM, which the server passes through to the DC. The NTLM block catches this case.
- Connect by FQDN (`dc1.lab.test`), never by IP, so `TryIPSPN` is not needed.
- Two engine-level checks per run: SQL Server reports `auth_scheme = 'KERBEROS'` (the build script grants `VIEW SERVER PERFORMANCE STATE` to `LAB\tester`). Both engines refuse the same sign-in by IP address, because NTLM is denied (negative test).
- A client-side check that needs no server access: run `klist` in the same netonly process after connecting; the cache should hold tickets for `MSSQLSvc/dc1.lab.test:1433` and `POSTGRES/dc1.lab.test`. Optionally the drivers could report the negotiated package through `QueryContextAttributes(SECPKG_ATTR_NEGOTIATION_INFO)`. That is a driver change, not needed for the lab.
- `gss` servers (#15's other path) need a PostgreSQL build with GSSAPI and a keytab. Whether the Windows PostgreSQL 18 binaries include GSSAPI was not checked. If they do, a second PostgreSQL instance on the same VM with `gss` and a keytab exported by `ktpass` covers that path; if not, it stays on the manual checklist.

### Settings on this PC

| Change | Why | Admin | Restart | Undo |
|---|---|---|---|---|
| Enable Hyper-V | Run the VM | Yes | Yes | `Disable-WindowsOptionalFeature` |
| Internal switch `dbide-lab`, host adapter with a static IP in the lab subnet | Host and VM talk privately | Yes | No (estimate) | `Remove-VMSwitch` |
| One NRPT rule: `.lab.test` → DC's IP | Find the KDC (SRV records) and the servers by name | Yes | No (estimate) | `Remove-DnsClientNrptRule` |
| Optional: add the owner's account to the local Hyper-V Administrators group | The test harness can start and stop the VM and use PowerShell Direct without an elevated shell | Yes | Sign out and in | Remove from the group |

Not needed: domain join, `ksetup` realm mappings (HKLM plus restart), hosts entries (cannot serve SRV records), `TryIPSPN`, WinRM `TrustedHosts`, or any change to this PC's Kerberos or NTLM policy. The NTLM block lives inside the VM.

Internet for the VM: the evaluation must activate online within 10 days, and an internal switch has no internet. Give the VM a second network adapter on Hyper-V's built-in Default Switch, a NAT switch that Windows 11 provides once Hyper-V is on (not checked against a Microsoft page here). A `NetNat` on the internal switch would work too but is another host change.

### Cost (estimates, not measured)

- **Disk:** Windows Server ISO about 5–6 GB, kept for rebuilds. VM disk as a dynamic VHDX: about 15–25 GB used with Server Core, AD DS, SQL Server Database Engine and PostgreSQL binaries. Allow 40 GB on the data drive.
- **Memory:** 4 GB for the VM while it runs (2 GB Windows minimum, SQL Server capped at about 1.5 GB with `max server memory`, PostgreSQL small). Nothing while it is off.
- **CPU:** 2 virtual processors.
- **Setup time:** writing and debugging the build scripts, 1–2 days of agent work. An unattended rebuild after that, about 1 hour, mostly Windows setup and SQL Server setup.
- **Per test run:** the VM is off between runs. A cold start until the DC answers Kerberos takes a few minutes. A VM saved with `Save-VM` resumes faster, but its clock must be resynchronised (Hyper-V time synchronisation integration service) to stay inside the 5-minute tolerance.
- **Licence:** the 180 days are wall-clock days, so a stopped VM probably keeps counting (not verified). Plan on a scripted rebuild before day 180 rather than `slmgr /rearm`: rearm counts come only from secondary sources and a Microsoft Q&A thread, which disagree (1, 5 or 6).

### Build steps

Scripts in the repo (for example `lab/`), PowerShell with the Hyper-V module, all values in one settings file, the lab password outside git. Everything inside the VM runs through PowerShell Direct (`Invoke-Command -VMName dc1`, `Copy-Item -ToSession`), which works "regardless of network configuration or remote management settings" and so needs no WinRM `TrustedHosts` entry on the host. It does need a Hyper-V administrator on the host and VM credentials. ([PowerShell Direct](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/powershell-direct))

1. **Host, once (owner, as admin):** enable Hyper-V, restart; create the internal switch and host adapter IP.
2. **VM:** `New-VHD` / `New-VM` (generation 2, 4 GB, 2 vCPU, two adapters: internal and Default Switch), boot the ISO with an `autounattend.xml` for Server Core, a fixed computer name `dc1` and the lab admin password.
3. **Domain:** `Install-ADDSForest -DomainName lab.test`, then create `tester`, `svc-sql` and `svc-pg`, and activate the evaluation over the Default Switch adapter. Stop that adapter registering its NAT address in `lab.test` DNS (`Set-DnsClient -RegisterThisConnectionsAddress $false`), or `dc1.lab.test` can resolve to an address the host cannot reach.
4. **Host DNS (owner, as admin):** add the NRPT rule for `.lab.test`.
5. **Gate:** `runas /netonly /user:tester@lab.test cmd`, then in that window `klist get host/dc1.lab.test`. If no ticket comes back, stop here; the lab cannot work and the manual checklist stays the only Kerberos test. This answers the open question before anything else is installed.
6. **SQL Server 2025 Developer:** unattended setup with a configuration file, TCP 1433, service account `LAB\svc-sql`; `setspn -S MSSQLSvc/dc1.lab.test:1433 LAB\svc-sql` and `MSSQLSvc/dc1.lab.test`; Windows login for `LAB\tester` with `VIEW SERVER PERFORMANCE STATE`.
7. **PostgreSQL 18:** the same pinned binaries as #16, registered as a service under `LAB\svc-pg`; `setspn -S POSTGRES/dc1.lab.test LAB\svc-pg`; `pg_hba.conf` line `host all all <lab subnet> sspi map=lab` with an ident map for `tester@LAB`; `log_connections = 'authentication'`.
8. **Deny NTLM:** set "Restrict NTLM: Incoming NTLM traffic" to "Deny all accounts" on the VM. Local connections on the server itself use NTLM (SPN page, "Authentication defaults"), so after this step Windows sign-in from inside the VM fails; maintenance uses a SQL login (mixed mode, as #16 chose) and the PostgreSQL superuser over a local `scram-sha-256` line.
9. **Harness:** a test target that starts the VM, waits for port 88 and 1433 and 5432, runs the Kerberos tests through `CreateProcessWithLogonW`, and stops the VM.

### Recommendation

Build it, but in two stages. Stage one is steps 1–5, about half a day: if the gate passes, the core question is answered. Stage two adds the servers and the harness. Keep #16's manual checklist at work for each release either way: the lab proves the protocol path, not the quirks of a real corporate domain (cross-domain trusts, constrained delegation, encryption-type policy).

## Not verified

- That Kerberos (rather than NTLM) is used inside a `/netonly` or `LOGON_NETCREDENTIALS_ONLY` session on a PC not joined to a domain, with KDC discovery through an NRPT rule. The docs say the credentials are used on the network and that the locator uses DNS SRV records, but no Microsoft page states this combination. Step 5 tests it.
- That a local NRPT rule is honoured for SRV lookups by the DC locator, and that adding one needs no restart.
- That the Hyper-V internal switch gives the host a virtual adapter that needs a manual static IP (Microsoft's page says only that the switch connects host and VMs).
- That the Windows PostgreSQL 18 binaries are built with GSSAPI (needed for the `gss` method).
- Whether `pg_ident.conf` sees `tester@LAB` or `tester@LAB.TEST` with the default `compat_realm = 1`; the docs say the NetBIOS name is used, which suggests `LAB`.
- Disk, memory and time figures: estimates, nothing was built.
- Whether a stopped evaluation VM's 180 days keep counting, and the rearm count.
- That the Default Switch exists on Windows 11 Pro once Hyper-V is enabled and gives VMs internet through NAT.
- Whether `runas /netonly` works with the UPN form. The runas page documents `user@domain` for `/user:` in general.
