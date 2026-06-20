# Skill Registry

This document lists the available agent skills for this project.

## Local Workspace Skills (`.agents/skills/`)

### rust-best-practices
**Description:** Guide for writing idiomatic Rust code based on Apollo GraphQL's best practices handbook. Use this skill when: (1) writing new Rust code or functions, (2) reviewing or refactoring existing Rust code, (3) deciding between borrowing vs cloning or ownership patterns, (4) implementing error handling with Result types, (5) optimizing Rust code for performance, (6) writing tests or documentation for Rust projects.
**Path:** `.agents/skills/rust-best-practices`

### rust-mcp-server-generator
**Description:** Generate a complete Rust Model Context Protocol server project with tools, prompts, resources, and tests using the official rmcp SDK
**Path:** `.agents/skills/rust-mcp-server-generator`

### semantic-search
**Description:** Use when searching for code by concept, meaning, or behavior — not by exact symbol name or literal string. Ideal for queries like "payment retry logic", "user authentication flow", "error handling for database connections", or "functions similar to createUser". Do NOT use for exact string matches (use grep) or known symbol lookups (use codegraph_explore).
**Path:** `.agents/skills/semantic-search`
