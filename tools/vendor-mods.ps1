<#
.SYNOPSIS
  Vendor the upstream fnOS_UI_Mods snapshot from .ref/fnOS_UI_Mods/ into
  src-tauri/assets/fnos-mods/ and emit a compliance NOTICE carrying the pinned
  commit, per-file SHA-256 and sizes.

.DESCRIPTION
  Upstream: https://github.com/aurysian-yan/fnOS_UI_Mods
  Pinned commit: 483c3e2e217faebc1be45b4e824865854a61e3dd

  Contract (later tasks reference these files with
  include_str!("../assets/fnos-mods/<name>")):
  * every vendored file is a BYTE-IDENTICAL copy of the snapshot file
    (Copy-Item, never a text round-trip through PowerShell strings);
  * the destination directory is rebuilt from scratch, which makes the script
    idempotent: repeated runs produce identical hashes and file lists;
  * NOTICE is written as BOM-less UTF-8. Windows PowerShell 5.1 writes a BOM
    with `Set-Content -Encoding utf8`, which would corrupt a compliance file
    read by other tools, so UTF8Encoding($false) is used explicitly;
  * the script self-checks: it recomputes every SHA-256 from the copied files
    and compares it with the value recorded in NOTICE, verifies each copy is
    byte-identical to its source, and verifies the file list has no duplicates.

  NOTE: this script is intentionally ASCII-only. Windows PowerShell 5.1 reads a
  BOM-less .ps1 file using the ANSI code page, so non-ASCII characters in
  comments can be mis-decoded and silently swallow the following code line.
  That failure mode was hit twice while building this task; keeping the file
  ASCII-only makes it immune to BOM loss and independent of the console
  code page.

.PARAMETER Fetch
  Optional. First complete the .ref/ snapshot from raw.githubusercontent.com
  (prefect_icon icons and icons/*), retrying once on failure. URLs point at
  -Commit rather than `main`, so a moving upstream branch can never silently
  change what gets vendored. Off by default: by default the script only
  vendors what is already in the local snapshot.

.PARAMETER SourceRoot
  Upstream snapshot directory. Defaults to <repo>\.ref\fnOS_UI_Mods.

.PARAMETER Destination
  Vendor output directory. Defaults to <repo>\src-tauri\assets\fnos-mods.

.PARAMETER Commit
  Pinned upstream commit (recorded in NOTICE and used as the pin for -Fetch).

.PARAMETER RepoUrl
  Upstream repository URL (recorded in NOTICE).

.EXAMPLE
  powershell -File D:\fnOS-desktop\tools\vendor-mods.ps1
  powershell -File D:\fnOS-desktop\tools\vendor-mods.ps1 -Fetch
#>
[CmdletBinding()]
param(
  [string]$SourceRoot,
  [string]$Destination,
  [string]$Commit  = '483c3e2e217faebc1be45b4e824865854a61e3dd',
  [string]$RepoUrl = 'https://github.com/aurysian-yan/fnOS_UI_Mods',
  [switch]$Fetch
)

$ErrorActionPreference = 'Stop'
$ProgressPreference    = 'SilentlyContinue'

# NOTE: in Windows PowerShell 5.1 $PSScriptRoot may still be empty while param
# defaults are evaluated, so the path defaults are resolved here instead.
$scriptDir = $PSScriptRoot
if (-not $scriptDir) { $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $SourceRoot)  { $SourceRoot  = Join-Path $scriptDir '..\.ref\fnOS_UI_Mods' }
if (-not $Destination) { $Destination = Join-Path $scriptDir '..\src-tauri\assets\fnos-mods' }

if (-not (Test-Path -LiteralPath $SourceRoot)) { throw "snapshot directory not found: $SourceRoot" }
$src = (Resolve-Path -LiteralPath $SourceRoot).Path
$dst = [System.IO.Path]::GetFullPath($Destination)

# --- fixed vendor list (this is also the order used inside NOTICE) ----------
$fixed = @(
  'LICENSE', 'content-script.js', 'mod.js', 'basic_mod.css',
  'windows_titlebar_mod.css', 'mac_titlebar_mod.css',
  'classic_launchpad_mod.css', 'spotlight_launchpad_mod.css',
  'desktop_icon_mod.css', 'lockscreen_mod.css',
  'prefect_icon/icon-map.json'
)

# Icon file names exactly as they exist upstream at the pinned commit. The case
# matters: upstream ships prefect_icon/panIndex.png while icon-map.json records
# the lower-case key "panindex", so deriving a download URL from the map value
# 404s. The list is cross-checked against icon-map.json below, and the script
# fails loudly if the map ever references a name that is not in this manifest.
$iconFiles = @(
  'alist.png', 'emby.png', 'home-assistant.png', 'icloud.png', 'it-tools.png',
  'kodi.png', 'one-panel.png', 'oray-hsk.png', 'panIndex.png', 'qbittorrent.png',
  'quarkpan.png', 'syncthing.png', 'transmission.png', 'xunlei.png'
)

function Get-IconMapKeys {
  param([string]$MapPath)
  # The real icon-map.json does NOT contain "prefect_icon/x.png" path literals;
  # it stores bare icon names in the values of its sub-objects
  # (aliases / appNameMap / serviceIconMap / keyMap). So expected icon file
  # names are derived as "prefect_icon/<value>.png", compared
  # case-insensitively because upstream ships panIndex.png while the map value
  # is the lower-case "panindex".
  $map = Get-Content -LiteralPath $MapPath -Raw -Encoding UTF8 | ConvertFrom-Json
  $keys = New-Object System.Collections.Generic.HashSet[string] ([System.StringComparer]::OrdinalIgnoreCase)
  foreach ($section in $map.PSObject.Properties) {
    $node = $section.Value
    if ($null -eq $node) { continue }
    if ($node -is [string]) { [void]$keys.Add([string]$node); continue }
    foreach ($p in $node.PSObject.Properties) {
      if ($p.Value -is [string] -and $p.Value -match '^[a-z0-9-]+$') { [void]$keys.Add([string]$p.Value) }
    }
  }
  return [string[]]@($keys)
}

function Invoke-UpstreamFetch {
  param([string]$RelativePath, [string]$DestinationPath)
  # Pin the download to the immutable commit (not `main`) so the bytes always
  # match the commit recorded in NOTICE.
  $url = "https://raw.githubusercontent.com/aurysian-yan/fnOS_UI_Mods/$Commit/$RelativePath"
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $DestinationPath) | Out-Null
  try {
    Invoke-WebRequest -Uri $url -OutFile $DestinationPath -TimeoutSec 60
  } catch {
    Write-Warning "fetch failed, retrying once: $RelativePath ($($_.Exception.Message))"
    Start-Sleep -Seconds 2
    Invoke-WebRequest -Uri $url -OutFile $DestinationPath -TimeoutSec 60
  }
  Write-Host "fetched  $RelativePath"
}

if ($Fetch) {
  Write-Host 'Fetch: completing missing upstream files in the .ref snapshot ...'
  foreach ($f in @('prefect_icon/icon-map.json', 'icons/icon16.png', 'icons/icon32.png',
                   'icons/icon48.png', 'icons/icon128.png')) {
    $target = Join-Path $src $f
    if (-not (Test-Path -LiteralPath $target)) { Invoke-UpstreamFetch -RelativePath $f -DestinationPath $target }
  }
  $mapPath = Join-Path $src 'prefect_icon/icon-map.json'
  if (-not (Test-Path -LiteralPath $mapPath)) { throw "missing icon-map.json after fetch: $mapPath" }
  foreach ($name in $iconFiles) {
    $target = Join-Path $src "prefect_icon/$name"
    if (-not (Test-Path -LiteralPath $target)) { Invoke-UpstreamFetch -RelativePath "prefect_icon/$name" -DestinationPath $target }
  }
  Write-Host ''
}

# --- build the vendor list: fixed files + prefect_icon/*.png actually present
$files = New-Object System.Collections.Generic.List[string]
foreach ($f in $fixed) { [void]$files.Add($f) }

$iconDir  = Join-Path $src 'prefect_icon'
$pngNames = @(Get-ChildItem -LiteralPath $iconDir -Filter '*.png' -File -ErrorAction SilentlyContinue |
              ForEach-Object { $_.Name } | Sort-Object -CaseSensitive)
foreach ($n in $pngNames) { [void]$files.Add("prefect_icon/$n") }

# Validate: every icon in the pinned manifest must be in the snapshot, and
# every icon name referenced by icon-map.json must correspond to a manifest
# entry (case-insensitive). Missing files are an error, never invented.
$missingFiles = @($iconFiles | Where-Object { -not (Test-Path -LiteralPath (Join-Path $iconDir $_)) })
if ($missingFiles.Count -gt 0) {
  throw ("snapshot is missing pinned icons: {0} -- run with -Fetch to complete the snapshot (never invent files)" -f ($missingFiles -join ', '))
}
$stems = @($iconFiles | ForEach-Object { [System.IO.Path]::GetFileNameWithoutExtension($_) })
$declared = @(Get-IconMapKeys -MapPath (Join-Path $src 'prefect_icon/icon-map.json'))
$undeclared = @($declared | Where-Object { $key = $_; -not ($stems | Where-Object { $_ -ieq $key }) })
if ($undeclared.Count -gt 0) {
  throw ("icon-map.json references icon names absent from the pinned manifest: {0} -- upstream changed; review and update the icon manifest list in this script" -f ($undeclared -join ', '))
}
$unreferenced = @($pngNames | Where-Object { $declared -notcontains [System.IO.Path]::GetFileNameWithoutExtension($_) })
if ($unreferenced.Count -gt 0) {
  Write-Warning ("snapshot icons not referenced by icon-map.json (vendored as-is anyway): {0}" -f ($unreferenced -join ', '))
}

# --- rebuild destination from scratch (idempotency, no stale/duplicate rows)
if (Test-Path -LiteralPath $dst) { Remove-Item -Recurse -Force -LiteralPath $dst }
New-Item -ItemType Directory -Force -Path $dst | Out-Null

$lines   = New-Object System.Collections.Generic.List[string]
$entries = New-Object System.Collections.Generic.List[object]

# [char]0x2014 keeps the em dash from the spec's wording while leaving this
# script file pure ASCII (see the encoding note in the header).
$lines.Add('fnOS Desktop Shell ' + [char]0x2014 + ' vendored third-party resources')
$lines.Add('')
$lines.Add("Upstream: $RepoUrl")
$lines.Add("Pinned commit: $Commit")
$lines.Add('License: FnOS UI Mods Non-Commercial License 1.0 (see LICENSE, copied verbatim below)')
$lines.Add('')
$lines.Add('Packaging changes made by this project (upstream files themselves are UNMODIFIED):')
$lines.Add('  1. chrome.* compatibility shim injected ahead of content-script.js (inject/shim.js)')
$lines.Add('  2. chrome.runtime.getURL() reimplemented to return data: URLs instead of chrome-extension:// URLs')
$lines.Add('  3. mod.js delivered through the shim-provided data: URL, with a MutationObserver fallback that')
$lines.Add('     executes the unmodified original if the data: script is blocked by page CSP')
$lines.Add('  4. configuration is supplied by the host application instead of chrome.storage')
$lines.Add('  5. content-script.js is not evaluated as a top-level document-start script: its unmodified')
$lines.Add('     bytes are embedded in a generated wrapper (src-tauri/src/injector.rs) that runs them only')
$lines.Add('     once document.documentElement exists (MutationObserver on document, DOMContentLoaded')
$lines.Add('     fallback) and records a synchronous throw as window.__FNOS_UPSTREAM_ERROR__')
$lines.Add('')
$lines.Add('File SHA-256:')

foreach ($f in $files) {
  $s = Join-Path $src ($f -replace '/', '\')
  if (-not (Test-Path -LiteralPath $s -PathType Leaf)) { throw "missing vendored source: $f" }
  $o = Join-Path $dst ($f -replace '/', '\')
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $o) | Out-Null
  Copy-Item -LiteralPath $s -Destination $o -Force
  $hash = (Get-FileHash -LiteralPath $o -Algorithm SHA256).Hash.ToLower()
  $size = (Get-Item -LiteralPath $o).Length
  $lines.Add(('  {0}  {1,9}  {2}' -f $hash, $size, $f))
  $entries.Add([pscustomobject]@{ Path = $f; Hash = $hash; Size = $size })
}

# Append the full LICENSE text so the "copied verbatim below" claim above is
# literally true and clause 2 (keep the license text on redistribution) holds.
$lines.Add('')
$lines.Add('LICENSE (verbatim):')
$licenseText = [System.IO.File]::ReadAllText((Join-Path $src 'LICENSE'))
foreach ($l in ($licenseText -split "`r?`n")) { $lines.Add($l) }
while ($lines.Count -gt 0 -and $lines[$lines.Count - 1] -eq '') { $lines.RemoveAt($lines.Count - 1) }

# BOM-less UTF-8 (do NOT use Set-Content -Encoding utf8: PS 5.1 writes a BOM).
$noticePath = Join-Path $dst 'NOTICE'
$utf8NoBom  = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllLines($noticePath, [string[]]$lines.ToArray(), $utf8NoBom)

Write-Host "vendored $($entries.Count) files into $dst"

# --- self-check -------------------------------------------------------------
$failures = New-Object System.Collections.Generic.List[string]

# 1) recompute hashes from the copied files, and prove each copy equals its source
$mismatches = 0
foreach ($e in $entries) {
  $o = Join-Path $dst ($e.Path -replace '/', '\')
  $s = Join-Path $src ($e.Path -replace '/', '\')
  $h = (Get-FileHash -LiteralPath $o -Algorithm SHA256).Hash.ToLower()
  if ($h -ne $e.Hash) { $mismatches++; $failures.Add("hash mismatch in NOTICE: $($e.Path)"); Write-Host "MISMATCH $($e.Path)" }
  if ((Get-Item -LiteralPath $o).Length -ne $e.Size) { $failures.Add("size mismatch: $($e.Path)") }
  if ((Get-FileHash -LiteralPath $s -Algorithm SHA256).Hash.ToLower() -ne $h) {
    $failures.Add("copy is not byte-identical to snapshot: $($e.Path)"); Write-Host "COPY-DIFF $($e.Path)"
  }
}
Write-Host "mismatches=$mismatches"

# 2) read NOTICE back and require every line to match what was written
$noticeLines   = [System.IO.File]::ReadAllLines($noticePath, [System.Text.Encoding]::UTF8)
$expectedLines = [string[]]$lines.ToArray()
if ($noticeLines.Count -ne $expectedLines.Count) {
  $failures.Add("NOTICE line count $($noticeLines.Count) != expected $($expectedLines.Count)")
} else {
  for ($i = 0; $i -lt $expectedLines.Count; $i++) {
    if ($noticeLines[$i] -cne $expectedLines[$i]) { $failures.Add("NOTICE line $($i + 1) differs on read-back"); break }
  }
}
$bytes        = [System.IO.File]::ReadAllBytes($noticePath)
$noticeHasBom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
if ($noticeHasBom) { $failures.Add('NOTICE carries a UTF-8 BOM on disk') }

# 3) every recorded entry must be parseable exactly once
$pattern = '^\s{2}([0-9a-f]{64})\s+(\d+)\s+(\S.*)$'
$parsed  = @()
foreach ($l in $noticeLines) {
  $m = [regex]::Match($l, $pattern)
  if ($m.Success) {
    $parsed += [pscustomobject]@{ Hash = $m.Groups[1].Value; Size = [int64]$m.Groups[2].Value; Path = $m.Groups[3].Value.Trim() }
  }
}
if ($parsed.Count -ne $entries.Count) { $failures.Add("NOTICE file-list entries $($parsed.Count) != vendored $($entries.Count)") }
$dupes = @($parsed | Group-Object Path | Where-Object { $_.Count -gt 1 } | ForEach-Object { $_.Name })
if ($dupes.Count -gt 0) { $failures.Add("duplicate NOTICE entries: $($dupes -join ', ')") }
Write-Host "duplicates=$($dupes.Count)"
Write-Host "notice-entries=$($parsed.Count) expected=$($entries.Count)"
Write-Host "notice-bom=$noticeHasBom"

if ($failures.Count -gt 0) {
  foreach ($f in $failures) { Write-Host "FAIL: $f" }
  throw "vendor-mods self-check failed ($($failures.Count) problem(s))"
}
Write-Host 'self-check OK'
