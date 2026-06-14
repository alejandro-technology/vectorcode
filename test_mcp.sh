#!/bin/bash
(
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"test","version":"1.0"}}}'
sleep 0.2
echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}'
sleep 0.2
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
sleep 1
) | target/debug/vectorcode serve --mcp 
