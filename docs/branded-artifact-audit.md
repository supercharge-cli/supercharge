# Branded artifact audit

The previously distributed Catbox artifact was downloaded from https://files.catbox.moe/wx7sot.gz on 2026-08-20. It contained `supercharge-linux-x86_64`, `README.txt`, and `SHA256SUMS`, but the executable reported `grok 1.0.5 (5dcf5f7)` with `--version` and its help still showed `Usage: grok`. This proves it was the older pre-branding binary, despite the archive filename.

The source checkout being rebuilt is `/home/ubuntu/hermes-agent-alternative-full` at the pushed source commit `075e017`. The rebuild uses Rust 1.94.1, `CARGO_BUILD_JOBS=1`, no incremental compilation, and LLD. Its output log is `/tmp/supercharge-branded-build.log`; the target directory is `target/supercharge-branded`. The source patch changes the default model-facing system-prompt identity to `Supercharge AI` and adds the Supercharge-first `SUPERCHARGE_SYSTEM_PROMPT_LABEL` environment override while preserving the inherited variable only for compatibility.
