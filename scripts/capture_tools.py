#!/usr/bin/env python3
"""Capture the release server's exposed MCP tool catalog and a Markdown index."""
import argparse
import json
from pathlib import Path

from mcp_client import Client

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--mx", default="target/release/mx")
parser.add_argument("--output", default="docs/profiling")
args = parser.parse_args()

output = Path(args.output)
output.mkdir(parents=True, exist_ok=True)
client = Client(Path(args.mx).resolve())
try:
    catalog = client.request("tools/list", {})
finally:
    client.close()

(output / "tools.json").write_text(json.dumps(catalog, indent=2) + "\n")
rows = [
    "# Exposed MCP tools\n\n",
    "Captured from the release server's `tools/list`. Full argument schemas: [tools.json](tools.json).\n\n",
    "| Tool | Description |\n| --- | --- |\n",
]
for tool in catalog["tools"]:
    description = tool.get("description", "").replace("|", "\\|")
    rows.append(f"| `{tool['name']}` | {description} |\n")
(output / "tools.md").write_text("".join(rows))
print(f"Captured {len(catalog['tools'])} tools.")
