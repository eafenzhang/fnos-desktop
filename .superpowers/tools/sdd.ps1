param(
  [Parameter(Mandatory=$true)][string]$Cmd,
  [string]$Plan = 'docs\superpowers\plans\2026-09-28-fnos-desktop-shell.md',
  [int]$N = 0,
  [string]$Base = '',
  [string]$Head = 'HEAD'
)

$root = (git rev-parse --show-toplevel).Trim()
$slug = [System.IO.Path]::GetFileNameWithoutExtension($Plan)
$ws   = Join-Path $root ".superpowers\sdd\$slug"

switch ($Cmd) {
  'workspace' {
    New-Item -ItemType Directory -Force -Path $ws | Out-Null
    Write-Output $ws
  }
  'brief' {
    if ($N -lt 1) { throw 'brief 需要 -N <task 编号>' }
    New-Item -ItemType Directory -Force -Path $ws | Out-Null
    $out = Join-Path $ws "task-$N-brief.md"
    $lines = Get-Content (Join-Path $root $Plan) -Encoding UTF8
    $inTask = $false; $inFence = $false; $buf = New-Object System.Collections.Generic.List[string]
    foreach ($line in $lines) {
      if ($line -match '^```') { $inFence = -not $inFence }
      if (-not $inFence -and $line -match '^#+[ \t]+Task[ \t]+([0-9]+)') {
        if ($inTask) { break }
        $inTask = ([int]$Matches[1] -eq $N)
      }
      if ($inTask) { $buf.Add($line) }
    }
    if ($buf.Count -eq 0) { throw "task $N not found in $Plan" }
    Set-Content -Path $out -Value $buf -Encoding UTF8
    Write-Output "$out ($($buf.Count) lines)"
  }
  'package' {
    if (-not $Base) { throw 'package 需要 -Base <sha>' }
    $b7 = (git rev-parse --short $Base).Trim(); $h7 = (git rev-parse --short $Head).Trim()
    $out = Join-Path $ws "review-$b7..$h7.diff"
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine("# Review package: $Base..$Head"); [void]$sb.AppendLine()
    [void]$sb.AppendLine('## Commits'); [void]$sb.AppendLine((git log --oneline "$Base..$Head" | Out-String))
    [void]$sb.AppendLine('## Files changed'); [void]$sb.AppendLine((git diff --stat "$Base..$Head" | Out-String))
    [void]$sb.AppendLine('## Diff'); [void]$sb.AppendLine((git diff -U10 "$Base..$Head" | Out-String))
    Set-Content -Path $out -Value $sb.ToString() -Encoding UTF8
    $count = (git rev-list --count "$Base..$Head").Trim()
    Write-Output "$out ($count commits, $((Get-Item $out).Length) bytes)"
  }
  default { throw "unknown cmd: $Cmd" }
}
