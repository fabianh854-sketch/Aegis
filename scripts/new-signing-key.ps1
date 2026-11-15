#Requires -Version 7
<#
.SYNOPSIS
    Erzeugt einmalig das Signierschlüsselpaar für Aegis-Releases.

.DESCRIPTION
    Ruft `aegis-sign keygen` auf. Das Werkzeug fragt das Passwort selbst ab (zweimal, mindestens 12 Zeichen);
    das Skript sieht, speichert und zeigt das Passwort nie. Der verschlüsselte private Schlüssel landet in
    %USERPROFILE%\.aegis-signing\aegis.key, der öffentliche in aegis.pub. Bestehende Schlüssel werden nie überschrieben.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

try {
    $repo = Split-Path -Parent $PSScriptRoot
    $keyDir = Join-Path $env:USERPROFILE '.aegis-signing'

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "Das Werkzeug 'cargo' wurde nicht gefunden."
    }

    Push-Location $repo
    try {
        Write-Host "Erzeuge Schlüsselpaar in $keyDir ..." -ForegroundColor Cyan
        Write-Host "Gleich wird zweimal nach einem Passwort gefragt (mindestens 12 Zeichen)." -ForegroundColor Cyan
        # Direkt aufrufen, ohne Ausgabe abzufangen: Das Werkzeug fragt das Passwort selbst am Terminal ab.
        cargo run --release --locked -p aegis-sign -- keygen --dir $keyDir
        if ($LASTEXITCODE -ne 0) { throw "Die Schlüsselerzeugung ist fehlgeschlagen (Exit $LASTEXITCODE)." }
    }
    finally {
        Pop-Location
    }

    Write-Host ''
    Write-Host 'Schlüssel erzeugt. Nächste Schritte:' -ForegroundColor Green
    Write-Host @"
  1. Den oben ausgegebenen öffentlichen Schlüssel (die lange Base64-Zeile, steht auch in
     $keyDir\aegis.pub) in crates\aegis-update\src\keys.rs bei TRUSTED_KEYS eintragen,
     z. B.  pub const TRUSTED_KEYS: &[&str] = &["RWQ..."];
  2. Denselben Schlüssel (und seinen Fingerabdruck) im README unter "Download prüfen" eintragen
     und beides committen.
  3. $keyDir\aegis.key zusätzlich OFFLINE sichern (z. B. USB-Stick im Safe) und das Passwort
     getrennt davon aufbewahren. Geht der Schlüssel verloren, kann kein Update mehr signiert werden.
  4. aegis.key niemals committen, hochladen oder per Mail/Chat verschicken.
  5. Zwei-Faktor-Anmeldung bei GitHub aktivieren, falls noch nicht geschehen.
  6. Danach (neues Release): Version in Cargo.toml erhöhen, committen, Tag setzen und pushen,
     dann scripts\release.ps1 -Version X.Y.Z ausführen. Das erste Release mit Updater
     müssen Nutzer von v0.1.0 einmal manuell installieren.
"@
}
catch {
    Write-Host "Abbruch: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
