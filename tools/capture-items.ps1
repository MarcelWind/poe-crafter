#Requires -Version 5.1
<#
.SYNOPSIS
Capture Path of Exile item clipboard text into fixture files.

.DESCRIPTION
Polls the clipboard and saves each new item text once. Use Advanced Mod
Descriptions (Ctrl+Alt+C) for tier/affix headers, and normal Ctrl+C for the
fallback format. Item texts are written as UTF-8 without BOM.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\capture-items.ps1

.EXAMPLE
powershell -ExecutionPolicy Bypass -File tools\capture-items.ps1 -Once
#>
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path $PSScriptRoot "..\samples\items"),
    [int]$PollMs = 400,
    [switch]$Once
)

$ErrorActionPreference = "Stop"
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$seen = @{}
$sha = [Security.Cryptography.SHA256]::Create()
$utf8 = New-Object Text.UTF8Encoding($false)
$counter = 0

Write-Host "Watching clipboard for Path of Exile item text."
Write-Host "  output: $OutDir"
Write-Host "  use Ctrl+Alt+C (advanced) and Ctrl+C (normal); Ctrl+C in this console stops."
Write-Host ""

while ($true) {
    $text = $null
    try {
        $text = Get-Clipboard -Raw -ErrorAction Stop
    }
    catch {
        Start-Sleep -Milliseconds $PollMs
        continue
    }

    if ([string]::IsNullOrWhiteSpace($text)) {
        Start-Sleep -Milliseconds $PollMs
        continue
    }

    $normalized = $text.Trim() -replace "`r`n", "`n"
    if ($normalized -notmatch "(?m)^Rarity:\s") {
        Start-Sleep -Milliseconds $PollMs
        continue
    }

    $digest = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($normalized)))
    if ($seen.ContainsKey($digest)) {
        Start-Sleep -Milliseconds $PollMs
        continue
    }
    $seen[$digest] = $true

    $kind = if ($normalized -match "(?m)^\{ .*(Modifier|Enhancement)") { "advanced" } else { "normal" }
    if ($kind -eq "normal" -and $normalized -match "(?m)^\{ ") { $kind = "mixed" }

    do {
        $counter++
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $name = "capture-$stamp-$kind-$counter.txt"
        $path = Join-Path $OutDir $name
    } while (Test-Path -LiteralPath $path)

    [IO.File]::WriteAllText($path, $normalized + "`n", $utf8)
    Write-Host ("saved {0} ({1} chars)" -f $name, $normalized.Length)

    if ($Once) { break }
    Start-Sleep -Milliseconds $PollMs
}
