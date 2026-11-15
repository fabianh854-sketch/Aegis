#Requires -Version 7
<#
.SYNOPSIS
    Baut, signiert und veröffentlicht ein Aegis-Release (lokal, nicht in GitHub Actions).

.DESCRIPTION
    Ablauf:
      1. Vorprüfungen (alle lokal, vor jeder Passwortabfrage): sauberer Arbeitsbaum, Tag v<Version> zeigt auf HEAD,
         Cargo-Version = <Version>, TRUSTED_KEYS in keys.rs nicht leer.
      2. Netz-Vorprüfungen: gh angemeldet, Tag liegt auf GitHub auf demselben Commit, Release existiert noch nicht.
         Vorgänger = höchste veröffentlichte Version unter <Version>. Fehlt ihm Manifest oder Signatur, bricht das
         Skript ab (außer beim allerersten signierten Release mit -FirstSignedRelease).
      3. Alle Commits seit dem letzten Release werden angezeigt; Weiter nur nach Eingabe von "geprüft".
      4. cargo deny, Tests, Release-Build, Cargo.lock unverändert.
      5. Übernommenes min_secure anzeigen und bestätigen lassen (j/n), dann Manifest bauen und signieren
         (das Werkzeug fragt das Schlüsselpasswort selbst ab; dieses Skript sieht es nie).
      6. Zip + .sha256 bauen, Release mit allen Dateien auf GitHub veröffentlichen.

    Gebaut wird in einem frischen Ordner unter $env:TEMP; ins Repository wird nie geschrieben.

.PARAMETER Version
    Die neue Version, genau X.Y.Z (ohne führendes v). Der Tag v<Version> muss bereits existieren, auf HEAD zeigen
    und gepusht sein.

.PARAMETER Security
    Markiert das Release als Sicherheitsupdate: min_secure wird auf diese Version gesetzt.

.PARAMETER FirstSignedRelease
    Nur für das allererste Release mit Manifest: erlaubt einen Vorgänger ohne Manifest (oder gar keinen), dann beginnt
    min_secure bei 0.0.0 (bzw. bei dieser Version mit -Security). Wird abgelehnt, sobald irgendein Release auf GitHub
    ein aegis-manifest.json enthält.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$Version,

    [switch]$Security,

    [switch]$FirstSignedRelease
)

$ErrorActionPreference = 'Stop'
# Exit-Codes nativer Befehle prüfen die Hilfsfunktionen selbst; nicht vom Wert in $PROFILE abhängen.
$PSNativeCommandUseErrorActionPreference = $false
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$Repo = 'fabianh854-sketch/Aegis'
$RepoUrl = "https://github.com/$Repo.git"
$Tag = "v$Version"
$ReleaseFiles = 'aegis-manifest.json', 'aegis-manifest.json.minisig', 'aegis.exe', 'aegis-updater.exe'

# Führt einen nativen Befehl aus und bricht bei Exit-Code <> 0 ab (ErrorActionPreference greift dafür nicht).
function Invoke-Native {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgList = @())
    & $Exe @ArgList
    if ($LASTEXITCODE -ne 0) {
        throw "Befehl fehlgeschlagen (Exit $LASTEXITCODE): $Exe $($ArgList -join ' ')"
    }
}

# Wie Invoke-Native, liefert aber die Ausgabe zeilenweise zurück.
function Get-NativeOutput {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgList = @())
    $out = & $Exe @ArgList
    if ($LASTEXITCODE -ne 0) {
        throw "Befehl fehlgeschlagen (Exit $LASTEXITCODE): $Exe $($ArgList -join ' ')"
    }
    return ,@($out)   # Komma: auch bei einer einzigen Zeile ein Array liefern
}

. (Join-Path $PSScriptRoot 'release-helpers.ps1')

# Liest die TRUSTED_KEYS-Konstante direkt aus keys.rs (Kommentare werden ignoriert) und liefert die Schlüssel.
# Unlesbares oder unerwartetes Format zählt als "keine Schlüssel" (Abbruch, nie Weitermachen).
function Get-TrustedKeys {
    param([Parameter(Mandatory)][string]$Path)
    return Get-TrustedKeysFromSource -Source (Get-Content -LiteralPath $Path -Raw -Encoding utf8)
}

$OldRustFlags = $env:RUSTFLAGS
$Stage = $null
$Failed = $true

try {
    # ---------------------------------------------------------------- 1. lokale Vorprüfungen
    # Wie der Updater: nur Ziffern, keine führenden Nullen.
    if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or -not (ConvertTo-AegisVersion $Version)) {
        throw "Ungültige Version '$Version'. Erlaubt ist nur X.Y.Z, z. B. 0.1.1 (ohne führendes v, ohne Zusatz)."
    }
    foreach ($tool in 'git', 'cargo', 'gh') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "Das Werkzeug '$tool' wurde nicht gefunden."
        }
    }

    $RepoRoot = Split-Path -Parent $PSScriptRoot
    Set-Location $RepoRoot

    Write-Host '== Vorprüfungen ==' -ForegroundColor Cyan

    $dirty = Get-NativeOutput git @('status', '--porcelain')
    if ($dirty.Count -gt 0) {
        throw "Der Arbeitsbaum ist nicht sauber (geänderte oder unversionierte Dateien). Erst committen oder aufräumen:`n$($dirty -join "`n")"
    }

    $head = (Get-NativeOutput git @('rev-parse', 'HEAD'))[0]
    & git rev-parse --verify --quiet "refs/tags/$Tag^{commit}" *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "Der Tag $Tag existiert nicht. Erst anlegen: git tag $Tag  (und danach: git push origin $Tag)"
    }
    $tagCommit = (Get-NativeOutput git @('rev-parse', "refs/tags/$Tag^{commit}"))[0]
    if ($tagCommit -ne $head) {
        throw "Der Tag $Tag zeigt auf $($tagCommit.Substring(0, 7)), HEAD ist aber $($head.Substring(0, 7)). Der Tag muss auf HEAD zeigen."
    }

    $meta = (Get-NativeOutput cargo @('metadata', '--no-deps', '--format-version', '1', '--locked')) -join "`n" | ConvertFrom-Json
    foreach ($pkg in $meta.packages) {
        if ($pkg.version -ne $Version) {
            throw "Die Cargo-Version von $($pkg.name) ist $($pkg.version), erwartet wird $Version. Version in Cargo.toml ([workspace.package]) anpassen, committen und den Tag neu setzen."
        }
    }
    $TargetDir = $meta.target_directory

    # Warum keys.rs direkt gelesen wird statt den Fehler des Signierwerkzeugs abzuwarten: So bricht das Skript
    # ab, bevor gebaut, getestet oder nach einem Passwort gefragt wird. Das Signierwerkzeug lehnt eine leere Liste
    # später zusätzlich selbst ab (doppelte Sicherung, "fail closed").
    $keysFile = Join-Path $RepoRoot 'crates\aegis-update\src\keys.rs'
    $trusted = Get-TrustedKeys -Path $keysFile
    if ($trusted.Count -eq 0) {
        throw "Kein vertrauenswürdiger Schlüssel in crates\aegis-update\src\keys.rs eingetragen. Erst scripts\new-signing-key.ps1 ausführen und den öffentlichen Schlüssel dort eintragen."
    }

    $KeyFile = Join-Path $env:USERPROFILE '.aegis-signing\aegis.key'
    if (-not (Test-Path -LiteralPath $KeyFile -PathType Leaf)) {
        throw "Der Signierschlüssel $KeyFile fehlt. Mit scripts\new-signing-key.ps1 erzeugen oder aus der Offline-Sicherung zurückkopieren."
    }

    # ---------------------------------------------------------------- 2. Netz-Vorprüfungen
    & gh auth status *> $null
    if ($LASTEXITCODE -ne 0) { throw 'gh ist nicht angemeldet. Erst "gh auth login" ausführen.' }

    $remoteTagLines = Get-NativeOutput git @('ls-remote', $RepoUrl, "refs/tags/$Tag*")
    $remoteTag = $null
    foreach ($line in $remoteTagLines) {
        $sha, $ref = ($line -split '\s+')[0, 1]
        if ($ref -eq "refs/tags/$Tag^{}") { $remoteTag = $sha; break }   # annotierter Tag: der abgeschälte Commit
        if ($ref -eq "refs/tags/$Tag") { $remoteTag = $sha }
    }
    if (-not $remoteTag) {
        throw "Der Tag $Tag ist auf GitHub nicht vorhanden. Erst pushen: git push origin $Tag"
    }
    if ($remoteTag -ne $head) {
        throw "Der Tag $Tag liegt auf GitHub auf $($remoteTag.Substring(0, 7)), nicht auf HEAD ($($head.Substring(0, 7)))."
    }

    # Alle Releases (alle Seiten, auch Entwürfe und Vorab-Releases) samt Dateinamen.
    $allReleases = Get-AegisReleases -Repo $Repo
    $newVersion = ConvertTo-AegisVersion $Version
    foreach ($r in $allReleases) {
        $rv = ConvertTo-AegisVersion $r.tagName
        if ($r.tagName -eq $Tag -or ($rv -and (Compare-AegisVersion $rv $newVersion) -eq 0)) {
            throw "Zu $Tag gibt es auf GitHub schon ein Release ($($r.tagName)), einen Entwurf oder ein Vorab-Release. Ein Release wird nicht überschrieben."
        }
    }
    & gh release view $Tag --repo $Repo *> $null
    if ($LASTEXITCODE -eq 0) {
        throw "Zu $Tag gibt es auf GitHub schon einen Entwurf oder ein Vorab-Release. Erst entfernen oder aufräumen."
    }
    $highest = Select-LatestRelease $allReleases
    if ($highest -and (Compare-AegisVersion $highest.Version $newVersion) -gt 0) {
        throw "Es gibt schon ein höheres veröffentlichtes Release ($($highest.tagName)). Die neue Version muss die höchste sein."
    }

    # Vorgänger = höchste veröffentlichte Version unter dieser (wie der Updater vergleicht), nicht das zuletzt veröffentlichte.
    $prev = Select-PreviousRelease -Releases $allReleases -Version $Version
    $prevTag = if ($prev) { $prev.tagName } else { $null }

    $prevHasManifest = $false
    if ($prevTag) {
        $hasManifest = $prev.assets -contains 'aegis-manifest.json'
        $hasSig = $prev.assets -contains 'aegis-manifest.json.minisig'
        if ($hasManifest -and -not $hasSig) {
            throw "Das vorherige Release $prevTag hat ein Manifest, aber keine Signatur (aegis-manifest.json.minisig). Das ist nicht in Ordnung; erst klären."
        }
        $prevHasManifest = $hasManifest -and $hasSig
    }
    if ($prevHasManifest -and $FirstSignedRelease) {
        throw "-FirstSignedRelease ist nicht erlaubt: Das vorherige Release $prevTag hat bereits ein signiertes Manifest."
    }
    if (-not $prevHasManifest) {
        $why = if ($prevTag) { "Das vorherige Release $prevTag enthält kein Manifest samt Signatur." } else { 'Es gibt kein vorheriges Release.' }
        if (-not $FirstSignedRelease) {
            throw "$why Ohne signiertes Vorgänger-Manifest ginge min_secure verloren (Beginn bei 0.0.0). Nur beim allerersten signierten Release mit -FirstSignedRelease erlaubt."
        }
        if (Test-AnyReleaseHasManifest $allReleases) {
            $with = @($allReleases | Where-Object { @($_.assets) -contains 'aegis-manifest.json' } | ForEach-Object { $_.tagName }) -join ', '
            throw "$why -FirstSignedRelease wird abgelehnt: Diese Releases haben schon ein Manifest: $with. Das passt nicht zu einem ersten signierten Release; erst klären."
        }
        Write-Warning "$why Erstes signiertes Release (-FirstSignedRelease): min_secure beginnt bei 0.0.0, mit -Security bei $Version."
    }

    # ---------------------------------------------------------------- 3. Prüfung der Änderungen
    Write-Host ''
    Write-Host '== Änderungen seit dem letzten Release ==' -ForegroundColor Cyan
    $reviewBase = $prevTag
    if (-not $reviewBase) {
        $t = & git describe --tags --abbrev=0 --match 'v*' "$Tag^" 2>$null
        if ($LASTEXITCODE -eq 0) { $reviewBase = $t }
    }
    if ($reviewBase) {
        Write-Host "Vorheriges Release: $reviewBase  ->  neu: $Tag" -ForegroundColor Cyan
        Invoke-Native git @('-c', 'core.quotepath=off', 'log', '--format=%h %an %s', "$reviewBase..HEAD")
        Write-Host ''
        Invoke-Native git @('diff', '--stat', $reviewBase, 'HEAD')
    }
    else {
        Write-Host 'Kein früherer Release-Tag gefunden; zeige die gesamte Historie.' -ForegroundColor Yellow
        Invoke-Native git @('-c', 'core.quotepath=off', 'log', '--format=%h %an %s')
    }
    Write-Host ''
    Write-Host 'Alles, was jetzt signiert wird, ist genau dieser Stand. Jeder Commit und jede Änderung oben muss von dir stammen oder von dir geprüft sein.' -ForegroundColor Yellow
    $answer = Read-Host 'Zum Fortfahren genau "geprüft" eingeben'
    if (-not $answer -or -not $answer.Normalize([Text.NormalizationForm]::FormC).Equals('geprüft', [StringComparison]::Ordinal)) {
        throw 'Nicht bestätigt (erwartet: geprüft). Es wurde nichts gebaut, signiert oder veröffentlicht.'
    }

    # ---------------------------------------------------------------- 4. Prüfungen und Build
    Write-Host ''
    Write-Host '== cargo deny ==' -ForegroundColor Cyan
    if (-not (Get-Command cargo-deny -ErrorAction SilentlyContinue)) {
        $install = Read-Host 'cargo-deny fehlt. Jetzt mit "cargo install cargo-deny --locked" installieren? (j/n)'
        if ($install -ne 'j') { throw 'cargo-deny ist Pflicht für ein Release und wurde nicht installiert.' }
        Invoke-Native cargo @('install', 'cargo-deny', '--locked')
    }
    Invoke-Native cargo @('deny', 'check')

    Write-Host ''
    Write-Host '== Tests ==' -ForegroundColor Cyan
    Invoke-Native cargo @('test', '--workspace', '--locked')

    Write-Host ''
    Write-Host '== Release-Build ==' -ForegroundColor Cyan
    # Wie in der CI: C-Laufzeit statisch einbinden, damit die .exe ohne Visual-C++-Redistributable startet.
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    Invoke-Native cargo @('build', '--release', '--locked', '-p', 'aegis-app', '-p', 'aegis-updater')
    $env:RUSTFLAGS = $OldRustFlags
    # Das Signierwerkzeug wird getrennt gebaut: Es gehört nicht zu den ausgelieferten Dateien.
    Invoke-Native cargo @('build', '--release', '--locked', '-p', 'aegis-sign')

    Invoke-Native git @('diff', '--exit-code', 'Cargo.lock')
    $dirtyAfter = Get-NativeOutput git @('status', '--porcelain')
    if ($dirtyAfter.Count -gt 0) {
        throw "Der Build hat den Arbeitsbaum verändert:`n$($dirtyAfter -join "`n")"
    }

    $builtApp = Join-Path $TargetDir 'release\aegis-app.exe'
    $builtUpdater = Join-Path $TargetDir 'release\aegis-updater.exe'
    $signExe = Join-Path $TargetDir 'release\aegis-sign.exe'
    foreach ($f in $builtApp, $builtUpdater, $signExe) {
        if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "Erwartete Datei fehlt nach dem Build: $f" }
    }

    # ---------------------------------------------------------------- 5. Staging, Manifest, Signatur
    # Frischer Ordner pro Lauf; ins Repository wird nie geschrieben.
    $Stage = Join-Path $env:TEMP "aegis-release-$Tag-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
    $upload = Join-Path $Stage 'upload'
    New-Item -ItemType Directory -Path $upload | Out-Null

    # Das Manifest wird aus genau diesen (umbenannten) Dateien gebaut, und genau diese werden hochgeladen.
    Copy-Item -LiteralPath $builtApp -Destination (Join-Path $upload 'aegis.exe')
    Copy-Item -LiteralPath $builtUpdater -Destination (Join-Path $upload 'aegis-updater.exe')

    $signArgs = @('manifest', '--version', $Version, '--key', $KeyFile,
        '--app', (Join-Path $upload 'aegis.exe'), '--updater', (Join-Path $upload 'aegis-updater.exe'))

    $carried = '0.0.0'
    $carriedFrom = 'Beginn (erstes signiertes Release)'
    if ($prevHasManifest) {
        $prevDir = Join-Path $Stage 'previous'
        New-Item -ItemType Directory -Path $prevDir | Out-Null
        Write-Host ''
        Write-Host "Lade Manifest und Signatur des vorherigen Releases $prevTag ..." -ForegroundColor Cyan
        Invoke-Native gh @('release', 'download', $prevTag, '--repo', $Repo, '--dir', $prevDir,
            '-p', 'aegis-manifest.json', '-p', 'aegis-manifest.json.minisig')
        # Nur zur Anzeige gelesen; die Signatur prüft gleich das Signierwerkzeug gegen TRUSTED_KEYS (sonst Abbruch).
        $prevManifest = Get-Content -LiteralPath (Join-Path $prevDir 'aegis-manifest.json') -Raw -Encoding utf8 | ConvertFrom-Json
        if ([string]$prevManifest.version -cne $prev.Version.Text) {
            throw "Das Manifest von $prevTag nennt die Version '$($prevManifest.version)', erwartet $($prev.Version.Text)."
        }
        $prevMin = ConvertTo-AegisVersion ([string]$prevManifest.min_secure)
        if (-not $prevMin -or [string]$prevManifest.min_secure -cne $prevMin.Text) {
            throw "Das Manifest von $prevTag enthält kein gültiges min_secure ('$($prevManifest.min_secure)')."
        }
        $carried = $prevMin.Text
        $carriedFrom = "übernommen aus $prevTag (Signatur prüft gleich das Signierwerkzeug)"
        # --previous und --previous-sig gehören immer zusammen: Nur ein signiertes Manifest darf min_secure liefern.
        $signArgs += @('--previous', (Join-Path $prevDir 'aegis-manifest.json'),
            '--previous-sig', (Join-Path $prevDir 'aegis-manifest.json.minisig'))
    }
    $expectedMinSecure = if ($Security) { $Version } else { $carried }
    if ($Security) { $signArgs += '--security' }
    $signArgs += @('--out', $upload)

    Write-Host ''
    Write-Host '== min_secure ==' -ForegroundColor Cyan
    Write-Host "Bisheriges min_secure: $carried ($carriedFrom)"
    Write-Host "min_secure des neuen Manifests: $expectedMinSecure$(if ($Security) { ' (Sicherheitsupdate: diese Version)' } else { ' (unverändert übernommen)' })" -ForegroundColor Yellow
    $okMin = Read-Host 'Stimmt das? Erst danach wird signiert. (j/n)'
    if ($okMin -ne 'j') { throw 'min_secure nicht bestätigt. Es wurde nichts signiert oder veröffentlicht.' }

    Write-Host ''
    Write-Host '== Manifest signieren ==' -ForegroundColor Cyan
    Write-Host 'Gleich fragt das Signierwerkzeug nach dem Passwort des Schlüssels.' -ForegroundColor Cyan
    # Ohne Ausgabeumleitung aufrufen, damit die Passwortabfrage am Terminal funktioniert.
    & $signExe @signArgs
    if ($LASTEXITCODE -ne 0) { throw "Das Signieren ist fehlgeschlagen (Exit $LASTEXITCODE)." }

    foreach ($f in $ReleaseFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $upload $f) -PathType Leaf)) { throw "Datei fehlt im Staging-Ordner: $f" }
    }

    # Gegenprobe: Die Prüfsummen im Manifest müssen zu den Dateien passen, die hochgeladen werden.
    $manifest = Get-Content -LiteralPath (Join-Path $upload 'aegis-manifest.json') -Raw -Encoding utf8 | ConvertFrom-Json
    if ($manifest.version -ne $Version) { throw "Das Manifest enthält die Version $($manifest.version), erwartet $Version." }
    if ([bool]$manifest.security -ne [bool]$Security) { throw 'Das Sicherheits-Kennzeichen im Manifest passt nicht zum Schalter -Security.' }
    if ([string]$manifest.min_secure -cne $expectedMinSecure) { throw "min_secure im Manifest ist $($manifest.min_secure), bestätigt war $expectedMinSecure." }
    foreach ($name in 'aegis.exe', 'aegis-updater.exe') {
        $entry = $manifest.files | Where-Object { $_.name -eq $name }
        $actual = (Get-FileHash -LiteralPath (Join-Path $upload $name) -Algorithm SHA256).Hash.ToLowerInvariant()
        if (-not $entry -or $entry.sha256 -ne $actual) { throw "Die Prüfsumme von $name im Manifest passt nicht zur Datei." }
    }

    # ---------------------------------------------------------------- 6. Zip + .sha256
    $zipName = "aegis-$Tag-windows-x64"
    $pkg = Join-Path $Stage $zipName
    New-Item -ItemType Directory -Path $pkg, (Join-Path $pkg 'licenses') | Out-Null
    Copy-Item -LiteralPath (Join-Path $upload 'aegis.exe') -Destination $pkg
    Copy-Item -LiteralPath (Join-Path $upload 'aegis-updater.exe') -Destination $pkg
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'README.md'), (Join-Path $RepoRoot 'LICENSE') -Destination $pkg
    # Die eingebetteten Schriften stehen unter der SIL OFL 1.1; die Lizenz muss mitgeliefert werden.
    Copy-Item -Path (Join-Path $RepoRoot 'crates\aegis-app\assets\fonts\OFL-*.txt') -Destination (Join-Path $pkg 'licenses')

    $zip = Join-Path $upload "$zipName.zip"
    Compress-Archive -Path (Join-Path $pkg '*') -DestinationPath $zip
    $zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    "$zipHash  $zipName.zip" | Out-File -Encoding ascii -LiteralPath "$zip.sha256"

    $uploadList = @($zip, "$zip.sha256") + @($ReleaseFiles | ForEach-Object { Join-Path $upload $_ })

    # ---------------------------------------------------------------- 7. Veröffentlichen
    Write-Host ''
    Write-Host '== Veröffentlichen ==' -ForegroundColor Cyan
    Write-Host "Version $Version$(if ($Security) { ' (Sicherheitsupdate)' }) mit diesen Dateien:" -ForegroundColor Cyan
    $uploadList | ForEach-Object { Write-Host "  $(Split-Path -Leaf $_)" }
    $go = Read-Host "Release $Tag jetzt auf GitHub veröffentlichen? (j/n)"
    if ($go -ne 'j') { throw "Nicht veröffentlicht. Die fertigen Dateien liegen unter $upload." }

    Invoke-Native gh (@('release', 'create', $Tag, '--repo', $Repo, '--verify-tag', '--title', "Aegis $Tag", '--generate-notes') + $uploadList)

    $published = ((Get-NativeOutput gh @('release', 'view', $Tag, '--repo', $Repo, '--json', 'assets')) -join "`n" | ConvertFrom-Json).assets.name
    foreach ($f in $uploadList | ForEach-Object { Split-Path -Leaf $_ }) {
        if ($published -notcontains $f) { throw "Nach dem Veröffentlichen fehlt die Datei $f im Release." }
    }

    $Failed = $false
    Write-Host ''
    Write-Host "Fertig: https://github.com/$Repo/releases/tag/$Tag" -ForegroundColor Green
    Write-Host 'Erinnerung: Das Manifest läuft nach 7 Tagen ab. Mindestens wöchentlich scripts\refresh-manifest.ps1 ausführen, sonst meldet der Updater "nicht bestätigt".' -ForegroundColor Yellow
}
catch {
    Write-Host ''
    Write-Host "Abbruch: $($_.Exception.Message)" -ForegroundColor Red
    if ($Stage -and (Test-Path -LiteralPath $Stage)) {
        Write-Host "Der Staging-Ordner bleibt zur Fehlersuche erhalten: $Stage" -ForegroundColor Yellow
    }
    exit 1
}
finally {
    $env:RUSTFLAGS = $OldRustFlags
    if (-not $Failed -and $Stage -and (Test-Path -LiteralPath $Stage)) {
        Remove-Item -LiteralPath $Stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}
