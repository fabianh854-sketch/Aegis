#Requires -Version 7
<#
.SYNOPSIS
    Erneuert das Manifest des neuesten Releases (wöchentlich ausführen!).

.DESCRIPTION
    Manifeste sind nur 7 Tage gültig (Schutz davor, dass ein neues Release versteckt wird). Dieses Skript lädt
    Manifest und Signatur des neuesten veröffentlichten Releases (kein Entwurf, kein Vorab-Release), lässt sie vom
    Signierwerkzeug prüfen und mit neuem Gültigkeitszeitraum neu signieren (Passwortabfrage durch das Werkzeug;
    dieses Skript sieht das Passwort nie) und ersetzt beide Dateien im Release. Version, Prüfsummen und
    min_secure bleiben unverändert.

    Das Signierwerkzeug wird aus dem aktuell ausgecheckten, sauberen Stand gebaut. Prüfe vorher, dass dieser Stand
    der geprüfte ist (git log -1).
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$Repo = 'fabianh854-sketch/Aegis'
$Work = $null

function Invoke-Native {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgList = @())
    & $Exe @ArgList
    if ($LASTEXITCODE -ne 0) {
        throw "Befehl fehlgeschlagen (Exit $LASTEXITCODE): $Exe $($ArgList -join ' ')"
    }
}

function Get-NativeOutput {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgList = @())
    $out = & $Exe @ArgList
    if ($LASTEXITCODE -ne 0) {
        throw "Befehl fehlgeschlagen (Exit $LASTEXITCODE): $Exe $($ArgList -join ' ')"
    }
    return ,@($out)   # Komma: auch bei einer einzigen Zeile ein Array liefern
}

try {
    foreach ($tool in 'git', 'cargo', 'gh') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "Das Werkzeug '$tool' wurde nicht gefunden."
        }
    }
    $RepoRoot = Split-Path -Parent $PSScriptRoot
    Set-Location $RepoRoot

    $dirty = Get-NativeOutput git @('status', '--porcelain')
    if ($dirty.Count -gt 0) {
        throw "Der Arbeitsbaum ist nicht sauber. Das Signierwerkzeug wird aus diesem Stand gebaut und bekommt dein Passwort; deshalb nur mit sauberem, geprüftem Stand:`n$($dirty -join "`n")"
    }
    Write-Host 'Signierwerkzeug wird gebaut aus:' -ForegroundColor Cyan
    Invoke-Native git @('-c', 'core.quotepath=off', 'log', '-1', '--format=%h %an %s')

    $KeyFile = Join-Path $env:USERPROFILE '.aegis-signing\aegis.key'
    if (-not (Test-Path -LiteralPath $KeyFile -PathType Leaf)) {
        throw "Der Signierschlüssel $KeyFile fehlt."
    }

    & gh auth status *> $null
    if ($LASTEXITCODE -ne 0) { throw 'gh ist nicht angemeldet. Erst "gh auth login" ausführen.' }

    # Neuestes veröffentlichtes Release (kein Entwurf, kein Vorab-Release).
    $releases = (Get-NativeOutput gh @('release', 'list', '--repo', $Repo, '--exclude-drafts', '--exclude-pre-releases',
            '--json', 'tagName,publishedAt', '--limit', '100')) -join "`n" | ConvertFrom-Json
    $latest = $releases | Sort-Object { [datetime]$_.publishedAt } -Descending | Select-Object -First 1
    if (-not $latest) { throw 'Es gibt noch kein veröffentlichtes Release.' }
    $tag = $latest.tagName
    Write-Host "Neuestes Release: $tag" -ForegroundColor Cyan

    $assets = ((Get-NativeOutput gh @('release', 'view', $tag, '--repo', $Repo, '--json', 'assets')) -join "`n" | ConvertFrom-Json).assets.name
    if (($assets -notcontains 'aegis-manifest.json') -or ($assets -notcontains 'aegis-manifest.json.minisig')) {
        throw "Das Release $tag enthält kein Manifest samt Signatur (aegis-manifest.json und aegis-manifest.json.minisig). Es kann nichts erneuert werden."
    }

    # Frischer Arbeitsordner pro Lauf.
    $Work = Join-Path $env:TEMP "aegis-refresh-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
    $inDir = Join-Path $Work 'in'
    $outDir = Join-Path $Work 'out'
    New-Item -ItemType Directory -Path $inDir, $outDir | Out-Null

    Invoke-Native gh @('release', 'download', $tag, '--repo', $Repo, '--dir', $inDir,
        '-p', 'aegis-manifest.json', '-p', 'aegis-manifest.json.minisig')

    Invoke-Native cargo @('build', '--release', '--locked', '-p', 'aegis-sign')
    $meta = (Get-NativeOutput cargo @('metadata', '--no-deps', '--format-version', '1', '--locked')) -join "`n" | ConvertFrom-Json
    $signExe = Join-Path $meta.target_directory 'release\aegis-sign.exe'
    if (-not (Test-Path -LiteralPath $signExe -PathType Leaf)) { throw "Erwartete Datei fehlt nach dem Build: $signExe" }

    Write-Host ''
    Write-Host 'Gleich fragt das Signierwerkzeug nach dem Passwort des Schlüssels.' -ForegroundColor Cyan
    # Ohne Ausgabeumleitung aufrufen, damit die Passwortabfrage am Terminal funktioniert.
    & $signExe refresh --key $KeyFile `
        --manifest (Join-Path $inDir 'aegis-manifest.json') `
        --sig (Join-Path $inDir 'aegis-manifest.json.minisig') `
        --out $outDir
    if ($LASTEXITCODE -ne 0) { throw "Das Erneuern ist fehlgeschlagen (Exit $LASTEXITCODE)." }

    foreach ($f in 'aegis-manifest.json', 'aegis-manifest.json.minisig') {
        if (-not (Test-Path -LiteralPath (Join-Path $outDir $f) -PathType Leaf)) { throw "Datei fehlt nach dem Erneuern: $f" }
    }

    Invoke-Native gh @('release', 'upload', $tag, '--repo', $Repo, '--clobber',
        (Join-Path $outDir 'aegis-manifest.json'), (Join-Path $outDir 'aegis-manifest.json.minisig'))

    Write-Host ''
    Write-Host "Fertig: Das Manifest von $tag ist für weitere 7 Tage gültig. Nächste Erneuerung spätestens in 7 Tagen." -ForegroundColor Green
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}
catch {
    Write-Host ''
    Write-Host "Abbruch: $($_.Exception.Message)" -ForegroundColor Red
    if ($Work -and (Test-Path -LiteralPath $Work)) {
        Write-Host "Der Arbeitsordner bleibt zur Fehlersuche erhalten: $Work" -ForegroundColor Yellow
    }
    exit 1
}
