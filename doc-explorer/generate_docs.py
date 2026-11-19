#!/usr/bin/env python3
"""Generate formal documentation for the midwimble Rust crate."""

import os
import sys
import time
import json
import re
import requests

HERE = os.path.dirname(os.path.abspath(__file__))
SRC_DIR = os.path.join(HERE, "..", "src")
OUT_FILE = os.path.join(HERE, "docs.jsonl")
API_URL = "http://192.168.50.79:8080/v1/chat/completions"

SYSTEM_PROMPT = """You are a highly capable software engineering agent. You are an expert in Rust and formal methods.
You are generating documentation for a Rust codebase.
For each function/method, provide:
1. **Reasoning**: Why the function exists and what role it plays.
2. **Preconditions**: What must be true before calling.
3. **Postconditions**: What is guaranteed after the call.
4. **Z Notation (informal)**: A simple schema or logic notation if applicable.
5. **Doc comment**: A concise Rust docstring.

Output the documentation in Markdown format. Be thorough but precise."""


def chunk_rust_code(code, max_lines=1200):
    """Split Rust code into chunks of at most max_lines, preferring top-level items."""
    lines = code.splitlines()
    if len(lines) <= max_lines:
        return [code]

    chunks = []
    current = []
    for line in lines:
        current.append(line)
        # Start a new chunk when we hit a top-level item and have enough lines
        if len(current) >= max_lines and re.match(r"^(pub\s+)?(fn|struct|enum|trait|impl|mod|const|static)\b", line):
            chunks.append("\n".join(current))
            current = []
    if current:
        chunks.append("\n".join(current))
    return chunks


def query_model(code, file_path):
    prompt = f"Generate documentation for the following Rust file portion ({file_path}):\n\n```rust\n{code}\n```"
    payload = {
        "model": "local-nemotron",
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": prompt},
        ],
        "temperature": 0.2,
        "max_tokens": 4096,
    }
    start = time.time()
    resp = requests.post(API_URL, json=payload, timeout=600)
    resp.raise_for_status()
    duration = time.time() - start
    content = resp.json()["choices"][0]["message"]["content"]
    return content, duration


def main():
    # Build the list of Rust files
    rust_files = []
    for root, _, files in os.walk(SRC_DIR):
        for f in sorted(files):
            if f.endswith(".rs"):
                rust_files.append(os.path.join(root, f))

    print(f"Found {len(rust_files)} Rust files.")
    os.makedirs(os.path.dirname(OUT_FILE), exist_ok=True)

    with open(OUT_FILE, "a") as out:
        for path in rust_files:
            rel = os.path.relpath(path, SRC_DIR)
            print(f"Processing {rel}...", flush=True)
            with open(path) as f:
                code = f.read()

            chunks = chunk_rust_code(code)
            for i, chunk in enumerate(chunks):
                label = f"{rel} (chunk {i+1}/{len(chunks)})" if len(chunks) > 1 else rel
                print(f"  -> {label} ({len(chunk)} chars)", flush=True)
                try:
                    doc, duration = query_model(chunk, label)
                    entry = {
                        "file": rel,
                        "chunk": i + 1,
                        "total_chunks": len(chunks),
                        "chars": len(chunk),
                        "duration_s": round(duration, 2),
                        "documentation": doc,
                    }
                    out.write(json.dumps(entry) + "\n")
                    out.flush()
                except Exception as e:
                    print(f"  !! FAILED: {e}", flush=True)
                    entry = {
                        "file": rel,
                        "chunk": i + 1,
                        "total_chunks": len(chunks),
                        "error": str(e),
                    }
                    out.write(json.dumps(entry) + "\n")
                    out.flush()
                time.sleep(1)

    print("Done.")


if __name__ == "__main__":
    main()