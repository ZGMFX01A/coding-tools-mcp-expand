# Agent evaluation

Imported from xyTom/coding-tools-mcp commit `9d2c179cb307a7121bf7a83420d9d4f7815afb45` under its Apache-2.0 license (see UPSTREAM_LICENSE). The task loader, verification loop and scorer are upstream code. The server launcher uses this repository's Rust binary.

Build once with `cargo build --manifest-path src-tauri/Cargo.toml`. By default the launcher uses src-tauri/target/debug/coding-tools-mcp-desktop (with .exe on Windows). Set CODING_TOOLS_EVAL_SERVER_BINARY to an absolute binary path to override it. The headless server binds only to localhost; each task receives an isolated Harness directory alongside its checkout.

Validate the starter manifest without launching an agent:

```text
python -m benchmarks.agent_eval.run_eval --validate-only
```

Run a comparison with your chosen agent commands:

```text
python -m benchmarks.agent_eval.run_eval --arm "native=<agent command>" --arm "mcp:mcp=<agent command>" --runs-out runs.json --report-out report.json
```

The runner sends the task prompt on stdin and CODING_TOOLS_EVAL_PROMPT. The MCP arm additionally receives CODING_TOOLS_MCP_URL. Commands and their API costs are under the operator's control. The report measures first-attempt success, rounds to green, regressions, and setup failures. The three starter fixtures are offline warm-up tasks; they are not the upstream 30-task release evaluation.

Task directories are recreated for each run. Use a dedicated --workdir containing only evaluation outputs. Starter verification commands use python3; on Windows where only python is installed, use the included starter-windows.json manifest.
