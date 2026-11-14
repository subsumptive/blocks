param(
    [string]$Version = "latest",
    [string]$InstallDir = "$HOME\.sub\bin"
)

$ErrorActionPreference = "Stop"
$Repo = "subsumptive/blocks"

if ($env:PROCESSOR_ARCHITECTURE -ne "AMD64") {
    Write-Error "Unsupported Windows architecture: $env:PROCESSOR_ARCHITECTURE (only x86_64/AMD64 is supported)"
    exit 1
}

$Target = "x86_64-pc-windows-msvc"

if ($Version -eq "latest") {
    $resp = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 0 -ErrorAction SilentlyContinue
    if ($resp.Headers.Location) {
        $Version = ($resp.Headers.Location -split "/")[-1]
    } else {
        $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $latest.tag_name
    }
}

$Archive = "sub-$Version-$Target.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"
$TmpDir = Join-Path $env:TEMP ("sub-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TmpDir | Out-Null

try {
    Write-Host "Downloading Subsumptive Blocks $Version ($Target)..."
    $ArchivePath = Join-Path $TmpDir $Archive
    $SumsPath = Join-Path $TmpDir "SHA256SUMS.txt"

    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ArchivePath
    Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS.txt" -OutFile $SumsPath

    $ExpectedLine = Get-Content $SumsPath | Where-Object { $_ -match "\s$([regex]::Escape($Archive))$" } | Select-Object -First 1
    if (-not $ExpectedLine) {
        throw "Checksum for $Archive not found in SHA256SUMS.txt"
    }
    $ExpectedSha = ($ExpectedLine -split "\s+")[0].ToUpperInvariant()
    $ActualSha = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToUpperInvariant()

    if ($ExpectedSha -ne $ActualSha) {
        throw "Checksum mismatch! Expected $ExpectedSha, got $ActualSha"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Expand-Archive -Path $ArchivePath -DestinationPath $TmpDir -Force
    Copy-Item (Join-Path $TmpDir "sub.exe") (Join-Path $InstallDir "sub.exe") -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
        Write-Host "Added $InstallDir to User PATH. Restart your terminal to use 'sub'."
    }

    Write-Host "Installed sub.exe to $InstallDir\sub.exe"
} finally {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}
