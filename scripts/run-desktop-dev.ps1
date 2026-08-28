<#
.SYNOPSIS
    Launch the Memory Vault desktop app against local dev fixtures.

.DESCRIPTION
    Running the built exe directly bypasses `.cargo/config.toml`, so every
    environment variable the app needs must be set by hand — including
    LANCE_MEM_POOL_SIZE (ADR-038), which dev `cargo run` inherits silently but
    an installed build does NOT. That makes this script the closest thing we
    have to installed-build conditions, and the same set the MSI launcher will
    have to provide.

    Previously this recipe lived as prose in HANDOFF.md and was retyped by hand
    each session. Six paths typed from memory is a mistake waiting to happen —
    a half-set reranker pair in particular fails SILENTLY, degrading search
    rather than erroring.

.PARAMETER Release
    Launch the release build instead of debug.

.PARAMETER NoReranker
    Deliberately omit the reranker paths. Use to reproduce the pre-ADR-087
    behaviour (search on the cosine gate) for an A/B comparison.

.EXAMPLE
    .\scripts\run-desktop-dev.ps1
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$NoReranker
)

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$fixtures = Join-Path $repo 'crates\vault-embedding\test-fixtures'
$bge = Join-Path $fixtures 'bge-small-en-v1.5'
$qwen = Join-Path $fixtures 'qwen3-reranker-0.6b-seq-cls'

$profileDir = if ($Release) { 'release' } else { 'debug' }
$exe = Join-Path $repo "target\$profileDir\zaaheen-desktop.exe"

# ── Preflight: fail loudly here rather than mysteriously at runtime ──────
$required = [ordered]@{
    'app binary'      = $exe
    'ORT dylib'       = Join-Path $bge 'onnxruntime.dll'
    'embedding model' = Join-Path $bge 'model.onnx'
    'embedding token' = Join-Path $bge 'tokenizer.json'
}
if (-not $NoReranker) {
    $required['reranker model'] = Join-Path $qwen 'model.onnx'
    $required['reranker token'] = Join-Path $qwen 'tokenizer.json'
}

$missing = @()
foreach ($item in $required.GetEnumerator()) {
    if (-not (Test-Path $item.Value)) { $missing += "  $($item.Key): $($item.Value)" }
}
if ($missing.Count -gt 0) {
    Write-Host 'Cannot launch - missing required files:' -ForegroundColor Red
    $missing | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    if (-not (Test-Path $exe)) {
        Write-Host ''
        Write-Host 'Build first:  cargo build -p vault-tauri' -ForegroundColor Yellow
    }
    exit 1
}

# ── Environment ─────────────────────────────────────────────────────────
$env:VAULT_ORT_LIB_PATH   = Join-Path $bge 'onnxruntime.dll'
$env:VAULT_MODEL_PATH     = Join-Path $bge 'model.onnx'
$env:VAULT_TOKENIZER_PATH = Join-Path $bge 'tokenizer.json'

if ($NoReranker) {
    Remove-Item Env:\VAULT_RERANK_MODEL_PATH -ErrorAction SilentlyContinue
    Remove-Item Env:\VAULT_RERANK_TOKENIZER_PATH -ErrorAction SilentlyContinue
} else {
    # Both or neither - main.rs treats a half-set pair as unset, because a
    # mismatched model/tokenizer produces correctly-shaped but semantically
    # shifted output (the insidious class flagged in vault_embedding::integrity).
    $env:VAULT_RERANK_MODEL_PATH     = Join-Path $qwen 'model.onnx'
    $env:VAULT_RERANK_TOKENIZER_PATH = Join-Path $qwen 'tokenizer.json'
}

# ADR-038: dev cargo runs inherit this from .cargo/config.toml; a directly
# launched exe does not. Without it, lance/datafusion runs with an unbounded
# memory pool and merge-heavy writes can OOM-abort the process.
$env:LANCE_MEM_POOL_SIZE = '268435456'

# Startup lines we want visible are logged at INFO on vault_app::startup.
if (-not $env:RUST_LOG) { $env:RUST_LOG = 'info' }

# ── Launch ──────────────────────────────────────────────────────────────
Write-Host ''
Write-Host "Launching  : $exe" -ForegroundColor Cyan
Write-Host "Vault data : $env:APPDATA\com.shahbaz242630.memory-vault" -ForegroundColor Cyan
if ($NoReranker) {
    Write-Host 'Reranker   : DISABLED (expect "no reranker model configured")' -ForegroundColor Yellow
} else {
    Write-Host 'Reranker   : enabled (expect "reranker configured: Qwen3-Reranker-0.6B")' -ForegroundColor Green
}
Write-Host ''
Write-Host 'NOTE: single-writer vault - do not run vault-cli while this is open.' -ForegroundColor DarkYellow
Write-Host ''

& $exe
exit $LASTEXITCODE
