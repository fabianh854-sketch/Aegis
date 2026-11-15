#Requires -Version 7
<#
.SYNOPSIS
    Gemeinsame Funktionen für release.ps1 und refresh-manifest.ps1 (wird per Dot-Sourcing geladen).

.DESCRIPTION
    Bis auf Get-AegisReleases sind alle Funktionen rein (kein Netz, keine Dateien) und lassen sich mit
    festen Beispieldaten prüfen. Ein Release-Objekt hat die Felder tagName, isDraft, isPrerelease und
    assets (Liste der Dateinamen).
#>

# Version wie im Updater (aegis-update/src/version.rs): MAJOR.MINOR.PATCH, optional mit führendem v,
# nur Ziffern, keine führende Null. Alles andere ergibt $null.
function ConvertTo-AegisVersion {
    param([AllowNull()][AllowEmptyString()][string]$Text)
    if ($null -eq $Text) { return $null }
    $m = [regex]::Match($Text, '^v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')
    if (-not $m.Success) { return $null }
    $parts = @()
    foreach ($i in 1..3) {
        $n = [uint64]0
        # Wie u32 im Updater: größere Zahlen sind keine gültige Version.
        if (-not [uint64]::TryParse($m.Groups[$i].Value, [ref]$n) -or $n -gt [uint32]::MaxValue) { return $null }
        $parts += $n
    }
    return [pscustomobject]@{ Major = $parts[0]; Minor = $parts[1]; Patch = $parts[2]; Text = "$($parts[0]).$($parts[1]).$($parts[2])" }
}

# -1, 0 oder 1 (wie a <=> b).
function Compare-AegisVersion {
    param([Parameter(Mandatory)]$A, [Parameter(Mandatory)]$B)
    foreach ($f in 'Major', 'Minor', 'Patch') {
        if ($A.$f -lt $B.$f) { return -1 }
        if ($A.$f -gt $B.$f) { return 1 }
    }
    return 0
}

# Liest die Einträge von TRUSTED_KEYS aus dem Quelltext von keys.rs.
# Kommentare werden nur außerhalb von Zeichenketten entfernt ("//" darf in Base64 vorkommen).
# Unerwartetes Format oder ein Eintrag, der kein Minisign-Public-Key ist, ergibt @() (Abbruch, nie Weitermachen).
function Get-TrustedKeysFromSource {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Source)
    $clean = [regex]::Replace($Source, '"[^"\\]*"|/\*.*?\*/|//[^\r\n]*', {
            param($m)
            if ($m.Value.StartsWith('"')) { $m.Value } else { '' }
        }, 'Singleline')
    $m = [regex]::Match($clean, 'TRUSTED_KEYS\s*:\s*&\[\s*&str\s*\]\s*=\s*&\[(?<body>[^\]]*)\]\s*;')
    if (-not $m.Success) { return @() }
    $body = $m.Groups['body'].Value
    $entries = @([regex]::Matches($body, '"([^"]*)"') | ForEach-Object { $_.Groups[1].Value })
    # Außer Zeichenketten, Kommas und Leerraum darf nichts im Rumpf stehen.
    if (([regex]::Replace($body, '"[^"]*"', '') -replace '[\s,]', '') -ne '') { return @() }
    foreach ($key in $entries) {
        # Minisign-Public-Key: "RW" + 54 Base64-Zeichen (2 Byte Algorithmus, 8 Byte ID, 32 Byte Schlüssel).
        if ($key -cnotmatch '^RW[A-Za-z0-9+/]{54}$') { return @() }
    }
    return $entries
}

# Nur veröffentlichte Releases (kein Entwurf, kein Vorab-Release) mit gültigem Versions-Tag, mit Feld Version.
function Get-PublishedVersionedReleases {
    param([AllowNull()][object[]]$Releases)
    foreach ($r in @($Releases)) {
        if ($null -eq $r -or $r.isDraft -or $r.isPrerelease) { continue }
        $v = ConvertTo-AegisVersion $r.tagName
        if ($null -eq $v) { continue }
        [pscustomobject]@{ tagName = $r.tagName; assets = @($r.assets); Version = $v }
    }
}

function Select-HighestRelease {
    param([AllowNull()][object[]]$Candidates)
    $best = $null
    foreach ($c in @($Candidates)) {
        if ($null -eq $c) { continue }
        if ($null -eq $best -or (Compare-AegisVersion $c.Version $best.Version) -gt 0) { $best = $c }
    }
    return $best
}

# Vorgänger eines neuen Releases: höchste Version UNTER $Version (nicht das zuletzt veröffentlichte).
function Select-PreviousRelease {
    param([AllowNull()][object[]]$Releases, [Parameter(Mandatory)][string]$Version)
    $new = ConvertTo-AegisVersion $Version
    if ($null -eq $new) { throw "Ungültige Version '$Version'." }
    $below = Get-PublishedVersionedReleases $Releases | Where-Object { (Compare-AegisVersion $_.Version $new) -lt 0 }
    return Select-HighestRelease @($below)
}

# Das Release, das auch der Updater als neuestes ansieht: höchste Version.
function Select-LatestRelease {
    param([AllowNull()][object[]]$Releases)
    return Select-HighestRelease @(Get-PublishedVersionedReleases $Releases)
}

# Hat irgendein Release (auch Entwurf oder Vorab-Release) ein Manifest? Dann gibt es keinen "ersten" signierten Release mehr.
function Test-AnyReleaseHasManifest {
    param([AllowNull()][object[]]$Releases)
    foreach ($r in @($Releases)) {
        if ($null -ne $r -and @($r.assets) -contains 'aegis-manifest.json') { return $true }
    }
    return $false
}

# Vergleicht das neu signierte Manifest mit dem heruntergeladenen. Liefert die Liste der Abweichungen (leer = in Ordnung).
function Test-RefreshedManifest {
    param([Parameter(Mandatory)]$Old, [Parameter(Mandatory)]$New, [Parameter(Mandatory)][string]$Tag)
    $problems = @()
    $tagVersion = ConvertTo-AegisVersion $Tag
    if ($null -eq $tagVersion) { return @("Ungültiger Tag '$Tag'.") }
    foreach ($pair in @(@('alten', $Old), @('neuen', $New))) {
        $v = ConvertTo-AegisVersion ([string]$pair[1].version)
        if ($null -eq $v -or [string]$pair[1].version -cne $tagVersion.Text) {
            $problems += "Version im $($pair[0]) Manifest ist '$($pair[1].version)', erwartet $($tagVersion.Text) (Tag $Tag)."
        }
    }
    foreach ($field in 'schema', 'security', 'min_secure') {
        if ([string]$Old.$field -cne [string]$New.$field) {
            $problems += "Feld $field hat sich geändert: '$($Old.$field)' -> '$($New.$field)'."
        }
    }
    $files = { param($m) (@($m.files) | ForEach-Object { "$($_.name)=$($_.sha256)" } | Sort-Object) -join ';' }
    $oldFiles = & $files $Old
    $newFiles = & $files $New
    if ($oldFiles -cne $newFiles -or $oldFiles -eq '') {
        $problems += "Dateien oder Prüfsummen haben sich geändert: '$oldFiles' -> '$newFiles'."
    }
    return , $problems
}

# Alle Releases des Repositorys über die GitHub-API (alle Seiten, auch Entwürfe), als Release-Objekte.
function Get-AegisReleases {
    param([Parameter(Mandatory)][string]$Repo)
    $jq = '.[] | {tagName: .tag_name, isDraft: .draft, isPrerelease: .prerelease, assets: [.assets[].name]} | tojson'
    $lines = & gh api --paginate "repos/$Repo/releases?per_page=100" --jq $jq
    if ($LASTEXITCODE -ne 0) { throw "Befehl fehlgeschlagen (Exit $LASTEXITCODE): gh api repos/$Repo/releases" }
    return , @(@($lines) | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
}
