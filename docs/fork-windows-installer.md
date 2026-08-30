# Fork Windows installer builds

This fork adds `.github/workflows/fork-windows-installer.yaml`, a workflow
not present upstream, so Windows installers can be built here without the
upstream project's Azure Key Vault code-signing secrets.

## Triggering the build

1. In this fork, go to **Actions → Fork Windows installer (unsigned by
   default) → Run workflow**.
2. Pick the branch to build from and optionally set `version_number`
   (defaults to `0.0.0`).
3. Run it. Two jobs execute: `build-windows` (compiles both Windows
   targets) and `package-windows` (produces the installers).

Equivalent via the CLI: `gh workflow run fork-windows-installer.yaml --ref <branch> -f version_number=1.2.3`.

## Artifacts produced

The `package-windows` job uploads three separate artifacts:

- `windows-installer-x64-msi` — standalone x64 MSI
- `windows-installer-arm64-msi` — standalone arm64 MSI
- `windows-installer-universal-exe` — universal bootstrapper EXE (installs
  the correct architecture at runtime)

The job fails outright if any of the three expected files is missing.

## Signing is optional and off by default

`resources/scripts/package.ps1` (shared with the upstream packaging
workflow) only signs a file when **all** of these secrets/variables are
configured on this fork; otherwise it logs that signing is being skipped
and continues, producing unsigned installers:

- Secrets: `AZ_VAULT_URL`, `AZ_CERT_NAME`, `AZ_CLIENT_ID`,
  `AZ_CLIENT_SECRET`, `AZ_TENANT_ID`
- Variable: `RFC3161_TIMESTAMP_URL`

To enable signing later, set those under this fork's repository
**Settings → Secrets and variables → Actions** with values for your own
Azure Key Vault certificate — no code changes are required. Values are
never printed to logs (GitHub redacts anything sourced from `secrets.*`,
and the script never echoes them).

## Unsigned builds intentionally disable UIAccess

`glazewm.exe` has an optional `ui_access` build feature that requests the
Windows [UIAccess privilege][uiaccess], which lets it set the foreground
window and reposition elevated windows. UIAccess only works for an exe
that is **Authenticode-signed and installed in a secure location** (e.g.
`C:\Program Files`) — Windows refuses to even launch a UIAccess exe that
isn't trustworthy, failing with "A referral was returned from the server."

Because this fork has no signing secrets configured by default, both
`fork-windows-installer.yaml` and `package.ps1`'s build fallback only
enable the `ui_access` feature when every signing secret above is
present. An unsigned fork build therefore launches normally, but with one
functional limitation: **it can't bring elevated windows to the
foreground or reposition them.** This does not affect non-elevated
windows. If you configure signing (see above), builds regain UIAccess.

Do not work around the launch failure by disabling UAC, SmartScreen, or
any other Windows security policy — that weakens your machine's security
for a problem that's fixed correctly by either not requesting UIAccess
(the default here) or by actually signing the binary.

[uiaccess]: https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-10/security/threat-protection/security-policy-settings/user-account-control-only-elevate-uiaccess-applications-that-are-installed-in-secure-locations

## Notes

- This workflow only builds/packages Windows targets. It does not touch
  the existing macOS packaging (`.github/workflows/package.yaml` /
  `release.yaml`), which is unchanged.
- It reuses the same build flags, `package.ps1`, and WiX sources
  (`resources/wix/`) as the upstream release pipeline, so the produced
  installers match upstream's layout.
