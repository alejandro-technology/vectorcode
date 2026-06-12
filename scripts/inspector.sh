#!/usr/bin/env bash
set -e

# Run the MCP inspector against the locally built vectorcode server.
echo "Starting MCP Inspector against vectorcode..."
npx @modelcontextprotocol/inspector cargo run -- serve --mcp
