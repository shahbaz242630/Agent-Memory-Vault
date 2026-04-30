# Model + tokenizer provenance — bge-small-en-v1.5

Captured during T0.1.7 Spike 3 (2026-04-30). This document is the canonical
record of where the bundled model / tokenizer / ORT runtime files come from,
their licenses, and their integrity hashes. Updates land alongside any
fixture refresh; the SHA-256 values here drive `crates/vault-embedding/src/integrity.rs`.

## Embedding model — `bge-small-en-v1.5`

| Field | Value |
|---|---|
| Author | BAAI (Beijing Academy of Artificial Intelligence) |
| Source repo | <https://huggingface.co/BAAI/bge-small-en-v1.5> |
| ONNX export | **Official** — committed by BAAI in the same repo (commit `5c38ec7`, "onnx support (#9)") |
| ONNX file | <https://huggingface.co/BAAI/bge-small-en-v1.5/blob/main/onnx/model.onnx> |
| Format | ONNX FP32 (no quantization for V0.1 — see T0.1.7_PLAN.md Q1 refinement) |
| Size | 133 MB |
| Architecture | BERT (`BertModel`), 12 layers, 12 attention heads, hidden_size 384 |
| Hidden dim | **384** (matches `vault_embedding::EMBEDDING_DIM` and `LanceVectorStore` configuration) |
| Max sequence | 512 tokens |
| Pooling | **CLS-token** per `1_Pooling/config.json` (`pooling_mode_cls_token: true`). NOT mean-pool. Test 9 enforces. |
| License | MIT |
| **SHA-256** | `828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35` |
| Compiled-in const | `BGE_SMALL_EN_V1_5_MODEL_SHA256` in `src/integrity.rs` |

**No conversion provenance issue.** The ONNX file is officially distributed
by BAAI in the same Hugging Face repo as the safetensors — we are not
shipping a community-converted artefact. ADR-022 was therefore NOT created
(see T0.1.7_PLAN.md ADR list, post-spike).

## Tokenizer — `tokenizer.json`

| Field | Value |
|---|---|
| Source | <https://huggingface.co/BAAI/bge-small-en-v1.5/blob/main/tokenizer.json> |
| Size | 711 KB |
| Class | `BertTokenizer` (`tokenizer_config.json` confirms) |
| Casing | `do_lower_case: true` (uncased) |
| Max length | 512 (NOT auto-applied by Rust `tokenizers` crate — see Phase 3 implementation note) |
| Pad token id | 0 (`[PAD]`) |
| Last commit | `9b09f79fe618f3969e13e52f12c01de64145ebc0` |
| **SHA-256** | `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66` (captured 2026-04-30 via `scripts/setup-dev-env.ps1`; pinned in `src/integrity.rs::BGE_SMALL_EN_V1_5_TOKENIZER_SHA256`) |

## ONNX Runtime native library — `onnxruntime.{dll,dylib,so}`

| Field | Value |
|---|---|
| Author | Microsoft |
| Source | <https://github.com/microsoft/onnxruntime/releases> |
| Version | **1.22.0** (Windows: <https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-win-x64-1.22.0.zip>; Linux/macOS via `setup-dev-env.sh`). Pinned via `ORT_VERSION` in setup scripts. **MUST match ort crate's expected version** — ort `=2.0.0-rc.10` binds to ORT `1.22.x` (encoded in `ort_sys::ORT_API_VERSION = 22`). When bumping ort, check release notes for the new bundled ORT version and update setup scripts together. |
| Linkage | Dynamic via `ort` crate's `load-dynamic` feature (Spike 1) |
| Init pattern | `ort::init_from(dylib_path)` — MUST be called once before any other ort use; idempotent via `std::sync::Once` (see Phase 1 implementation note in plan) |
| License | MIT |

The ORT dylib is loaded at runtime, not link-time. Choosing `load-dynamic` over
the default `download-binaries` (which fetches at build time) gives us:
- Deterministic builds (no Microsoft CDN dependency at compile time).
- Single point of CVE patching (swap the file, no rebuild).
- Cleaner signed-binary chain at T0.1.11 (the dylib is bundled inside the
  signed installer — see T0.1.7_PLAN.md "Notes for T0.1.11").

## Re-fetch / verify procedure

If the source disappears or you need to verify the bundle in CI:

```bash
# Linux / macOS
curl -L -o model.onnx \
  "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx?download=true"
sha256sum model.onnx
# Expect: 828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35

curl -L -o tokenizer.json \
  "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json"
sha256sum tokenizer.json
# Compare to BGE_SMALL_EN_V1_5_TOKENIZER_SHA256 in src/integrity.rs
```

```powershell
# Windows
Invoke-WebRequest "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx?download=true" -OutFile model.onnx
Get-FileHash model.onnx -Algorithm SHA256

Invoke-WebRequest "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json" -OutFile tokenizer.json
Get-FileHash tokenizer.json -Algorithm SHA256
```

`scripts/setup-dev-env.{sh,ps1}` automates both downloads + verification +
ORT dylib download for the host platform.
