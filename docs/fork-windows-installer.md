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

## Notes

- This workflow only builds/packages Windows targets. It does not touch
  the existing macOS packaging (`.github/workflows/package.yaml` /
  `release.yaml`), which is unchanged.
- It reuses the same build flags, `package.ps1`, and WiX sources
  (`resources/wix/`) as the upstream release pipeline, so the produced
  installers match upstream's layout.
