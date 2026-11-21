# Security Policy

[Français](SECURITY.fr.md)

## Reporting a vulnerability

Report vulnerabilities privately through **GitHub Security Advisories** of this repository: open the repository's **Security** tab and choose **Report a vulnerability**. Do not put vulnerabilities or reproduction details in issues, pull requests or discussions.

**Current state of the channel.** The repository is private. Maintainers can open draft security advisories now. Private vulnerability reporting by third parties will be enabled when the repository becomes public; until then, a reporter needs a maintainer to open a draft advisory and invite them. No email address, bounty or response-time commitment is offered.

## What to include

*   A clear and concise description of the issue and the affected component (gateway, broker, dashboard, worker image, release archives, documentation).
*   The version (`agentsandbox version --json`) or commit, and the runtime (Docker, Podman) when relevant.
*   Steps to reproduce, using synthetic data only. Never include real credentials, tokens or customer data.
*   An assessment of the potential impact.

## Handling procedure

1.  **Receive.** A maintainer opens or accepts the draft advisory. The report stays private to the advisory participants.
2.  **Acknowledge.** A maintainer answers in the advisory. No fixed delay is promised.
3.  **Triage.** The maintainer reproduces the report against the current checkout and compares it with the known findings in [the audit follow-up](docs/engineering/AUDIT_FOLLOWUP.md). The advisory records the outcome: confirmed, needs information, duplicate or rejected, with the reason.
4.  **Fix.** A confirmed issue is fixed in a private advisory branch with a regression test, then independently reviewed.
5.  **Release and disclosure.** The fix ships in a release. The advisory is then published with the affected and fixed versions, and the reporter is credited unless they decline.

## Supported versions

Only the latest published release is considered for fixes. ai-guard has not been qualified for production; see [the production-readiness gates](docs/engineering/PRODUCTION_READINESS.md).

## Verifying release artifacts

Release archives are published with `SHA256SUMS`, a CycloneDX SBOM and a `BUILD-INFO` file in each archive that binds it to the tag, commit and workflow run. **Nothing is cryptographically signed**: checksums detect corruption, not a compromised release page. The verification procedure is in [the installation guide](docs/setup/release/install.md).
