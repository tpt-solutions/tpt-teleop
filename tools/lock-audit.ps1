#!/usr/bin/env pwsh
# Phase 14, item 3: verify the hot path uses no locking primitives.
#
# Scans every library `src/` tree (the compiled hot path) for mutex / rwlock
# / third-party locking crates. A zero-lock data plane is a hard requirement
# for the deterministic real-time loop, so any hit in `src/` is a hard failure.
# Lock usage inside `tests/`/`benches/` is reported separately (informational
# only) because those are not on the runtime critical path.

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')

$lockPatterns = @(
    'Mutex', 'RwLock', 'parking_lot', 'lazy_static', 'once_cell',
    'std::sync::Lock', 'spin::Mutex', 'crossbeam::', 'std::sync::Condvar'
)

# --- Hot path: crates/*/src ------------------------------------------------
$srcRoots = Get-ChildItem -Directory -Path (Join-Path $root 'crates') -Recurse -Depth 2 |
    Where-Object { $_.FullName.Replace('\', '/') -match '/src$' }
$hotFiles = $srcRoots | ForEach-Object { Get-ChildItem -File -Path $_.FullName -Filter *.rs -Recurse }

$hotHits = @()
foreach ($f in $hotFiles) {
    foreach ($p in $lockPatterns) {
        if (Select-String -Path $f.FullName -Pattern $p -SimpleMatch -Quiet) {
            $hotHits += "$($f.FullName.replace($root.Path, '')) : $p"
        }
    }
}

# --- Off-path: tests/benches (informational only) -------------------------
$offFiles = Get-ChildItem -File -Path (Join-Path $root 'crates') -Filter *.rs -Recurse |
    Where-Object { $_.FullName.Replace('\', '/') -match '/(tests|benches)/' }
$offHits = @()
foreach ($f in $offFiles) {
    foreach ($p in $lockPatterns) {
        if (Select-String -Path $f.FullName -Pattern $p -SimpleMatch -Quiet) {
            $offHits += "$($f.FullName.replace($root.Path, '')) : $p"
        }
    }
}

if ($offHits.Count -gt 0) {
    Write-Host "INFO: locking primitives in tests/benches (not on the hot path):"
    $offHits | ForEach-Object { Write-Host "  $_" }
}

if ($hotHits.Count -eq 0) {
    Write-Host "PASS: no locking primitives in the hot-path library source."
    exit 0
} else {
    Write-Host "FAIL: locking primitives found in hot-path source:"
    $hotHits | ForEach-Object { Write-Host "  $_" }
    exit 1
}
