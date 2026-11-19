# Midwimble crate documentation explorer

Auto-generated formal documentation for the Midwimble Rust crate: reasoning,
pre/postconditions and informal Z notation for each type and function, produced
by the local Nemotron model.

## Contents

- `index.html` — self-contained explorer (open in any browser, no server needed).
- `docs.jsonl` — raw documentation data, one JSON object per file chunk.
- `generate_docs.py` — regenerates `docs.jsonl` from `../src`.
- `generate_explorer.py` — rebuilds `index.html` from `docs.jsonl`.

## Regenerate

```bash
python3 generate_docs.py      # needs llama-server on 192.168.50.79:8080
python3 generate_explorer.py
```

## Serve

```bash
python3 -m http.server 8000
# open http://localhost:8000/doc-explorer/
```
