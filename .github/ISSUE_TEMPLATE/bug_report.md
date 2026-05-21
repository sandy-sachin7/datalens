---
name: Bug report
about: Something is broken or producing wrong output
title: ''
labels: bug
assignees: ''
---

## Describe the bug

A clear description of what went wrong.

## Reproduction

```bash
# Exact command you ran
deltalens inspect /path/to/table --json
```

## Expected vs actual

```
Expected: ...
Actual: ...
```

## Environment

- OS: [e.g. Ubuntu 24.04, macOS 15.2]
- Installation: [cargo install / pre-built binary / built from source]
- Version: `deltalens --version`
- Delta table: approximate size (# commits, # files)

## Delta log sample (if relevant)

```json
# Include a line or two from a problematic commit file
```

## Additional context

- Does the table contain custom action types not in the Delta spec?
- Are there checkpoint `.parquet` files present?
