#!/usr/bin/env python3
"""Generate a self-contained HTML documentation explorer from docs.jsonl."""

import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
DOCS_JSONL = os.path.join(HERE, "docs.jsonl")
OUT_HTML = os.path.join(HERE, "index.html")

# Load and combine chunks per file
files = {}
with open(DOCS_JSONL) as f:
    for line in f:
        e = json.loads(line)
        path = e["file"]
        if path not in files:
            files[path] = []
        if "documentation" in e:
            files[path].append(e["documentation"])

# Build a list of {path, module, doc} sorted by path
docs = []
for path in sorted(files):
    module = path.split("/")[0] if "/" in path else "(root)"
    docs.append({
        "path": path,
        "module": module,
        "doc": "\n\n".join(files[path]),
    })

# Group by module
modules = {}
for d in docs:
    modules.setdefault(d["module"], []).append(d)

modules_json = json.dumps(modules, ensure_ascii=False)

html = f"""<!DOCTYPE html>
<html lang="en" data-theme="dark">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Midwimble Crate Documentation</title>
<script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/katex.min.css">
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/katex.min.js"></script>
<script defer src="https://cdn.jsdelivr.net/npm/katex@0.16.9/dist/contrib/auto-render.min.js" onload="window.katexReady = true"></script>
<style>
  :root {{
    --bg: #0f0e17;
    --bg2: #1a1926;
    --bg3: #252438;
    --text: #f4f3f7;
    --muted: #9c98aa;
    --accent: #8b5cf6;
    --accent2: #a78bfa;
    --border: #2e2c42;
    --code-bg: #1e1d2e;
    --green: #00ff41;
  }}
  [data-theme="light"] {{
    --bg: #faf8ff;
    --bg2: #f0edf7;
    --bg3: #e4e0f0;
    --text: #1a1a2e;
    --muted: #6b6899;
    --accent: #7c3aed;
    --accent2: #8b5cf6;
    --border: #d4cfe6;
    --code-bg: #f3f1fa;
    --green: #059669;
  }}
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: var(--bg);
    color: var(--text);
    display: flex;
    height: 100vh;
    overflow: hidden;
  }}
  aside {{
    width: 320px;
    min-width: 320px;
    background: var(--bg2);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    height: 100vh;
  }}
  aside header {{
    padding: 20px;
    border-bottom: 1px solid var(--border);
  }}
  aside header h1 {{
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }}
  aside header h1 span {{ color: var(--accent); }}
  aside header p {{
    font-size: 12px;
    color: var(--muted);
    margin-top: 4px;
  }}
  #search {{
    margin: 12px 20px;
    padding: 10px 14px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    font-size: 13px;
    outline: none;
    transition: border-color 0.2s;
  }}
  #search:focus {{ border-color: var(--accent); }}
  nav {{
    flex: 1;
    overflow-y: auto;
    padding: 0 12px 20px;
  }}
  .module {{
    margin-top: 16px;
  }}
  .module-title {{
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    padding: 4px 8px;
    font-weight: 600;
  }}
  .file-link {{
    display: block;
    padding: 7px 10px;
    font-size: 13px;
    color: var(--text);
    text-decoration: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.15s;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }}
  .file-link:hover {{ background: var(--bg3); }}
  .file-link.active {{
    background: var(--accent);
    color: #fff;
  }}
  main {{
    flex: 1;
    overflow-y: auto;
    padding: 40px 60px;
    max-width: 1100px;
  }}
  main .doc-header {{
    margin-bottom: 32px;
  }}
  main .doc-header h2 {{
    font-size: 28px;
    font-weight: 600;
    margin-bottom: 8px;
  }}
  main .doc-header .breadcrumb {{
    font-size: 12px;
    color: var(--muted);
    font-family: ui-monospace, 'JetBrains Mono', monospace;
  }}
  main .doc-content {{
    line-height: 1.7;
    font-size: 15px;
  }}
  .doc-content h2 {{
    font-size: 20px;
    margin: 32px 0 16px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border);
  }}
  .doc-content h3 {{
    font-size: 16px;
    margin: 24px 0 12px;
    color: var(--accent2);
  }}
  .doc-content p {{ margin-bottom: 12px; }}
  .doc-content ul, .doc-content ol {{
    margin: 12px 0 12px 24px;
  }}
  .doc-content li {{ margin-bottom: 6px; }}
  .doc-content code {{
    background: var(--code-bg);
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 0.9em;
    font-family: ui-monospace, 'JetBrains Mono', monospace;
  }}
  .doc-content pre {{
    background: var(--code-bg);
    padding: 16px;
    border-radius: 8px;
    overflow-x: auto;
    margin: 16px 0;
    border: 1px solid var(--border);
  }}
  .doc-content pre code {{
    background: none;
    padding: 0;
  }}
  .doc-content blockquote {{
    border-left: 3px solid var(--accent);
    padding-left: 16px;
    margin: 16px 0;
    color: var(--muted);
  }}
  .doc-content table {{
    border-collapse: collapse;
    width: 100%;
    margin: 16px 0;
  }}
  .doc-content th, .doc-content td {{
    border: 1px solid var(--border);
    padding: 8px 12px;
    text-align: left;
  }}
  .doc-content th {{ background: var(--bg2); }}
  #themeToggle {{
    margin: 12px 20px;
    padding: 8px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    cursor: pointer;
    font-size: 13px;
  }}
  #themeToggle:hover {{ border-color: var(--accent); }}
  .empty {{
    color: var(--muted);
    text-align: center;
    margin-top: 80px;
    font-size: 14px;
  }}
</style>
</head>
<body>
<aside>
  <header>
    <h1><span>Midwimble</span> Docs</h1>
    <p id="stats"></p>
  </header>
  <input id="search" type="text" placeholder="Filter files...">
  <nav id="nav"></nav>
  <button id="themeToggle">Toggle theme</button>
</aside>
<main id="main">
  <div class="empty">Select a file to view its documentation.</div>
</main>
<script>
const MODULES = {modules_json};
const allDocs = Object.values(MODULES).flat();
document.getElementById('stats').textContent = `${{allDocs.length}} files documented`;

function renderNav(filter) {{
  const nav = document.getElementById('nav');
  nav.innerHTML = '';
  for (const [mod, docs] of Object.entries(MODULES)) {{
    const matches = docs.filter(d => d.path.toLowerCase().includes(filter));
    if (matches.length === 0) continue;
    const modDiv = document.createElement('div');
    modDiv.className = 'module';
    const title = document.createElement('div');
    title.className = 'module-title';
    title.textContent = mod;
    modDiv.appendChild(title);
    for (const d of matches) {{
      const a = document.createElement('a');
      a.className = 'file-link';
      a.textContent = d.path;
      a.dataset.path = d.path;
      a.onclick = () => showDoc(d, a);
      modDiv.appendChild(a);
    }}
    nav.appendChild(modDiv);
  }}
}}

function showDoc(d, el) {{
  for (const a of document.querySelectorAll('.file-link')) a.classList.remove('active');
  if (el) el.classList.add('active');
  const main = document.getElementById('main');

  // Protect LaTeX blocks from Markdown parsing, then restore them.
  const mathBlocks = [];
  const protectedDoc = d.doc.replace(/\\$([^$]+)\\$/g, (match) => {{
    const id = '%%MATH' + mathBlocks.length + '%%';
    mathBlocks.push(match);
    return id;
  }});
  let html = marked.parse(protectedDoc);
  html = html.replace(/%%MATH(\\d+)%%/g, (_, i) => mathBlocks[parseInt(i)]);

  main.innerHTML = `
    <div class="doc-header">
      <h2>${{d.path.split('/').pop()}}</h2>
      <div class="breadcrumb">${{d.path}}</div>
    </div>
    <div class="doc-content">${{html}}</div>
  `;
  renderMath(main);
  main.scrollTop = 0;
}}

function renderMath(container) {{
  if (window.renderMathInElement) {{
    renderMathInElement(container, {{
      delimiters: [
        {{left: '$$', right: '$$', display: true}},
        {{left: '$', right: '$', display: false}},
        {{left: '\\\\(', right: '\\\\)', display: false}},
        {{left: '\\\\[', right: '\\\\]', display: true}}
      ],
      throwOnError: false
    }});
  }} else {{
    setTimeout(() => renderMath(container), 300);
  }}
}}

document.getElementById('search').addEventListener('input', (e) => {{
  renderNav(e.target.value.toLowerCase());
}});

document.getElementById('themeToggle').addEventListener('click', () => {{
  const html = document.documentElement;
  const next = html.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
  html.setAttribute('data-theme', next);
  localStorage.setItem('mw-docs-theme', next);
}});

if (localStorage.getItem('mw-docs-theme')) {{
  document.documentElement.setAttribute('data-theme', localStorage.getItem('mw-docs-theme'));
}}

renderNav('');

// Open the first file by default
const first = document.querySelector('.file-link');
if (first) first.click();
</script>
</body>
</html>
"""

with open(OUT_HTML, "w") as f:
    f.write(html)

print(f"Wrote {OUT_HTML} ({len(html):,} bytes, {len(docs)} files)")