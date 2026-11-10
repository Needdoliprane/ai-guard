# Authentication audit V4 tenant capacity — 10 November 2026

The software correction uses finite durable record/byte partitions for the
authentication audit. Twenty-eight distinct targeted functions PASS across
two explicitly scoped sources; this is not a result from one full source or
a deployed-provider/runtime qualification.

## Sources, reviews and actual RED

The baseline is public8af plus one test: 2347-file manifest
`e01cb4066918798c61d57d8355df488f113c17e10b8b30ab00f1c57d3bdf3124`.
Its exact NV-02 parent builds RC0 then reaches the final
`NV02_TENANT_CAPACITY` assertion RC101: after a positive B control, seven valid
A refreshes and 17 durable records, B's first shared admission is503 rather
than201. Head/quota/A-state/reopen and fixture cleanup precede that oracle.
This is a controlled software-fixture result, with no formal deployment
severity assigned. The actual baseline source export is
`4a71f1a94782ef48e0854028030ed8b88492c33574d466069dfef5533104ecfe`.

Auth V4 source is composed with the reviewed Fed/Core12 code. The original
R1 manifest is `08524e5027cbd478e6a37d1736ad5bb65478a37e31f69fb697c57bd3f54cb4b7`.
Only three test files receive the reviewed type/import correction for R2:

| Executed scope | Full source manifest, 2368 files | Distinct PASS |
|---|---|---:|
| R2: library and configuration | `2211c5b0036e866e56b263308639f8957705ed200040e258a1995e9b4ede9365` |19+4|
| After R4 fixture correction: three public builders, NV-02 and tenant flow |`ef04abc364994b1144cc8e3b3c1bd82f2ce05ff5c237c6a41baa18b31293e8c6`|5|

R2→R4 changes exactly `tests/oidc_tenant_capacity_configuration.rs`: correct
callback URI and a specific typed Format refusal. No business source changes.
The earlier23 functions are defined in seven distinct Rust files, all
byte-identical between R2/R4. Only the builder target is rebuilt; NV-02/flow
use their unchanged already-built binaries. The28 unique names and stdout
hashes are recorded in private `ROOT_28_SCOPED_INDEX.json`.

The19 source postimages proposed on public
`b6dc5db05173848c18047184ecc71f610b203472` all equal the actual R4 manifest;
manifest19 `d991f0256c50e800156a227dba42269f0409c7ef21c5c5469ed12cb02305751f`.
Composed main/identity preserve Core12. The19-source patch does not replace
old8af files wholesale. Publicb6 already has eight documentation/evidence
differences from the tested composition, including one additional Fed README;
b6+Auth19 therefore has2369 files. Those documentation-only differences do not
turn the QA source into a2369-file test run or qualify unrelated working WIP.
The six reviewed operator-document changes are separate from the compiled gel.

Independent source reviews: Core `de6d32c93408c57c022fa47f2c18e662f18ff6673e42a90462b0e70329cd9634`,
Root/gateway delta `8d054a3b2edffa681812ad7f6e0b4202d734896e148c0f5a8ec577ee34d90771`.
Fixture review `313c2a72899b4af40d3a0333c3f931c6996057d9ee9c306cef15fe21ed7bbc42`;
operator GO_DOC `e3760e0a80a4691cdb2e110a17fbd8dc63b7cc30cfc1b4d78a96d63d100dd1b1`.
The R2 type/import patch review is `21aa1e51…`, without changing business,
capacities, deadlines or the behavioral oracles. These source/doc reviews
are separate from executed test results.

## Execution history and conservation

| Operation | Actual result |
|---|---|
| R1 lib build |RC101, eight test-type/import errors; no behavioral RED or PASS.|
| R2 lib build |RC101/SIGXFSZ after the harness incorrectly set an inner32MiB file cap instead of the admitted512MiB; not a new product failure.|
| R3 unchanged R2-source lib build |RC0,11:37:09–11:40:35UTC, direct argv with the incorrect inner shell/limit removed.|
| Four selected targets build |RC0,11:50:58–11:52:48UTC.|
|19 library +4 config parents on R2 |23 distinct exact PASS.|
| Old parent24 |RC101 in OidcGateway fixture construction: wrong redirect URI, before the intended builder oracle; retained separately.|
| R4 builder-only rebuild |RC0,12:04:02–12:04:12UTC.|
| Remaining five parents |Five exact PASS, no repeat of the unaffected23.|
| R4 all-targets check |RC0,12:09:29–12:12:45UTC; warnings retained.|

Builds use frozen/offline inputs and exclusive retained QA target, with Root's
bounded direct timeout→env→Cargo commands after the R2 correction. Each parent
is selected once with --exact/--nocapture/--test-threads=1. Build/list/check
operations are not tests. No provider, engine or native application ran on the
maintainer's host; this publication preparation only reads trusted data/Git.

Post-R4 source hashes:2368 OK at12:15:16–17UTC; exact pathset and nonregular
controls RC0. Actual R4 export:24330240 bytes, SHA
`1d8e0a6ddfb3f53f9ea772c53e194832d5e2f0f1d406591c6221712df7a37565`;
all2368 regular contents match the R4 manifest, with no link/special/PAX or
AppleDouble member. Root's closure report SHA is
`e47320d57356896e0620b5d6698208121c17f7cc3959d89948fc4e279cb09dd0`.
The owned Auth QA was stopped and removed normally at12:21:30UTC after only
its idle lifecycle process remained. Its explicit-stop exit137 is not an
inner test failure or OOM. Memory events/OOM zero, peak5659213824 bytes;
scratch2049531904/3221225472 bytes,24552/98304 inodes. Source/raws/private
proofs remain outside worktrees; no other QA or user data is covered by this
closure.

## Delivered scope and remaining limits

V4 funds committed history L, exact historical terminal tickets R and the real
plan marker M before allocating tenant and unscoped parts, under unchanged G
in records and bytes. Parts do not borrow from each other and committed history
is not recycled. Unscoped cannot admit a session. Accounting ownership comes
from verified membership metadata and remains fixed for the session; business
permissions and tenant identity still require their existing checks. A free
JSON tenant claim cannot select the business scope.

The demonstrated preservation is finite and applies to disjoint memberships.
A member of A+B can consume its own B budget; B is not protected from its own
members. Exhausting B's part, the global bounds, retained history or promises
still refuses explicitly. This isolates this ledger budget, while shared store I/O and other control capacities
retain their own limits. It is neither infinite availability nor a measured
disk/WAL, throughput, latency, actual CPU-consumption or financial-billing model.

The public multi-tenant builder requires V4 and the exact configured tenant set.
Explicit mono-V3 compatibility remains. Configuration2 and backed-up stopped
3→4 migration are explicit; old history/receipts and outstanding exact terminals
are preserved, not reassigned into invented sessions or Logout BEGINs. The
existing Setup1 generator remains V3/config1; its separate SetupV2 work is not
included. See the paired [SSO operator guide](../../../../setup/sso/README.md).

The tests cover bounded SQLite/real-router/controlled-issuer software cases,
two handles, replay/capacity/refusal, metadata and native-logout ledger canaries.
They do not qualify a real IdP/SSO deployment, user provider, Docker/Podman/
Kubernetes worker,100 workers, crash/fencing/cold recovery, membership-race
matrix or every capacity path. Old-reader executable verification, retention/
archive policy, compiler warnings and Clippy remain separate.
**G09 FAIL;0/12 global gates closed.**
