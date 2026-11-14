# Installing Subsumptive Blocks (`sub`)

## Quick Install

### Linux & macOS

```bash
curl -fsSL https://subsumptive.net/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://subsumptive.net/install.ps1 | iex
```

## Download-Then-Inspect (Recommended)

### Linux & macOS

```bash
curl -fsSL https://subsumptive.net/install.sh -o install.sh
less install.sh
sh install.sh --version v0.3.0 --install-dir ~/.local/bin
```

### Windows (PowerShell)

```powershell
Invoke-WebRequest https://subsumptive.net/install.ps1 -OutFile install.ps1
Get-Content install.ps1
.\install.ps1 -Version v0.3.0 -InstallDir "$HOME\.sub\bin"
```

## Verify Installation

```bash
sub --version
sub --help
```

## Astro Website Integration

The canonical installer scripts live in `scripts/install.sh` and `scripts/install.ps1` in `subsumptive/blocks`. The `subsumptive.net` Astro website serves them from `public/install.sh` and `public/install.ps1` at the domain root.
